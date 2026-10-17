//! Input and progress-event types for [`crate::run_job`].

use opusdm_ipc::glob::GlobSet;
use opusdm_ipc::{FileMeta, JobOptions, Op, Resolution};
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
    /// Filter and flags; everything off by default.
    pub options: JobOptions,
}

/// The parsed, ready-to-apply form of [`JobOptions`] for one transfer.
#[derive(Debug, Clone, Default)]
pub struct TransferOpts {
    /// Only transfer files whose name already exists at the destination.
    pub update: bool,
    /// Only transfer files missing at the destination or newer than it.
    pub newer: bool,
    /// Only transfer files whose name matches.
    pub glob: Option<GlobSet>,
    /// Source `i` lands under `names[i]` instead of its own name.
    pub names: Option<Vec<String>>,
}

impl TransferOpts {
    /// Parse `options`, failing on an invalid glob before any file is touched.
    pub fn from_options(options: &JobOptions, names: Option<Vec<String>>) -> Result<Self, String> {
        let glob = match &options.include_glob {
            Some(text) => Some(GlobSet::parse(text)?),
            None => None,
        };
        Ok(Self { update: options.update, newer: options.newer, glob, names })
    }

    /// Whether the options alone may leave some source files untouched, so a
    /// Move cannot rename whole trees and must go through the copy engine
    /// (which always deletes only what it copied).
    pub fn is_partial(&self) -> bool {
        self.update || self.newer || self.glob.is_some()
    }
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
        /// Bytes written so far for `path` alone. `0` when not tracked (e.g. delete).
        file_bytes_done: u64,
        /// Size of `path` alone. `0` when not tracked (e.g. delete).
        file_bytes_total: u64,
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
