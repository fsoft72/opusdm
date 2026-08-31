//! Delete and single-entry creation. Fully populated in Task 5.

use crate::cancel::CancelToken;
use crate::types::Progress;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Placeholder replaced in Task 5.
pub fn run_delete(_srcs: &[PathBuf], _tx: &Sender<Progress>, _cancel: &CancelToken) -> Progress {
    Progress::Done { copied: 0, skipped: 0, cancelled: false, errors: Vec::new() }
}

/// Placeholder replaced in Task 5.
pub fn make_entry(dest_dir: &Path, _name: &str, _file: bool) -> Progress {
    Progress::Done {
        copied: 0,
        skipped: 0,
        cancelled: false,
        errors: vec![(dest_dir.to_path_buf(), "make_entry not implemented".into())],
    }
}
