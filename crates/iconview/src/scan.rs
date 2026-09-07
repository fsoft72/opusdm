//! Read a directory into `IconEntry` values. Pure filesystem access, no GTK.

use crate::types::{IconEntry, IconKind};
use std::io;
use std::path::Path;

/// The per-directory layout file, always excluded from the listing.
pub const LAYOUT_FILE: &str = ".opusdm-layout";

/// Read `dir` into entries. `.` and `..` are skipped, so is `LAYOUT_FILE`, and
/// dotfiles unless `show_hidden`. A symlink's kind is `IconKind::Symlink` when
/// its target resolves and `IconKind::Broken` when it dangles.
pub fn read_entries(dir: &Path, show_hidden: bool) -> io::Result<Vec<IconEntry>> {
    use std::os::unix::fs::MetadataExt;

    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("opusdm-iconview: skipping unreadable entry in {}: {e}", dir.display());
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == LAYOUT_FILE {
            continue;
        }
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let (kind, mode) = if file_type.is_symlink() {
            match std::fs::metadata(entry.path()) {
                Ok(m) => (IconKind::Symlink, m.mode()),
                Err(_) => (IconKind::Broken, 0),
            }
        } else {
            match entry.metadata() {
                Ok(m) if m.is_dir() => (IconKind::Dir, m.mode()),
                Ok(m) => (IconKind::File, m.mode()),
                Err(_) => continue,
            }
        };
        let ext = if kind == IconKind::Dir { None } else { IconEntry::ext_of(&name) };
        out.push(IconEntry { name, path: entry.path(), kind, mode, ext });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reports_kinds_and_filters() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        fs::create_dir(base.join("adir")).unwrap();
        fs::write(base.join("afile.txt"), b"12345").unwrap();
        fs::write(base.join(".hidden"), b"x").unwrap();
        fs::write(base.join(LAYOUT_FILE), b"{}").unwrap();
        std::os::unix::fs::symlink("afile.txt", base.join("good_link")).unwrap();
        std::os::unix::fs::symlink("nope", base.join("bad_link")).unwrap();

        let mut got = read_entries(base, false).unwrap();
        got.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["adir", "afile.txt", "bad_link", "good_link"]);

        let by = |n: &str| got.iter().find(|e| e.name == n).unwrap().clone();
        assert_eq!(by("adir").kind, IconKind::Dir);
        assert_eq!(by("afile.txt").kind, IconKind::File);
        assert_eq!(by("afile.txt").ext.as_deref(), Some("txt"));
        assert_eq!(by("good_link").kind, IconKind::Symlink);
        assert_eq!(by("bad_link").kind, IconKind::Broken);

        let with_hidden = read_entries(base, true).unwrap();
        assert!(with_hidden.iter().any(|e| e.name == ".hidden"));
        assert!(!with_hidden.iter().any(|e| e.name == LAYOUT_FILE));
    }
}
