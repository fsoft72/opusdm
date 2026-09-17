//! Recursive pre-pass: enumerate every entry to copy/move and total up
//! file count and bytes, so the work pass can report progress.

use crate::errors::FsError;
use std::io;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// One entry to recreate at the destination. `rel` is relative to the
/// originating source's parent directory.
pub enum Item {
    /// A directory to create.
    Dir { rel: PathBuf },
    /// A regular file to copy, with its absolute source path and size.
    File { rel: PathBuf, abs: PathBuf, size: u64 },
    /// A symbolic link to recreate verbatim (target not followed).
    Symlink { rel: PathBuf, target: PathBuf },
}

/// The full result of scanning all sources.
pub struct Plan {
    /// Number of regular files (symlinks excluded).
    pub total_files: u64,
    /// Sum of regular-file sizes in bytes.
    pub total_bytes: u64,
    /// Every entry, parents before children.
    pub items: Vec<Item>,
}

/// Walk every path in `srcs` and build a [`Plan`]. Symlinks are not followed.
pub fn scan(srcs: &[PathBuf]) -> io::Result<Plan> {
    let mut plan = Plan { total_files: 0, total_bytes: 0, items: Vec::new() };
    for src in srcs {
        let base = src.parent().unwrap_or_else(|| Path::new("/"));
        // `sort_by_file_name` is purely cosmetic: it makes the per-directory
        // walk order (and so the `Progress::File` messages the work pass
        // emits from this plan) predictable and alphabetical instead of
        // whatever the OS returns, at the cost of buffering each directory's
        // entries before descending. Worth it at the sizes a file manager
        // copies; drop it if this is ever used on trees large enough for
        // that buffering to matter.
        for entry in WalkDir::new(src).follow_links(false).sort_by_file_name() {
            let entry = entry.map_err(io::Error::other)?;
            let rel = entry
                .path()
                .strip_prefix(base)
                .map_err(|_| io::Error::other(FsError::PathOutsideBase(entry.path().to_path_buf())))?
                .to_path_buf();
            let ft = entry.file_type();
            if ft.is_symlink() {
                let target = std::fs::read_link(entry.path())?;
                plan.items.push(Item::Symlink { rel, target });
            } else if ft.is_dir() {
                plan.items.push(Item::Dir { rel });
            } else {
                let size = entry.metadata().map_err(io::Error::other)?.len();
                plan.total_files += 1;
                plan.total_bytes += size;
                plan.items.push(Item::File { rel, abs: entry.path().to_path_buf(), size });
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_a_small_tree() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("proj");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"hello").unwrap();
        fs::write(root.join("sub/b.txt"), b"worldXX").unwrap();

        let plan = scan(&[root.clone()]).unwrap();

        assert_eq!(plan.total_files, 2);
        assert_eq!(plan.total_bytes, 12);
        let dirs: Vec<_> = plan
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Dir { rel } => Some(rel.clone()),
                _ => None,
            })
            .collect();
        assert!(dirs.contains(&PathBuf::from("proj")));
        assert!(dirs.contains(&PathBuf::from("proj/sub")));

        let files: Vec<_> = plan
            .items
            .iter()
            .filter_map(|i| match i {
                Item::File { rel, .. } => Some(rel.clone()),
                _ => None,
            })
            .collect();
        assert!(files.contains(&PathBuf::from("proj/a.txt")));
        assert!(files.contains(&PathBuf::from("proj/sub/b.txt")));
    }

    #[test]
    fn scans_a_single_file() {
        let td = tempfile::tempdir().unwrap();
        let f = td.path().join("only.bin");
        fs::write(&f, vec![0u8; 100]).unwrap();
        let plan = scan(&[f]).unwrap();
        assert_eq!(plan.total_files, 1);
        assert_eq!(plan.total_bytes, 100);
        assert!(matches!(&plan.items[0], Item::File { rel, .. } if rel == &PathBuf::from("only.bin")));
    }

    #[test]
    fn symlink_is_recorded_not_followed() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("d");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("target.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("target.txt", root.join("link.txt")).unwrap();

        let plan = scan(&[root]).unwrap();
        assert_eq!(plan.total_files, 1);
        assert_eq!(plan.total_bytes, 1);
        assert!(plan.items.iter().any(|i| matches!(i, Item::Symlink { rel, .. } if rel == &PathBuf::from("d/link.txt"))));
    }
}
