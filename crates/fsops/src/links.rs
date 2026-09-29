//! MakeLink / MakeLinkAs: hard and symbolic links in a destination directory.

use crate::cancel::CancelToken;
use crate::types::Progress;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Create one link per source in `dest_dir`. `names[i]`, when given, is the
/// name of the link to source `i`; otherwise the source's own name is used.
///
/// A symbolic link stores the absolute source path. Hard links to
/// directories are refused. An existing target is an error for that entry
/// and is never overwritten. Failures are collected, the rest still run.
pub fn run_link(
    srcs: &[PathBuf],
    dest_dir: &Path,
    symbolic: bool,
    names: Option<&[String]>,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    let mut made = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();
    for (idx, src) in srcs.iter().enumerate() {
        if cancel.is_cancelled() {
            return Progress::Done { copied: made, skipped: 0, cancelled: true, errors };
        }
        let name = match names.and_then(|n| n.get(idx)) {
            Some(n) => n.clone(),
            None => match src.file_name() {
                Some(n) => n.to_string_lossy().into_owned(),
                None => {
                    errors.push((src.clone(), "source has no file name".into()));
                    continue;
                }
            },
        };
        let link = dest_dir.join(&name);
        let _ = tx.send(Progress::File {
            path: link.clone(),
            done_bytes: 0,
            total_bytes: 0,
            files_done: made,
            files_total: srcs.len() as u64,
            file_bytes_done: 0,
            file_bytes_total: 0,
        });
        match make_link(src, &link, symbolic) {
            Ok(()) => made += 1,
            Err(msg) => errors.push((link, msg)),
        }
    }
    Progress::Done { copied: made, skipped: 0, cancelled: false, errors }
}

/// Create the single link `link` to `src`, with a readable error.
fn make_link(src: &Path, link: &Path, symbolic: bool) -> Result<(), String> {
    if std::fs::symlink_metadata(link).is_ok() {
        return Err(format!("{} already exists", link.display()));
    }
    let result = if symbolic {
        let target = std::path::absolute(src).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(target, link)
    } else {
        let meta = std::fs::symlink_metadata(src).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            return Err(format!("{} is a directory: hard links to directories are not allowed", src.display()));
        }
        std::fs::hard_link(src, link)
    };
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::sync::mpsc::channel;

    fn run(srcs: &[PathBuf], dest: &Path, symbolic: bool, names: Option<&[String]>) -> (u64, Vec<(PathBuf, String)>) {
        let (tx, _rx) = channel();
        match run_link(srcs, dest, symbolic, names, &tx, &CancelToken::new()) {
            Progress::Done { copied, errors, .. } => (copied, errors),
            _ => panic!("not Done"),
        }
    }

    #[test]
    fn symbolic_link_points_at_the_source() {
        let td = tempfile::tempdir().unwrap();
        let (src, dst) = (td.path().join("s"), td.path().join("d"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("f.txt"), "x").unwrap();

        let (n, errors) = run(&[src.join("f.txt")], &dst, true, None);

        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(n, 1);
        assert_eq!(fs::read_link(dst.join("f.txt")).unwrap(), src.join("f.txt"));
        assert_eq!(fs::read_to_string(dst.join("f.txt")).unwrap(), "x");
    }

    #[test]
    fn hard_link_shares_the_inode() {
        let td = tempfile::tempdir().unwrap();
        let (src, dst) = (td.path().join("s"), td.path().join("d"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("f.txt"), "x").unwrap();

        let (n, errors) = run(&[src.join("f.txt")], &dst, false, Some(&["g.txt".to_string()]));

        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(n, 1);
        let (a, b) = (fs::metadata(src.join("f.txt")).unwrap(), fs::metadata(dst.join("g.txt")).unwrap());
        assert_eq!(a.ino(), b.ino());
    }

    #[test]
    fn refuses_directories_and_existing_targets_without_stopping() {
        let td = tempfile::tempdir().unwrap();
        let (src, dst) = (td.path().join("s"), td.path().join("d"));
        fs::create_dir_all(src.join("dir")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("f.txt"), "new").unwrap();
        fs::write(src.join("g.txt"), "g").unwrap();
        fs::write(dst.join("f.txt"), "old").unwrap();

        let (n, errors) = run(&[src.join("dir"), src.join("f.txt"), src.join("g.txt")], &dst, false, None);

        assert_eq!(n, 1, "only g.txt can be linked");
        assert_eq!(errors.len(), 2);
        assert_eq!(fs::read_to_string(dst.join("f.txt")).unwrap(), "old");
        assert!(dst.join("g.txt").exists());
    }

    #[test]
    fn symbolic_link_to_a_directory_is_allowed() {
        let td = tempfile::tempdir().unwrap();
        let (src, dst) = (td.path().join("s"), td.path().join("d"));
        fs::create_dir_all(src.join("dir")).unwrap();
        fs::create_dir_all(&dst).unwrap();

        let (n, errors) = run(&[src.join("dir")], &dst, true, None);

        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(n, 1);
        assert!(dst.join("dir").is_dir());
    }
}
