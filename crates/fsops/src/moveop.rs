//! Move: per-source `rename` fast-path, `EXDEV` fallback to copy-then-delete.

use crate::cancel::CancelToken;
use crate::copy::copy_tree;
use crate::file_meta;
use crate::types::Progress;
use opusdm_ipc::Resolution;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

/// Emit a `Conflict` event and block for the reply. `None` if the channel closed.
fn ask_conflict(tx: &Sender<Progress>, dst: &Path, src: &Path) -> Option<Resolution> {
    let (reply_tx, reply_rx) = channel::<Resolution>();
    let src_meta = std::fs::metadata(src).ok().map(|m| file_meta(&m))?;
    let dst_meta = std::fs::metadata(dst).ok().map(|m| file_meta(&m))?;
    tx.send(Progress::Conflict { path: dst.to_path_buf(), src: src_meta, dst: dst_meta, reply: reply_tx })
        .ok()?;
    reply_rx.recv().ok()
}

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
    let mut overwrite_all = false;
    let mut skip_all = false;

    for src in srcs {
        if cancel.is_cancelled() {
            return Progress::Done { copied: moved, skipped: 0, cancelled: true, errors };
        }
        let Some(name) = src.file_name() else {
            errors.push((src.clone(), "source has no file name".into()));
            continue;
        };
        let target = dest_dir.join(name);

        // The rename(2) fast-path silently replaces an existing destination.
        // Ask before clobbering, same as the copy path does.
        if std::fs::symlink_metadata(&target).is_ok() && !overwrite_all {
            if skip_all {
                continue;
            }
            match ask_conflict(tx, &target, src) {
                Some(Resolution::Overwrite) => {}
                Some(Resolution::OverwriteAll) => overwrite_all = true,
                Some(Resolution::Skip) => continue,
                Some(Resolution::SkipAll) => {
                    skip_all = true;
                    continue;
                }
                Some(Resolution::Cancel) | None => {
                    return Progress::Done { copied: moved, skipped: 0, cancelled: true, errors };
                }
            }
        }

        match std::fs::rename(src, &target) {
            Ok(()) => moved += 1,
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => cross_fs.push(src.clone()),
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
        drive_with(srcs, dest, Resolution::Overwrite)
    }

    /// Drive one `run_move`, answering every `Conflict` with `resolution`.
    fn drive_with(srcs: Vec<PathBuf>, dest: PathBuf, resolution: Resolution) -> Progress {
        let (tx, rx) = channel::<Progress>();
        let h = thread::spawn(move || run_move(&srcs, &dest, &tx, &CancelToken::new()));
        while let Ok(ev) = rx.recv() {
            if let Progress::Conflict { reply, .. } = ev {
                let _ = reply.send(resolution);
            }
        }
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

    #[test]
    fn same_fs_rename_prompts_and_overwrites_on_overwrite() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("a.txt");
        let destdir = td.path().join("dst");
        fs::write(&src, b"new").unwrap();
        fs::create_dir_all(&destdir).unwrap();
        fs::write(destdir.join("a.txt"), b"old").unwrap();

        let done = drive_with(vec![src.clone()], destdir.clone(), Resolution::Overwrite);
        match done {
            Progress::Done { copied, cancelled, errors, .. } => {
                assert_eq!(copied, 1);
                assert!(!cancelled);
                assert!(errors.is_empty());
            }
            _ => panic!(),
        }
        assert!(!src.exists());
        assert_eq!(fs::read(destdir.join("a.txt")).unwrap(), b"new");
    }

    #[test]
    fn same_fs_rename_prompts_and_keeps_destination_on_skip() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("a.txt");
        let destdir = td.path().join("dst");
        fs::write(&src, b"new").unwrap();
        fs::create_dir_all(&destdir).unwrap();
        fs::write(destdir.join("a.txt"), b"old").unwrap();

        let done = drive_with(vec![src.clone()], destdir.clone(), Resolution::Skip);
        match done {
            Progress::Done { copied, cancelled, errors, .. } => {
                assert_eq!(copied, 0);
                assert!(!cancelled);
                assert!(errors.is_empty());
            }
            _ => panic!(),
        }
        assert!(src.exists());
        assert_eq!(fs::read(destdir.join("a.txt")).unwrap(), b"old");
    }

    #[test]
    fn same_fs_rename_cancel_stops_the_job() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("a.txt");
        let destdir = td.path().join("dst");
        fs::write(&src, b"new").unwrap();
        fs::create_dir_all(&destdir).unwrap();
        fs::write(destdir.join("a.txt"), b"old").unwrap();

        let done = drive_with(vec![src.clone()], destdir.clone(), Resolution::Cancel);
        match done {
            Progress::Done { cancelled, .. } => assert!(cancelled),
            _ => panic!(),
        }
        assert!(src.exists());
        assert_eq!(fs::read(destdir.join("a.txt")).unwrap(), b"old");
    }
}
