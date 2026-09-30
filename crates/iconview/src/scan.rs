//! Read a directory into `IconEntry` values. Pure filesystem access, no GTK.

use crate::types::{IconEntry, IconKind};
use std::io;
use std::path::Path;

/// The per-folder directory holding OpusDM state scoped to that folder
/// (icon layout today, room for more later). Always excluded from a listing.
pub const OPUSDM_DIR: &str = ".opusdm";

/// File name, inside [`OPUSDM_DIR`], that stores desktop icon positions.
pub const LAYOUT_BASENAME: &str = "layout";

/// Absolute path to `dir`'s icon-layout file: `<dir>/.opusdm/layout`. The
/// writer must `create_dir_all` the parent first.
pub fn layout_path(dir: &Path) -> std::path::PathBuf {
    dir.join(OPUSDM_DIR).join(LAYOUT_BASENAME)
}

/// Read `dir` into entries. `.` and `..` are skipped, so is the `.opusdm` dir, and
/// dotfiles unless `show_hidden`. A symlink's kind is `IconKind::Symlink` when
/// its target resolves and `IconKind::Broken` when it dangles.
///
/// The result is sorted for a tidy grid: directories first, then files, each
/// block by name case-insensitively (see [`sort_dirs_first`]).
pub fn read_entries(dir: &Path, show_hidden: bool) -> io::Result<Vec<IconEntry>> {
    use std::os::unix::fs::MetadataExt;

    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                opusdm_ipc::err_log!("iconview", "skipping unreadable entry in {}: {e}", dir.display());
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == OPUSDM_DIR {
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
        out.push(IconEntry { name, path: entry.path(), kind, mode, ext, left_out: false, group: false });
    }
    sort_dirs_first(&mut out);
    Ok(out)
}

/// Build the entry for one absolute `path`, wherever it lives. A path that
/// does not exist (or cannot be inspected) is `IconKind::Broken` with mode 0;
/// a symlink is `Symlink` when its target resolves and `Broken` when it
/// dangles. The name is the final path component. `left_out` is `false`.
pub fn entry_for_path(path: &Path) -> IconEntry {
    use std::os::unix::fs::MetadataExt;

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let (kind, mode) = match std::fs::symlink_metadata(path) {
        Err(_) => (IconKind::Broken, 0),
        Ok(m) if m.file_type().is_symlink() => match std::fs::metadata(path) {
            Ok(target) => (IconKind::Symlink, target.mode()),
            Err(_) => (IconKind::Broken, 0),
        },
        Ok(m) if m.is_dir() => (IconKind::Dir, m.mode()),
        Ok(m) => (IconKind::File, m.mode()),
    };
    let ext = if kind == IconKind::Dir { None } else { IconEntry::ext_of(&name) };
    IconEntry { name, path: path.to_path_buf(), kind, mode, ext, left_out: false, group: false }
}

/// Sort entries for a tidy grid: directories first, then files, each block by
/// name case-insensitively.
pub fn sort_dirs_first(entries: &mut [IconEntry]) {
    entries.sort_by(|a, b| {
        let a_dir = a.kind == IconKind::Dir;
        let b_dir = b.kind == IconKind::Dir;
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
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
        fs::create_dir(base.join(OPUSDM_DIR)).unwrap();
        fs::write(base.join(OPUSDM_DIR).join(LAYOUT_BASENAME), b"{}").unwrap();
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
        assert!(!with_hidden.iter().any(|e| e.name == OPUSDM_DIR));
    }

    #[test]
    fn read_entries_returns_dirs_first_then_files_by_name() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        for d in ["zeta", "Alpha"] {
            fs::create_dir(base.join(d)).unwrap();
        }
        for f in ["yankee.txt", "Bravo.md"] {
            fs::write(base.join(f), b"x").unwrap();
        }
        let got = read_entries(base, false).unwrap();
        let names: Vec<&str> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "zeta", "Bravo.md", "yankee.txt"]);
    }

    #[test]
    fn layout_path_lives_under_the_per_folder_opusdm_dir() {
        let p = layout_path(Path::new("/tmp/desk"));
        assert_eq!(p, Path::new("/tmp/desk/.opusdm/layout"));
    }

    #[test]
    fn opusdm_dir_is_hidden_even_with_show_hidden() {
        let td = tempfile::tempdir().unwrap();
        fs::create_dir(td.path().join(OPUSDM_DIR)).unwrap();
        fs::write(td.path().join(".other"), b"x").unwrap();
        let got = read_entries(td.path(), true).unwrap();
        assert!(got.iter().any(|e| e.name == ".other"));
        assert!(!got.iter().any(|e| e.name == OPUSDM_DIR));
    }

    #[test]
    fn sort_dirs_first_orders_dirs_then_files_by_name() {
        let mk = |name: &str, kind: IconKind| IconEntry {
            name: name.into(),
            path: Path::new("/x").join(name),
            kind,
            mode: 0,
            ext: None,
            left_out: false,
            group: false,
        };
        let mut v = vec![
            mk("Zebra", IconKind::File),
            mk("beta", IconKind::Dir),
            mk("Alpha", IconKind::File),
            mk("Acme", IconKind::Dir),
        ];
        sort_dirs_first(&mut v);
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Acme", "beta", "Alpha", "Zebra"]);
    }

    #[test]
    fn entry_for_path_reports_file_dir_symlink_and_missing() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        fs::write(base.join("a.txt"), b"x").unwrap();
        fs::create_dir(base.join("d")).unwrap();
        std::os::unix::fs::symlink("a.txt", base.join("good")).unwrap();
        std::os::unix::fs::symlink("nope", base.join("bad")).unwrap();

        let f = entry_for_path(&base.join("a.txt"));
        assert_eq!((f.name.as_str(), f.kind, f.ext.as_deref()), ("a.txt", IconKind::File, Some("txt")));
        assert!(!f.left_out);
        assert_eq!(entry_for_path(&base.join("d")).kind, IconKind::Dir);
        assert_eq!(entry_for_path(&base.join("good")).kind, IconKind::Symlink);
        assert_eq!(entry_for_path(&base.join("bad")).kind, IconKind::Broken);
        let missing = entry_for_path(&base.join("gone.png"));
        assert_eq!((missing.name.as_str(), missing.kind, missing.mode), ("gone.png", IconKind::Broken, 0));
    }
}
