//! Input and progress-event types for [`crate::run_job`].

use opusdm_ipc::{FileMeta, Op, Resolution};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// One requested filesystem job.
pub struct JobSpec {
    /// What to do.
    pub op: Op,
    /// Absolute source paths (files or directories).
    pub srcs: Vec<PathBuf>,
    /// Absolute destination directory.
    pub dest_dir: PathBuf,
}

/// Events emitted by a running job, in order.
pub enum Progress {
    /// Emitted once after the scan pass completes.
    Scanning { files: u64, bytes: u64 },
    /// Emitted repeatedly during the work pass, throttled by the caller of `run_job`.
    File {
        path: PathBuf,
        done_bytes: u64,
        total_bytes: u64,
        files_done: u64,
        files_total: u64,
    },
    /// A destination entry exists. The worker blocks until `reply` is sent.
    Conflict {
        path: PathBuf,
        src: FileMeta,
        dst: FileMeta,
        reply: Sender<Resolution>,
    },
    /// Terminal event. Always emitted exactly once.
    Done {
        copied: u64,
        skipped: u64,
        cancelled: bool,
        errors: Vec<(PathBuf, String)>,
    },
}
