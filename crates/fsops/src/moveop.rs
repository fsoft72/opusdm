//! Move: per-source `rename` fast-path, `EXDEV` fallback to copy-then-delete.

use crate::cancel::CancelToken;
use crate::copy::copy_tree;
use crate::types::Progress;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Linux `EXDEV`: "cross-device link". Raw OS error number.
const EXDEV: i32 = 18;

/// Move each source into `dest_dir`.
pub fn run_move(
    srcs: &[PathBuf],
    dest_dir: &Path,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    let mut cross_fs: Vec<PathBuf> = Vec::new();
    let mut moved = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();

    for src in srcs {
        if cancel.is_cancelled() {
            return Progress::Done { copied: moved, skipped: 0, cancelled: true, errors };
        }
        let Some(name) = src.file_name() else {
            errors.push((src.clone(), "source has no file name".into()));
            continue;
        };
        let target = dest_dir.join(name);
        match std::fs::rename(src, &target) {
            Ok(()) => moved += 1,
            Err(e) if e.raw_os_error() == Some(EXDEV) => cross_fs.push(src.clone()),
            Err(e) => errors.push((src.clone(), e.to_string())),
        }
    }

    if !cross_fs.is_empty() {
        // copy_tree emits its own Scanning/File events and deletes sources.
        match copy_tree(&cross_fs, dest_dir, tx, cancel, true) {
            Progress::Done { copied, skipped, cancelled, errors: mut e2 } => {
                errors.append(&mut e2);
                return Progress::Done {
                    copied: moved + copied,
                    skipped,
                    cancelled,
                    errors,
                };
            }
            other => return other,
        }
    }

    Progress::Done { copied: moved, skipped: 0, cancelled: false, errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::channel;
    use std::thread;

    fn drive(srcs: Vec<PathBuf>, dest: PathBuf) -> Progress {
        let (tx, rx) = channel::<Progress>();
        let h = thread::spawn(move || run_move(&srcs, &dest, &tx, &CancelToken::new()));
        while rx.recv().is_ok() {}
        h.join().unwrap()
    }

    #[test]
    fn same_dir_rename_is_used() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("a.txt");
        let destdir = td.path().join("dst");
        fs::write(&src, b"hi").unwrap();
        fs::create_dir_all(&destdir).unwrap();

        let done = drive(vec![src.clone()], destdir.clone());
        match done {
            Progress::Done { copied, errors, cancelled, .. } => {
                assert_eq!(copied, 1);
                assert!(errors.is_empty());
                assert!(!cancelled);
            }
            _ => panic!(),
        }
        assert!(!src.exists());
        assert_eq!(fs::read(destdir.join("a.txt")).unwrap(), b"hi");
    }

    #[test]
    fn moves_a_directory() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("d");
        let destdir = td.path().join("dst");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/x"), b"y").unwrap();
        fs::create_dir_all(&destdir).unwrap();

        drive(vec![src.clone()], destdir.clone());
        assert!(!src.exists());
        assert_eq!(fs::read(destdir.join("d/sub/x")).unwrap(), b"y");
    }
}
