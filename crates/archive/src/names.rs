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
    fn unsafe_names_are_refused() {
        assert!(is_safe("a/b.txt"));
        assert!(is_safe("./a"));
        assert!(!is_safe("../evil"));
        assert!(!is_safe("a/../../evil"));
        assert!(!is_safe("/etc/passwd"));
    }
}
