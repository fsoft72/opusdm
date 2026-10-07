//! Recursive pre-pass: enumerate every entry to copy/move and total up
//! file count and bytes, so the work pass can report progress.

use crate::errors::FsError;
use opusdm_ipc::glob::GlobSet;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// One entry to recreate at the destination. `rel` is relative to the
/// originating source's parent directory.
pub enum Item {
    /// A directory to create, with its absolute source path.
    Dir { rel: PathBuf, abs: PathBuf },
    /// A regular file to copy, with its absolute source path and size.
    File { rel: PathBuf, abs: PathBuf, size: u64 },
    /// A symbolic link to recreate verbatim (target not followed), with its
    /// absolute source path.
    Symlink { rel: PathBuf, abs: PathBuf, target: PathBuf },
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
    scan_with(srcs, None, None)
}

/// [`scan`] with a file-name filter and per-source renames.
///
/// - `glob`: only regular files and symlinks whose name matches are kept.
///   Directories are always walked, but one that ends up with no kept entry
///   is left out of the plan.
/// - `names`: source `i` is planned under `names[i]` instead of its own name.
pub fn scan_with(
    srcs: &[PathBuf],
    glob: Option<&GlobSet>,
    names: Option<&[String]>,
) -> io::Result<Plan> {
    let mut plan = Plan { total_files: 0, total_bytes: 0, items: Vec::new() };
    for (idx, src) in srcs.iter().enumerate() {
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
            let plain_rel = entry
                .path()
                .strip_prefix(base)
                .map_err(|_| io::Error::other(FsError::PathOutsideBase(entry.path().to_path_buf())))?
                .to_path_buf();
            let rel = match names.and_then(|n| n.get(idx)) {
                Some(new_name) => renamed_rel(&plain_rel, new_name),
                None => plain_rel,
            };
            let ft = entry.file_type();
            if ft.is_dir() {
                plan.items.push(Item::Dir { rel, abs: entry.path().to_path_buf() });
                continue;
            }
            if let Some(g) = glob {
                if !g.is_match(&entry.file_name().to_string_lossy()) {
                    continue;
                }
            }
            if ft.is_symlink() {
                let target = std::fs::read_link(entry.path())?;
                plan.items.push(Item::Symlink { rel, abs: entry.path().to_path_buf(), target });
            } else {
                let size = entry.metadata().map_err(io::Error::other)?.len();
                plan.total_files += 1;
                plan.total_bytes += size;
                plan.items.push(Item::File { rel, abs: entry.path().to_path_buf(), size });
            }
        }
    }
    if glob.is_some() {
        drop_empty_dirs(&mut plan);
    }
    Ok(plan)
}

/// Replace the first component of `rel` (the source's own name) with `new_name`.
fn renamed_rel(rel: &Path, new_name: &str) -> PathBuf {
    let mut parts = rel.components();
    parts.next();
    let rest = parts.as_path();
    if rest.as_os_str().is_empty() {
        return PathBuf::from(new_name);
    }
    PathBuf::from(new_name).join(rest)
}

/// Remove every `Dir` that is not an ancestor of a kept file or symlink.
fn drop_empty_dirs(plan: &mut Plan) {
    let mut needed: HashSet<PathBuf> = HashSet::new();
    for item in &plan.items {
        let rel = match item {
            Item::File { rel, .. } | Item::Symlink { rel, .. } => rel,
            Item::Dir { .. } => continue,
        };
        needed.extend(rel.ancestors().skip(1).map(Path::to_path_buf));
    }
    plan.items.retain(|i| match i {
        Item::Dir { rel, .. } => needed.contains(rel),
        _ => true,
    });
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

        let plan = scan(std::slice::from_ref(&root)).unwrap();

        assert_eq!(plan.total_files, 2);
        assert_eq!(plan.total_bytes, 12);
        let dirs: Vec<_> = plan
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Dir { rel, .. } => Some(rel.clone()),
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

    #[test]
    fn glob_keeps_matching_files_and_only_needed_dirs() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("p");
        fs::create_dir_all(root.join("has")).unwrap();
        fs::create_dir_all(root.join("none")).unwrap();
        fs::write(root.join("has/a.rs"), b"1").unwrap();
        fs::write(root.join("has/b.txt"), b"2").unwrap();
        fs::write(root.join("none/c.txt"), b"3").unwrap();

        let glob = GlobSet::parse("*.rs").unwrap();
        let plan = scan_with(&[root], Some(&glob), None).unwrap();

        assert_eq!(plan.total_files, 1);
        let dirs: Vec<_> = plan
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Dir { rel, .. } => Some(rel.clone()),
                _ => None,
            })
            .collect();
        assert!(dirs.contains(&PathBuf::from("p")));
        assert!(dirs.contains(&PathBuf::from("p/has")));
        assert!(!dirs.contains(&PathBuf::from("p/none")));
    }

    #[test]
    fn names_rename_the_top_level_entry() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("old");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("f"), b"x").unwrap();
        let file = td.path().join("g.txt");
        fs::write(&file, b"y").unwrap();

        let names = vec!["new".to_string(), "h.txt".to_string()];
        let plan = scan_with(&[root, file], None, Some(&names)).unwrap();

        let rels: Vec<PathBuf> = plan
            .items
            .iter()
            .map(|i| match i {
                Item::Dir { rel, .. } | Item::File { rel, .. } | Item::Symlink { rel, .. } => rel.clone(),
            })
            .collect();
        assert_eq!(rels, vec![PathBuf::from("new"), PathBuf::from("new/f"), PathBuf::from("h.txt")]);
    }
}
