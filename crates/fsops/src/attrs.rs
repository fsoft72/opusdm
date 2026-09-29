//! Attribute jobs: permission bits (Chmod) and timestamps (Touch), with
//! optional recursion and a file-name filter.

use crate::cancel::CancelToken;
use crate::types::Progress;
use opusdm_ipc::glob::GlobSet;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use walkdir::WalkDir;

/// Permission bits Chmod is allowed to set.
const MODE_MASK: u32 = 0o777;

/// Read bits of the three classes; each one earns the matching `x` on directories.
const READ_BITS: [(u32, u32); 3] = [(0o400, 0o100), (0o040, 0o010), (0o004, 0o001)];

/// The mode to apply to a directory: `mode` plus `x` wherever `r` is set, so
/// a recursive 0644 keeps directories traversable.
pub fn dir_mode(mode: u32) -> u32 {
    READ_BITS.iter().fold(mode & MODE_MASK, |m, (r, x)| if m & r != 0 { m | x } else { m })
}

/// What kind of entry the walker is handing to the action.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    Symlink,
}

/// Visit every entry a job affects, children before their directory, and run
/// `action` on it. Returns the terminal `Done`.
///
/// - Without `recurse` only the entries of `srcs` are visited.
/// - With `glob` only files and symlinks whose name matches are visited;
///   directories are never visited.
fn walk_entries(
    srcs: &[PathBuf],
    recurse: bool,
    glob: Option<&GlobSet>,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
    action: &dyn Fn(&Path, Kind) -> io::Result<()>,
) -> Progress {
    let mut changed = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();
    for src in srcs {
        let depth = if recurse { usize::MAX } else { 0 };
        for entry in WalkDir::new(src).follow_links(false).max_depth(depth).contents_first(true) {
            if cancel.is_cancelled() {
                return Progress::Done { copied: changed, skipped: 0, cancelled: true, errors };
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    errors.push((src.clone(), e.to_string()));
                    continue;
                }
            };
            let ft = entry.file_type();
            let kind = if ft.is_symlink() {
                Kind::Symlink
            } else if ft.is_dir() {
                Kind::Dir
            } else {
                Kind::File
            };
            if let Some(g) = glob {
                if kind == Kind::Dir || !g.is_match(&entry.file_name().to_string_lossy()) {
                    continue;
                }
            }
            let _ = tx.send(Progress::File {
                path: entry.path().to_path_buf(),
                done_bytes: 0,
                total_bytes: 0,
                files_done: changed,
                files_total: 0,
                file_bytes_done: 0,
                file_bytes_total: 0,
            });
            match action(entry.path(), kind) {
                Ok(()) => changed += 1,
                Err(e) => errors.push((entry.path().to_path_buf(), e.to_string())),
            }
        }
    }
    Progress::Done { copied: changed, skipped: 0, cancelled: false, errors }
}

/// Set the permission bits of `srcs` to `mode` (see [`dir_mode`] for
/// directories). Symlinks are skipped: their own mode means nothing on Linux.
pub fn run_chmod(
    srcs: &[PathBuf],
    mode: u32,
    recurse: bool,
    glob: Option<&GlobSet>,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    walk_entries(srcs, recurse, glob, tx, cancel, &|path, kind| match kind {
        Kind::Symlink => Ok(()),
        Kind::Dir => std::fs::set_permissions(path, std::fs::Permissions::from_mode(dir_mode(mode))),
        Kind::File => std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & MODE_MASK)),
    })
}

/// Set the access and modification time of `srcs` to `unix_secs`. A symlink
/// gets the time itself, its target is untouched.
pub fn run_touch(
    srcs: &[PathBuf],
    unix_secs: i64,
    recurse: bool,
    glob: Option<&GlobSet>,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    let time = filetime::FileTime::from_unix_time(unix_secs, 0);
    walk_entries(srcs, recurse, glob, tx, cancel, &|path, kind| match kind {
        Kind::Symlink => filetime::set_symlink_file_times(path, time, time),
        Kind::Dir | Kind::File => filetime::set_file_times(path, time, time),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::channel;

    fn done(p: Progress) -> (u64, Vec<(PathBuf, String)>) {
        match p {
            Progress::Done { copied, errors, .. } => (copied, errors),
            _ => panic!("not Done"),
        }
    }

    fn mode_of(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn dir_mode_adds_x_where_r_is_set() {
        assert_eq!(dir_mode(0o644), 0o755);
        assert_eq!(dir_mode(0o600), 0o700);
        assert_eq!(dir_mode(0o000), 0o000);
        assert_eq!(dir_mode(0o4755), 0o755);
    }

    #[test]
    fn chmod_recursive_changes_files_and_keeps_dirs_traversable() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("r");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("sub/b.txt"), "b").unwrap();
        let (tx, _rx) = channel();

        let (n, errors) = done(run_chmod(&[root.clone()], 0o644, true, None, &tx, &CancelToken::new()));

        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(n, 4);
        assert_eq!(mode_of(&root.join("a.txt")), 0o644);
        assert_eq!(mode_of(&root.join("sub/b.txt")), 0o644);
        assert_eq!(mode_of(&root.join("sub")), 0o755);
        assert_eq!(mode_of(&root), 0o755);
    }

    #[test]
    fn chmod_with_filter_touches_only_matching_files() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("r");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("b.rs"), "b").unwrap();
        fs::set_permissions(root.join("b.rs"), fs::Permissions::from_mode(0o600)).unwrap();
        let (tx, _rx) = channel();
        let glob = GlobSet::parse("*.txt").unwrap();

        let (n, _) = done(run_chmod(&[root.clone()], 0o640, true, Some(&glob), &tx, &CancelToken::new()));

        assert_eq!(n, 1);
        assert_eq!(mode_of(&root.join("a.txt")), 0o640);
        assert_eq!(mode_of(&root.join("b.rs")), 0o600);
    }

    #[test]
    fn chmod_without_recursion_only_changes_the_selection() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("r");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::set_permissions(root.join("a.txt"), fs::Permissions::from_mode(0o600)).unwrap();
        let (tx, _rx) = channel();

        let (n, _) = done(run_chmod(&[root.clone()], 0o700, false, None, &tx, &CancelToken::new()));

        assert_eq!(n, 1);
        assert_eq!(mode_of(&root), 0o700);
        assert_eq!(mode_of(&root.join("a.txt")), 0o600);
    }

    #[test]
    fn chmod_skips_symlinks() {
        let td = tempfile::tempdir().unwrap();
        let target = td.path().join("t");
        fs::write(&target, "x").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let link = td.path().join("l");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (tx, _rx) = channel();

        let (_, errors) = done(run_chmod(&[link], 0o777, false, None, &tx, &CancelToken::new()));

        assert!(errors.is_empty());
        assert_eq!(mode_of(&target), 0o600, "the target must not change");
    }

    #[test]
    fn touch_sets_the_time_on_files_and_contents() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("r");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a"), "a").unwrap();
        fs::write(root.join("b"), "b").unwrap();
        let (tx, _rx) = channel();
        let when = 1_500_000_000;

        let (n, errors) = done(run_touch(&[root.clone()], when, true, None, &tx, &CancelToken::new()));

        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(n, 3);
        for p in [root.join("a"), root.join("b"), root.clone()] {
            let m = fs::metadata(&p).unwrap();
            assert_eq!(filetime::FileTime::from_last_modification_time(&m).unix_seconds(), when);
        }
    }

    #[test]
    fn touch_of_a_symlink_leaves_its_target_alone() {
        let td = tempfile::tempdir().unwrap();
        let target = td.path().join("t");
        fs::write(&target, "x").unwrap();
        let before = filetime::FileTime::from_last_modification_time(&fs::metadata(&target).unwrap());
        let link = td.path().join("l");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (tx, _rx) = channel();

        run_touch(&[link.clone()], 1_000_000_000, false, None, &tx, &CancelToken::new());

        let after = filetime::FileTime::from_last_modification_time(&fs::metadata(&target).unwrap());
        assert_eq!(before, after);
        let lm = fs::symlink_metadata(&link).unwrap();
        assert_eq!(filetime::FileTime::from_last_modification_time(&lm).unix_seconds(), 1_000_000_000);
    }
}
