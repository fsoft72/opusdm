//! Blocking filesystem operations (copy, move, delete, mkdir) with progress
//! events and overwrite-conflict resolution. No GTK, no sockets.

pub mod cancel;
pub mod copy;
pub mod delete;
pub mod moveop;
pub mod scan;
pub mod types;

pub use cancel::CancelToken;
pub use types::{JobSpec, Progress};

use opusdm_ipc::FileMeta;
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
