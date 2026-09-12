//! Permanent recursive delete (no trash), and single-entry creation.

use crate::cancel::CancelToken;
use crate::types::Progress;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Recursively delete every path in `srcs`. Read-only files are retried once
/// after adding write permission. Failures are collected, not fatal.
pub fn run_delete(srcs: &[PathBuf], tx: &Sender<Progress>, cancel: &CancelToken) -> Progress {
    let mut removed = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();

    for src in srcs {
        if cancel.is_cancelled() {
            return Progress::Done { copied: removed, skipped: 0, cancelled: true, errors };
        }
        let _ = tx.send(Progress::File {
            path: src.clone(),
            done_bytes: 0,
            total_bytes: 0,
            files_done: removed,
            files_total: srcs.len() as u64,
            file_bytes_done: 0,
            file_bytes_total: 0,
        });
        match remove_recursive(src) {
            Ok(()) => removed += 1,
            Err(e) => errors.push((src.clone(), e.to_string())),
        }
    }

    Progress::Done { copied: removed, skipped: 0, cancelled: false, errors }
}

/// Remove a file or directory tree, chmod-retrying read-only entries once.
fn remove_recursive(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            remove_recursive(&entry?.path())?;
        }
        return std::fs::remove_dir(path);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let mut perm = meta.permissions();
            perm.set_mode(perm.mode() | 0o200);
            std::fs::set_permissions(path, perm)?;
            std::fs::remove_file(path)
        }
    }
}

/// Create one empty file (`file == true`) or one directory under `dest_dir`.
pub fn make_entry(dest_dir: &Path, name: &str, file: bool) -> Progress {
    let target = dest_dir.join(name);
    let result = if file {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map(|_| ())
            .map_err(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    format!("{} already exists", target.display())
                } else {
                    e.to_string()
                }
            })
    } else {
        std::fs::create_dir(&target).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                format!("{} already exists", target.display())
            } else {
                e.to_string()
            }
        })
    };
    match result {
        Ok(()) => Progress::Done { copied: 1, skipped: 0, cancelled: false, errors: Vec::new() },
        Err(msg) => Progress::Done {
            copied: 0,
            skipped: 0,
            cancelled: false,
            errors: vec![(target, msg)],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::channel;

    fn drive_delete(srcs: Vec<PathBuf>) -> Progress {
        let (tx, rx) = channel::<Progress>();
        let h = std::thread::spawn(move || run_delete(&srcs, &tx, &CancelToken::new()));
        while rx.recv().is_ok() {}
        h.join().unwrap()
    }

    #[test]
    fn deletes_tree_including_readonly_file() {
        let td = tempfile::tempdir().unwrap();
        let d = td.path().join("victim");
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("sub/ro"), b"x").unwrap();
        fs::set_permissions(d.join("sub/ro"), fs::Permissions::from_mode(0o400)).unwrap();

        let done = drive_delete(vec![d.clone()]);
        match done {
            Progress::Done { errors, .. } => assert!(errors.is_empty(), "{errors:?}"),
            _ => panic!(),
        }
        assert!(!d.exists());
    }

    #[test]
    fn make_entry_creates_dir_and_file() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path().to_path_buf();
        assert!(matches!(make_entry(&base, "newdir", false), Progress::Done { errors, .. } if errors.is_empty()));
        assert!(base.join("newdir").is_dir());
        assert!(matches!(make_entry(&base, "newfile", true), Progress::Done { errors, .. } if errors.is_empty()));
        assert!(base.join("newfile").is_file());
    }

    #[test]
    fn make_entry_rejects_existing() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path().to_path_buf();
        fs::write(base.join("dup"), b"").unwrap();
        assert!(matches!(make_entry(&base, "dup", true), Progress::Done { errors, .. } if !errors.is_empty()));
    }
}
