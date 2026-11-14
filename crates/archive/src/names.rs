//! Pure helpers on archive entry names: suffix whitelist, normalisation,
//! selection and safety checks. No libarchive here, so they are easy to test.

use std::path::{Component, Path};

/// Suffixes the UI treats as browsable archives (lower case). The reader
/// itself detects the real format from the content.
const ARCHIVE_SUFFIXES: &[&str] = &[
    ".zip", ".7z", ".rar", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst",
    ".tar.lz4", ".iso", ".cpio",
];

/// Whether `name` looks like an archive this crate can read, by its suffix
/// (case-insensitive).
pub fn is_archive_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    ARCHIVE_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

/// `name` without its archive suffix (`a.tar.gz` -> `a`), matched
/// case-insensitively, longest suffix first. A name without a known suffix,
/// or made only of the suffix, comes back unchanged.
pub fn archive_stem(name: &str) -> &str {
    let bytes = name.as_bytes();
    let longest = ARCHIVE_SUFFIXES
        .iter()
        .filter(|s| bytes.len() > s.len() && bytes[bytes.len() - s.len()..].eq_ignore_ascii_case(s.as_bytes()))
        .map(|s| s.len())
        .max();
    match longest {
        // The suffixes are ASCII, so the cut falls on a char boundary.
        Some(len) => &name[..name.len() - len],
        None => name,
    }
}

/// Tracks whether the entries of an archive all sit in one top folder.
/// Feed it every normalised entry name with [`RootCheck::add`].
#[derive(Debug, Default)]
pub struct RootCheck {
    top: Option<String>,
    broken: bool,
}

impl RootCheck {
    /// Account for the normalised entry `name`; `is_dir` when the entry is a
    /// folder. Returns `false` once the archive is known to have no single
    /// top folder (the caller can stop reading).
    pub fn add(&mut self, name: &str, is_dir: bool) -> bool {
        if self.broken || name.is_empty() {
            return !self.broken;
        }
        let (top, below) = match name.split_once('/') {
            Some((top, _)) => (top, true),
            None => (name, false),
        };
        // A file (or link) at the top level is not inside a folder.
        if !below && !is_dir {
            self.broken = true;
            return false;
        }
        match &self.top {
            Some(t) if t != top => self.broken = true,
            Some(_) => {}
            None => self.top = Some(top.to_string()),
        }
        !self.broken
    }

    /// Whether every entry seen sits in one top folder. An empty archive
    /// counts as having one (there is nothing to wrap).
    pub fn has_root_folder(&self) -> bool {
        !self.broken
    }
}

/// An entry name without leading `./` and trailing `/`. The archive root
/// (`.`, `./`) becomes an empty string.
pub fn normalize(name: &str) -> String {
    let mut s = name;
    while let Some(rest) = s.strip_prefix("./") {
        s = rest;
    }
    let s = s.trim_end_matches('/');
    if s == "." { String::new() } else { s.to_string() }
}

/// Whether the normalised `entry` is picked by the normalised `selection`:
/// it is one of the names, or sits under one of them.
pub fn is_selected(entry: &str, selection: &[String]) -> bool {
    selection.iter().any(|sel| entry == sel || entry.strip_prefix(sel.as_str()).is_some_and(|rest| rest.starts_with('/')))
}

/// Whether `name` stays inside the destination folder: relative, and without
/// any `..` component.
pub fn is_safe(name: &str) -> bool {
    Path::new(name).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_selects_the_archives() {
        for n in ["a.zip", "A.ZIP", "b.tar.gz", "c.tgz", "d.7z", "e.tar.zst", "f.rar", "g.iso", "h.tar"] {
            assert!(is_archive_name(n), "{n}");
        }
        for n in ["a.rs", "zip", "a.gz", "a.zipx.txt", "tar"] {
            assert!(!is_archive_name(n), "{n}");
        }
    }

    #[test]
    fn normalize_drops_dot_slash_and_trailing_slash() {
        assert_eq!(normalize("./a/b/"), "a/b");
        assert_eq!(normalize("././a"), "a");
        assert_eq!(normalize("./"), "");
        assert_eq!(normalize("."), "");
        assert_eq!(normalize("a"), "a");
    }

    #[test]
    fn selection_is_exact_or_below_a_directory() {
        let sel = vec!["dir".to_string(), "x/y.txt".to_string()];
        assert!(is_selected("dir", &sel));
        assert!(is_selected("dir/sub/f", &sel));
        assert!(is_selected("x/y.txt", &sel));
        assert!(!is_selected("directory", &sel));
        assert!(!is_selected("x/y.txt2", &sel));
        assert!(!is_selected("st[ar]*.txt", &["st*".to_string()]));
    }

    #[test]
    fn the_stem_drops_the_longest_archive_suffix() {
        assert_eq!(archive_stem("photos.tar.gz"), "photos");
        assert_eq!(archive_stem("Photos.TAR.GZ"), "Photos");
        assert_eq!(archive_stem("a.b.zip"), "a.b");
        assert_eq!(archive_stem("città.7z"), "città");
        assert_eq!(archive_stem("notes.txt"), "notes.txt");
        assert_eq!(archive_stem(".zip"), ".zip");
    }

    /// Run `entries` through a [`RootCheck`].
    fn _root(entries: &[(&str, bool)]) -> bool {
        let mut check = RootCheck::default();
        for (name, is_dir) in entries {
            check.add(name, *is_dir);
        }
        check.has_root_folder()
    }

    #[test]
    fn one_top_folder_is_a_root_folder() {
        assert!(_root(&[("proj", true), ("proj/a.txt", false), ("proj/sub/b", false)]));
        // The folder itself may be missing from the archive.
        assert!(_root(&[("proj/a.txt", false), ("proj/b.txt", false)]));
        assert!(_root(&[]));
    }

    #[test]
    fn files_at_the_top_or_two_top_folders_are_not_a_root_folder() {
        assert!(!_root(&[("a.txt", false)]));
        assert!(!_root(&[("proj/a.txt", false), ("readme", false)]));
        assert!(!_root(&[("one/a", false), ("two/b", false)]));
        assert!(!_root(&[("one", true), ("two", true)]));
    }

    #[test]
    fn the_check_says_stop_as_soon_as_it_knows() {
        let mut check = RootCheck::default();
        assert!(check.add("p/a", false));
        assert!(!check.add("q/b", false));
        assert!(!check.add("p/c", false));
    }

    #[test]
    fn unsafe_names_are_refused() {
        assert!(is_safe("a/b.txt"));
        assert!(is_safe("./a"));
        assert!(!is_safe("../evil"));
        assert!(!is_safe("a/../../evil"));
        assert!(!is_safe("/etc/passwd"));
    }
}
