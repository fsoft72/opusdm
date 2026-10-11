//! Blocking filesystem operations (copy, move, delete, mkdir) with progress
//! events and overwrite-conflict resolution. No GTK, no sockets.

pub mod attrs;
pub mod cancel;
pub mod checkfit;
pub mod cleanup;
pub mod compress;
pub mod copy;
pub mod crypt;
pub mod delete;
pub mod duplicate;
pub mod errors;
pub mod extract;
pub mod joinsplit;
pub mod links;
pub mod moveop;
pub mod pool;
pub mod rename;
pub mod scan;
pub mod tools;
pub mod types;

pub use cancel::CancelToken;
pub use errors::FsError;
pub use rename::rename_noreplace;
pub use types::{JobSpec, Progress, TransferOpts};

use opusdm_ipc::{FileMeta, Op};
use std::path::{Path, PathBuf};
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
    let _ = tx.send(run_job_inner(&spec, &tx, &cancel));
}

/// A `Done` reporting one failure that happened before any file was touched.
fn rejected(path: &Path, msg: String) -> Progress {
    Progress::Done { copied: 0, skipped: 0, cancelled: false, errors: vec![(path.to_path_buf(), msg)] }
}

/// Dispatch `spec` to the right engine and return its terminal `Done`.
fn run_job_inner(spec: &JobSpec, tx: &Sender<Progress>, cancel: &CancelToken) -> Progress {
    let (srcs, dest) = (&spec.srcs, &spec.dest_dir);
    let names = match &spec.op {
        Op::CopyAs { new_names } | Op::MoveAs { new_names } => Some(new_names.clone()),
        Op::Link { new_names: Some(n), .. } => Some(n.clone()),
        _ => None,
    };
    if let Some(n) = &names {
        if let Err(msg) = check_names(n, srcs.len()) {
            return rejected(dest, msg);
        }
    }
    let opts = match TransferOpts::from_options(&spec.options, names) {
        Ok(o) => o,
        Err(msg) => return rejected(dest, msg),
    };
    match &spec.op {
        Op::Copy if spec.options.move_when_same && same_device(srcs, dest) => {
            moveop::run_move_opts(srcs, dest, tx, cancel, &opts)
        }
        Op::Copy | Op::CopyAs { .. } => copy::copy_tree_opts(srcs, dest, tx, cancel, false, &opts),
        Op::Move | Op::MoveAs { .. } => moveop::run_move_opts(srcs, dest, tx, cancel, &opts),
        Op::Duplicate => duplicate::run_duplicate(srcs, &opts, tx, cancel),
        Op::Delete => match &opts.glob {
            Some(glob) => delete::run_delete_filtered(srcs, glob, tx, cancel),
            None => delete::run_delete(srcs, tx, cancel),
        },
        Op::Chmod { mode, recurse } => {
            attrs::run_chmod(srcs, *mode, *recurse, opts.glob.as_ref(), tx, cancel)
        }
        Op::Touch { unix_secs, recurse } => {
            attrs::run_touch(srcs, *unix_secs, *recurse, opts.glob.as_ref(), tx, cancel)
        }
        Op::Link { symbolic, .. } => {
            links::run_link(srcs, dest, *symbolic, opts.names.as_deref(), tx, cancel)
        }
        Op::Join { target } => match opusdm_ipc::glob::validate_new_name(target) {
            Ok(()) => joinsplit::run_join(srcs, dest, target, tx, cancel),
            Err(msg) => rejected(dest, msg),
        },
        Op::Compress { archive } => compress::run_compress(srcs, dest, archive, tx, cancel),
        Op::Extract => extract::run_extract(srcs, dest, tx, cancel),
        Op::Encrypt { password } => crypt::run_crypt(srcs, dest, &password.0, true, tx, cancel),
        Op::Decrypt { password } => crypt::run_crypt(srcs, dest, &password.0, false, tx, cancel),
        Op::Split { chunk_bytes } => joinsplit::run_split(srcs, dest, *chunk_bytes, tx, cancel),
        Op::MakeDir { name } => delete::make_entry(dest, name, false),
        Op::NewFile { name } => delete::make_entry(dest, name, true),
    }
}

/// Reject a `CopyAs`/`MoveAs` name list that does not fit the sources.
fn check_names(names: &[String], sources: usize) -> Result<(), String> {
    if names.len() != sources {
        return Err(format!("{} new names given for {sources} sources", names.len()));
    }
    names.iter().try_for_each(|n| opusdm_ipc::glob::validate_new_name(n))?;
    let mut seen = std::collections::HashSet::new();
    match names.iter().find(|n| !seen.insert(n.as_str())) {
        Some(dup) => Err(format!("several sources would be named {dup:?}")),
        None => Ok(()),
    }
}

/// Whether every source lives on the same device as `dest_dir`.
pub fn same_device(srcs: &[PathBuf], dest_dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(dest) = std::fs::metadata(dest_dir) else { return false };
    !srcs.is_empty()
        && srcs.iter().all(|s| {
            std::fs::symlink_metadata(s).map(|m| m.dev() == dest.dev()).unwrap_or(false)
        })
}
