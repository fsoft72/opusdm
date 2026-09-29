//! Duplicate: copy each source next to itself under a free `.copy` name.

use crate::cancel::CancelToken;
use crate::copy::copy_tree_opts;
use crate::types::{Progress, TransferOpts};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// The first free name among `name.copy`, `name.copy2`, `name.copy3`, ...
/// in `dir`, skipping names already in `reserved`.
pub fn free_copy_name(dir: &Path, name: &str, reserved: &HashSet<PathBuf>) -> String {
    let mut n = 1u64;
    loop {
        let candidate = if n == 1 { format!("{name}.copy") } else { format!("{name}.copy{n}") };
        let path = dir.join(&candidate);
        if std::fs::symlink_metadata(&path).is_err() && !reserved.contains(&path) {
            return candidate;
        }
        n += 1;
    }
}

/// Duplicate every source in its own parent directory. Only the glob of
/// `opts` applies; `update`/`newer` make no sense for a fresh name.
pub fn run_duplicate(
    srcs: &[PathBuf],
    opts: &TransferOpts,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    let mut copied = 0u64;
    let mut skipped = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();
    let mut reserved: HashSet<PathBuf> = HashSet::new();
    let one_opts = |name: String| TransferOpts {
        glob: opts.glob.clone(),
        names: Some(vec![name]),
        ..TransferOpts::default()
    };

    for src in srcs {
        let (Some(parent), Some(own_name)) = (src.parent(), src.file_name()) else {
            errors.push((src.clone(), "source has no parent or name".into()));
            continue;
        };
        let new_name = free_copy_name(parent, &own_name.to_string_lossy(), &reserved);
        reserved.insert(parent.join(&new_name));
        match copy_tree_opts(&[src.clone()], parent, tx, cancel, false, &one_opts(new_name)) {
            Progress::Done { copied: c, skipped: s, cancelled, errors: mut e } => {
                copied += c;
                skipped += s;
                errors.append(&mut e);
                if cancelled {
                    return Progress::Done { copied, skipped, cancelled: true, errors };
                }
            }
            _ => unreachable!("copy_tree_opts always ends with Done"),
        }
    }
    Progress::Done { copied, skipped, cancelled: false, errors }
}
