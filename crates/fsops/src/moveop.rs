//! Move implementation. Fully populated in Task 5.

use crate::cancel::CancelToken;
use crate::types::Progress;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Placeholder replaced in Task 5.
pub fn run_move(
    _srcs: &[PathBuf],
    dest_dir: &Path,
    _tx: &Sender<Progress>,
    _cancel: &CancelToken,
) -> Progress {
    Progress::Done {
        copied: 0,
        skipped: 0,
        cancelled: false,
        errors: vec![(dest_dir.to_path_buf(), "move not implemented".into())],
    }
}
