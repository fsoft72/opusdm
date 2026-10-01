//! Bookmarks (F-48), the Linux stand-in for Amiga assigns: a name that
//! stands for a folder, usable in the path bar as `name:` or `name:sub/dir`.
//! Kept in `~/.config/opusdm/bookmarks.json`.

use crate::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// Names that would clash with the remote address schemes.
const RESERVED_NAMES: [&str; 3] = ["ftp", "ftps", "sftp"];

/// One bookmark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Bookmark {
    /// Letters, digits, `_` and `-`; compared without regard to case.
    pub name: String,
    /// Absolute folder, or one starting with `~`.
    pub path: String,
}

/// The saved bookmarks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Bookmarks {
    /// In display order.
    pub items: Vec<Bookmark>,
}

/// Whether `name` can be a bookmark name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !RESERVED_NAMES.contains(&name.to_ascii_lowercase().as_str())
}

/// `~` and `~/x` expanded with `home`; any other path unchanged.
fn expand_home(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

impl Bookmarks {
    /// What the path bar text means when it starts with a bookmark name:
    /// `docs:` is the folder, `docs:sub/x` something below it. `None` when
    /// `text` is not a bookmark reference (unknown name, an address like
    /// `sftp://...`, a plain path). `.` and `..` segments are refused so a
    /// reference cannot climb out of its bookmark.
    pub fn resolve(&self, text: &str, home: &Path) -> Option<PathBuf> {
        let (name, rest) = text.trim().split_once(':')?;
        if rest.starts_with("//") || !valid_name(name) {
            return None;
        }
        let item = self.items.iter().find(|b| b.name.eq_ignore_ascii_case(name))?;
        let mut path = expand_home(&item.path, home);
        for segment in rest.split('/').filter(|s| !s.is_empty()) {
            if segment == ".." || segment == "." {
                return None;
            }
            path.push(segment);
        }
        Some(path)
    }

    /// Every problem that should block saving.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for b in &self.items {
            if !valid_name(&b.name) {
                out.push(format!("{:?} is not a usable name (letters, digits, _ and - only; not ftp, ftps or sftp)", b.name));
            } else if !seen.insert(b.name.to_ascii_lowercase()) {
                out.push(format!("{}: the name is used twice", b.name));
            }
            if !(b.path.starts_with('/') || b.path == "~" || b.path.starts_with("~/")) {
                out.push(format!("{}: the folder must be absolute or start with ~", b.name));
            }
        }
        out
    }

    /// Add `name` for `path`, or replace the folder of an existing `name`.
    pub fn set(&mut self, name: &str, path: &str) {
        match self.items.iter_mut().find(|b| b.name.eq_ignore_ascii_case(name)) {
            Some(existing) => existing.path = path.to_string(),
            None => self.items.push(Bookmark { name: name.to_string(), path: path.to_string() }),
        }
    }
}

/// Path to `bookmarks.json`.
pub fn bookmarks_path() -> PathBuf {
    config_dir().join("bookmarks.json")
}

/// Load the bookmarks from `path` (empty when missing; a file that does not
/// parse is copied to `.bak` first so a later save cannot destroy it).
pub fn load_bookmarks_from(path: &Path) -> Bookmarks {
    let unparsable = std::fs::read_to_string(path).is_ok_and(|t| serde_json::from_str::<Bookmarks>(&t).is_err());
    if unparsable {
        let _ = std::fs::copy(path, path.with_extension("json.bak"));
        crate::err_log!("opusdm", "bookmarks.json does not parse; a copy was kept in bookmarks.json.bak");
    }
    load_json(path, "opusdm", "bookmarks")
}

/// Load the bookmarks from the standard path.
pub fn load_bookmarks() -> Bookmarks {
    load_bookmarks_from(&bookmarks_path())
}

/// Save the bookmarks to the standard path.
pub fn save_bookmarks(bookmarks: &Bookmarks) -> io::Result<()> {
    save_json(&bookmarks_path(), bookmarks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks() -> Bookmarks {
        Bookmarks {
            items: vec![
                Bookmark { name: "docs".into(), path: "/home/me/Documents".into() },
                Bookmark { name: "Work".into(), path: "~/work".into() },
            ],
        }
    }

    #[test]
    fn a_reference_resolves_to_the_folder_and_below() {
        let home = Path::new("/home/me");
        assert_eq!(marks().resolve("docs:", home), Some(PathBuf::from("/home/me/Documents")));
        assert_eq!(marks().resolve("DOCS:sub/dir", home), Some(PathBuf::from("/home/me/Documents/sub/dir")));
        assert_eq!(marks().resolve("work:x", home), Some(PathBuf::from("/home/me/work/x")));
        assert_eq!(marks().resolve("  docs:a//b/ ", home), Some(PathBuf::from("/home/me/Documents/a/b")));
    }

    #[test]
    fn what_is_not_a_reference_is_left_alone() {
        let home = Path::new("/home/me");
        for text in ["nope:", "/etc", "sftp://h/x", "docs", "ftp://docs", "bad name:x", ":x"] {
            assert_eq!(marks().resolve(text, home), None, "{text}");
        }
    }

    #[test]
    fn a_reference_cannot_climb_out_of_its_bookmark() {
        let home = Path::new("/home/me");
        assert_eq!(marks().resolve("docs:../etc", home), None);
        assert_eq!(marks().resolve("docs:a/./b", home), None);
    }

    #[test]
    fn names_and_folders_are_checked() {
        assert!(valid_name("my-docs_2") && !valid_name("") && !valid_name("a b") && !valid_name("SFTP") && !valid_name("a:b"));
        assert!(marks().problems().is_empty());
        let bad = Bookmarks {
            items: vec![
                Bookmark { name: "a".into(), path: "rel".into() },
                Bookmark { name: "A".into(), path: "/x".into() },
                Bookmark { name: "x y".into(), path: "/x".into() },
            ],
        };
        assert_eq!(bad.problems().len(), 3, "{:?}", bad.problems());
    }

    #[test]
    fn set_adds_or_replaces_by_name() {
        let mut m = marks();
        m.set("docs", "/new");
        m.set("fresh", "/f");
        assert_eq!(m.items.len(), 3);
        assert_eq!(m.items[0].path, "/new");
    }

    #[test]
    fn save_load_round_trip_and_a_bad_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.json");
        assert_eq!(load_bookmarks_from(&path), Bookmarks::default());
        save_json(&path, &marks()).unwrap();
        assert_eq!(load_bookmarks_from(&path), marks());
        std::fs::write(&path, "{ nope").unwrap();
        assert_eq!(load_bookmarks_from(&path), Bookmarks::default());
        assert_eq!(std::fs::read_to_string(dir.path().join("bookmarks.json.bak")).unwrap(), "{ nope");
    }
}
