//! Look-ahead for the path bar: while the user types an absolute local path,
//! suggest the folder name that completes the last component.
//!
//! [`suggest`] lists a folder, so the path bar runs it on a worker thread;
//! [`Requests`] tells which answer belongs to the newest keystroke.

use std::cell::Cell;
use std::path::Path;

/// Numbers the look-ahead requests, so the answer to an older keystroke that
/// arrives after a newer one was started is dropped.
#[derive(Default)]
pub struct Requests {
    latest: Cell<u64>,
}

impl Requests {
    /// Start a new request and return its number.
    pub fn next(&self) -> u64 {
        self.latest.set(self.latest.get() + 1);
        self.latest.get()
    }

    /// Whether `id` is still the newest request.
    pub fn is_current(&self, id: u64) -> bool {
        self.latest.get() == id
    }
}

/// `text` with a leading `~` (alone or followed by `/`) replaced by `$HOME`.
/// Any other text, or an unset `$HOME`, is returned unchanged.
pub fn expand_tilde(text: &str) -> String {
    let Some(rest) = text.strip_prefix('~') else { return text.to_string() };
    if !(rest.is_empty() || rest.starts_with('/')) { return text.to_string(); }
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => format!("{}{rest}", home.trim_end_matches('/')),
        _ => text.to_string(),
    }
}

/// Suffix that completes `text` to an existing local folder, or `None` when
/// there is nothing to suggest. The suffix ends with `/`. Only absolute local
/// paths (a leading `~` counts as `$HOME`) are completed: bookmarks, remote URLs and archive paths are ignored.
/// With several matches the first one in alphabetical order wins.
pub fn suggest(text: &str) -> Option<String> {
    let expanded = expand_tilde(text);
    let text = expanded.as_str();
    if !text.starts_with('/') || text.contains('#') { return None; }
    let slash = text.rfind('/')?;
    let (dir, prefix) = (&text[..=slash], &text[slash + 1..]);
    if prefix.is_empty() { return None; }
    let mut names: Vec<String> = Path::new(dir)
        .read_dir()
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok().map(|n| (e, n)))
        .filter(|(_, n)| n.starts_with(prefix))
        .filter(|(e, _)| is_folder(e))
        .map(|(_, n)| n)
        .collect();
    names.sort();
    let first = names.into_iter().next()?;
    Some(format!("{}/", &first[prefix.len()..]))
}

/// Where the look-ahead `ghost` starts when it must be selected again: it is
/// still the end of `text`, the cursor (`cursor`, in characters) sits right
/// before it and nothing is selected. GTK drops the selection of an entry when
/// it believes the X11 PRIMARY selection was lost, which a late ownership
/// notice can make it believe while the user types fast; the next key would
/// then be inserted before the ghost instead of replacing it. `None` when
/// there is nothing to restore (no ghost, still selected, removed, or a cursor
/// the user moved).
pub fn ghost_to_restore(text: &str, cursor: i32, has_selection: bool, ghost: &str) -> Option<i32> {
    if ghost.is_empty() || has_selection || !text.ends_with(ghost) { return None; }
    let start = (text.chars().count() - ghost.chars().count()) as i32;
    (cursor == start).then_some(start)
}

/// Whether the directory entry is a folder. The type comes from `read_dir`
/// itself; only a symbolic link is followed with a `stat`.
fn is_folder(entry: &std::fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(t) if t.is_symlink() => entry.path().is_dir(),
        Ok(t) => t.is_dir(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_ghost_that_lost_its_selection_is_selected_again() {
        // "/a/s" typed, "ub/" suggested; GTK dropped the selection.
        assert_eq!(ghost_to_restore("/a/sub/", 4, false, "ub/"), Some(4));
    }

    #[test]
    fn a_ghost_is_left_alone_when_it_is_still_selected_or_gone() {
        assert_eq!(ghost_to_restore("/a/sub/", 4, true, "ub/"), None, "still selected");
        assert_eq!(ghost_to_restore("/a/sub/", 4, false, ""), None, "no ghost");
        assert_eq!(ghost_to_restore("/a/s", 4, false, "ub/"), None, "the ghost text was removed");
        assert_eq!(ghost_to_restore("/a/sub/x", 4, false, "ub/"), None, "the ghost is not at the end any more");
    }

    #[test]
    fn a_cursor_the_user_moved_is_not_overridden() {
        assert_eq!(ghost_to_restore("/a/sub/", 3, false, "ub/"), None, "Left arrow");
        assert_eq!(ghost_to_restore("/a/sub/", 7, false, "ub/"), None, "End: the user took the suggestion");
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        // "/è/s" is 4 characters but 5 bytes.
        assert_eq!(ghost_to_restore("/è/sub/", 4, false, "ub/"), Some(4));
    }

    /// Temp tree: alpha/, alpine/, .hidden/, beta/ and a plain file `alfile`.
    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for d in ["alpha", "alpine", ".hidden", "beta"] {
            fs::create_dir(dir.path().join(d)).unwrap();
        }
        fs::write(dir.path().join("alfile"), b"x").unwrap();
        dir
    }

    fn typed(dir: &tempfile::TempDir, tail: &str) -> String {
        format!("{}/{}", dir.path().display(), tail)
    }

    #[test]
    fn completes_a_unique_folder() {
        let dir = tree();
        assert_eq!(suggest(&typed(&dir, "be")), Some("ta/".to_string()));
    }

    #[test]
    fn picks_the_first_match_alphabetically() {
        let dir = tree();
        assert_eq!(suggest(&typed(&dir, "al")), Some("pha/".to_string()));
    }

    #[test]
    fn ignores_plain_files() {
        let dir = tree();
        assert_eq!(suggest(&typed(&dir, "alf")), None);
    }

    #[test]
    fn hidden_folders_need_a_leading_dot() {
        let dir = tree();
        assert_eq!(suggest(&typed(&dir, "")), None);
        assert_eq!(suggest(&typed(&dir, ".h")), Some("idden/".to_string()));
    }

    #[test]
    fn exact_name_suggests_only_the_slash() {
        let dir = tree();
        assert_eq!(suggest(&typed(&dir, "beta")), Some("/".to_string()));
    }

    #[test]
    fn no_match_gives_nothing() {
        let dir = tree();
        assert_eq!(suggest(&typed(&dir, "zzz")), None);
    }

    #[test]
    fn completes_below_the_root() {
        assert_eq!(suggest("/et"), Some("c/".to_string()));
    }

    #[test]
    fn ignores_non_local_or_relative_text() {
        assert_eq!(suggest(""), None);
        assert_eq!(suggest("docs"), None);
        assert_eq!(suggest("bm:su"), None);
        assert_eq!(suggest("sftp://host/va"), None);
        assert_eq!(suggest("/tmp/file.zip#in"), None);
    }

    #[test]
    fn tilde_expands_only_as_a_home_prefix() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/x"), format!("{}/x", home.trim_end_matches('/')));
        assert_eq!(expand_tilde("~user/x"), "~user/x");
        assert_eq!(expand_tilde("/a/~"), "/a/~");
    }

    #[test]
    fn a_symlink_to_a_folder_counts_as_a_folder() {
        let dir = tree();
        std::os::unix::fs::symlink(dir.path().join("beta"), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("alfile"), dir.path().join("lfile")).unwrap();
        assert_eq!(suggest(&typed(&dir, "li")), Some("nk/".to_string()));
        assert_eq!(suggest(&typed(&dir, "lf")), None, "a link to a file is not a folder");
    }

    #[test]
    fn only_the_newest_request_is_current() {
        let requests = Requests::default();
        let first = requests.next();
        let second = requests.next();
        assert!(!requests.is_current(first));
        assert!(requests.is_current(second));
    }

    #[test]
    fn missing_parent_gives_nothing() {
        assert_eq!(suggest("/definitely/not/here/x"), None);
    }
}
