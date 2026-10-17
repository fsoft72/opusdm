//! Permanent recursive delete (no trash), and single-entry creation.

use crate::cancel::CancelToken;
use crate::types::Progress;
use opusdm_ipc::glob::GlobSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use walkdir::WalkDir;

/// Recursively delete every path in `srcs`. Failures are collected, not
/// fatal. `cancel` is honoured inside a tree too, so one huge folder can be
/// stopped.
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
        let tree_errors = remove_recursive(src, cancel);
        if cancel.is_cancelled() {
            errors.extend(tree_errors);
            return Progress::Done { copied: removed, skipped: 0, cancelled: true, errors };
        }
        if tree_errors.is_empty() {
            removed += 1;
        } else {
            errors.extend(tree_errors);
        }
    }

    Progress::Done { copied: removed, skipped: 0, cancelled: false, errors }
}

/// Delete only the files and symlinks under `srcs` whose name matches
/// `glob`. Directories are walked but never removed. Failures are collected.
pub fn run_delete_filtered(
    srcs: &[PathBuf],
    glob: &GlobSet,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    let mut removed = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();

    for src in srcs {
        for entry in WalkDir::new(src).follow_links(false).contents_first(true) {
            if cancel.is_cancelled() {
                return Progress::Done { copied: removed, skipped: 0, cancelled: true, errors };
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    errors.push((src.clone(), e.to_string()));
                    continue;
                }
            };
            if entry.file_type().is_dir() || !glob.is_match(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let _ = tx.send(Progress::File {
                path: entry.path().to_path_buf(),
                done_bytes: 0,
                total_bytes: 0,
                files_done: removed,
                files_total: 0,
                file_bytes_done: 0,
                file_bytes_total: 0,
            });
            match std::fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(e) => errors.push((entry.path().to_path_buf(), e.to_string())),
            }
        }
    }

    Progress::Done { copied: removed, skipped: 0, cancelled: false, errors }
}

/// One step of the explicit-stack walk in `remove_recursive`: either visit a
/// path (deciding whether it's a file to remove or a directory to descend
/// into) or remove a directory once every entry `Visit` pushed for it has
/// already been popped and handled (LIFO order puts a directory's children
/// on top of its own `RemoveDir`, so they are always popped first).
enum Step {
    Visit(PathBuf),
    RemoveDir(PathBuf),
}

/// Remove a file or directory tree. One unreadable subdirectory or one file
/// that resists deletion does not abort the rest of the tree: every failure
/// is collected and the walk continues. Permissions are never changed: the
/// parent folder decides whether an entry can be removed, and a chmod would
/// follow a symlink to a file outside the tree. The walk stops as soon as
/// `cancel` is set, leaving the rest in place. An explicit stack replaces
/// call-stack recursion so a very deep tree can't overflow the worker
/// thread's stack.
pub(crate) fn remove_recursive(root: &Path, cancel: &CancelToken) -> Vec<(PathBuf, String)> {
    let mut errors = Vec::new();
    let mut stack = vec![Step::Visit(root.to_path_buf())];

    while let Some(step) = stack.pop() {
        if cancel.is_cancelled() { break; }
        match step {
            Step::Visit(path) => {
                let meta = match std::fs::symlink_metadata(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        errors.push((path, e.to_string()));
                        continue;
                    }
                };
                if meta.is_dir() {
                    let entries = match std::fs::read_dir(&path) {
                        Ok(e) => e,
                        Err(e) => {
                            errors.push((path, e.to_string()));
                            continue;
                        }
                    };
                    stack.push(Step::RemoveDir(path.clone()));
                    for entry in entries {
                        match entry {
                            Ok(e) => stack.push(Step::Visit(e.path())),
                            Err(e) => errors.push((path.clone(), e.to_string())),
                        }
                    }
                } else if let Err(e) = std::fs::remove_file(&path) {
                    errors.push((path, e.to_string()));
                }
            }
            Step::RemoveDir(path) => {
                if let Err(e) = std::fs::remove_dir(&path) {
                    errors.push((path, e.to_string()));
                }
            }
        }
    }
    errors
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

    /// One unreadable subdirectory must not abort deletion of the rest of
    /// the tree: siblings still get removed and the failure is reported,
    /// not silently dropped. (Trivially passes when running as root, which
    /// bypasses permissions.)
    #[test]
    #[cfg(unix)]
    fn one_unreadable_subdir_does_not_abort_the_rest_of_the_tree() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("victim");
        let locked = root.join("locked");
        let sibling = root.join("sibling");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("inner.txt"), b"x").unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("ok.txt"), b"y").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let done = drive_delete(vec![root.clone()]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        if !nix_is_root() {
            match done {
                Progress::Done { errors, .. } => assert!(!errors.is_empty(), "expected the locked entry to be reported"),
                _ => panic!(),
            }
            assert!(!sibling.exists(), "the readable sibling should still be removed");
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// A deeply nested tree must still delete cleanly with an explicit
    /// stack instead of call-stack recursion (bounded well under `PATH_MAX`,
    /// which a real overflow-sized tree would hit before the stack would).
    #[test]
    fn deeply_nested_tree_deletes_cleanly() {
        let td = tempfile::tempdir().unwrap();
        let mut path = td.path().to_path_buf();
        const DEPTH: usize = 1500;
        for _ in 0..DEPTH {
            path.push("d");
        }
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("leaf.txt"), b"x").unwrap();

        let root = td.path().join("d");
        let done = drive_delete(vec![root.clone()]);
        match done {
            Progress::Done { errors, .. } => assert!(errors.is_empty(), "{errors:?}"),
            _ => panic!(),
        }
        assert!(!root.exists());
    }

    /// A link that cannot be removed (its folder is read-only) must not get
    /// its target, outside the tree, made writable.
    #[test]
    fn a_failed_delete_never_changes_a_link_target() {
        let td = tempfile::tempdir().unwrap();
        let outside = td.path().join("outside.txt");
        fs::write(&outside, b"x").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o400)).unwrap();
        let root = td.path().join("victim");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();

        let done = drive_delete(vec![root.clone()]);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();

        if !nix_is_root() {
            assert!(matches!(done, Progress::Done { errors, .. } if !errors.is_empty()));
        }
        assert_eq!(fs::metadata(&outside).unwrap().permissions().mode() & 0o777, 0o400);
    }

    /// A cancelled token stops the walk inside a tree and leaves it alone.
    #[test]
    fn cancel_stops_inside_a_tree() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("big");
        fs::create_dir_all(root.join("sub")).unwrap();
        for i in 0..20 {
            fs::write(root.join("sub").join(format!("f{i}")), b"x").unwrap();
        }
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(remove_recursive(&root, &cancel).is_empty());
        assert_eq!(fs::read_dir(root.join("sub")).unwrap().count(), 20);

        let (tx, _rx) = channel::<Progress>();
        let done = run_delete(std::slice::from_ref(&root), &tx, &cancel);
        assert!(matches!(done, Progress::Done { cancelled: true, copied: 0, .. }));
        assert!(root.join("sub/f0").exists());
    }

    #[cfg(unix)]
    fn nix_is_root() -> bool {
        std::env::var_os("USER").is_some_and(|u| u == "root")
            || std::process::Command::new("id").arg("-u")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .is_some_and(|s| s.trim() == "0")
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
