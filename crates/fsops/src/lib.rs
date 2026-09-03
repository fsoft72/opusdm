//! Blocking filesystem operations (copy, move, delete, mkdir) with progress
//! events and overwrite-conflict resolution. No GTK, no sockets.

pub mod cancel;
pub mod copy;
pub mod delete;
pub mod errors;
pub mod moveop;
pub mod pool;
pub mod scan;
pub mod types;

pub use cancel::CancelToken;
pub use errors::FsError;
pub use types::{JobSpec, Progress};

use opusdm_ipc::{FileMeta, Op};
use std::sync::mpsc::Sender;
use std::time::UNIX_EPOCH;

/// Convert `std::fs::Metadata` into the wire `FileMeta` (seconds since epoch).
pub fn file_meta(m: &std::fs::Metadata) -> FileMeta {
    let modified_unix = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    FileMeta { size: m.len(), modified_unix }
}

/// Run a job to completion, emitting `Progress` events. Always finishes by
/// sending exactly one `Progress::Done`.
pub fn run_job(spec: JobSpec, tx: Sender<Progress>, cancel: CancelToken) {
    let done = match &spec.op {
        Op::Copy => copy::copy_tree(&spec.srcs, &spec.dest_dir, &tx, &cancel, false),
        Op::Move => moveop::run_move(&spec.srcs, &spec.dest_dir, &tx, &cancel),
        Op::Delete => delete::run_delete(&spec.srcs, &tx, &cancel),
        Op::MakeDir { name } => delete::make_entry(&spec.dest_dir, name, false),
        Op::NewFile { name } => delete::make_entry(&spec.dest_dir, name, true),
    };
    let _ = tx.send(done);
}
