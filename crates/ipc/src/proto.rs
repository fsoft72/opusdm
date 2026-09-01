//! Messages exchanged over the hub socket. `ToHub` travels Lister -> Hub,
//! `ToLister` travels Hub -> Lister. Both are framed JSON (see `frame`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable identifier the hub assigns to each connected Lister.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ListerId(pub u64);

/// Which global role a Lister currently holds, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// The Lister that Copy/Move read from.
    Source,
    /// The Lister that Copy/Move write into.
    Dest,
    /// No role.
    None,
}

/// Minimal metadata shown in the overwrite-conflict prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    /// File size in bytes.
    pub size: u64,
    /// Modification time as whole seconds since the Unix epoch.
    pub modified_unix: i64,
}

/// A filesystem operation the hub can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    /// Recursively copy `srcs` into `dest_dir`.
    Copy,
    /// Move `srcs` into `dest_dir` (rename fast-path, else copy+delete).
    Move,
    /// Permanently delete `srcs` (no trash).
    Delete,
    /// Create one directory named `name` inside `dest_dir`.
    MakeDir { name: String },
    /// Create one empty file named `name` inside `dest_dir`.
    NewFile { name: String },
}

/// The user's answer to an overwrite conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    /// Skip this one entry.
    Skip,
    /// Overwrite this one entry.
    Overwrite,
    /// Skip this and every later conflict in the job.
    SkipAll,
    /// Overwrite this and every later conflict in the job.
    OverwriteAll,
    /// Abort the whole job now.
    Cancel,
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// Ran to completion.
    Ok { copied: u64, skipped: u64 },
    /// Stopped early on user cancel; `copied`/`skipped` are partial counts.
    Cancelled { copied: u64, skipped: u64 },
    /// Failed before doing useful work (bad destination, all sources gone).
    Error { message: String },
}

/// Messages a Lister sends to the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToHub {
    /// First message on a fresh connection.
    Register { pid: u32, initial_path: PathBuf },
    /// Single-instance probe; hub replies `ToLister::Pong`.
    Ping,
    /// Sent by a redundant hub invocation asking the running hub to raise its toolbar.
    RaiseToolbar,
    /// Clean shutdown of this Lister.
    Goodbye,
    /// Explicit role button pressed.
    SetRole(Role),
    /// Pin this Lister's current role against focus changes.
    LockRole,
    /// Release a previous `LockRole`.
    UnlockRole,
    /// This Lister's window gained focus.
    FocusGained,
    /// This Lister navigated to a new directory.
    PathChanged(PathBuf),
    /// Request a filesystem job. `dest_dir` is `None` for a button Copy/Move
    /// (the hub fills it from the DEST Lister's current path) and `Some` for
    /// an explicit destination (drag-drop, Delete, MakeDir, NewFile).
    RunJob { op: Op, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf> },
    /// Ask the hub to open another Lister.
    SpawnLister { start_path: Option<PathBuf> },
    /// Answer to a `ToLister::ConflictPrompt`.
    ConflictReply { req_id: u64, resolution: Resolution },
}

/// Messages the hub sends to a Lister.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToLister {
    /// Reply to `Register`; carries the assigned id and current roles.
    Welcome {
        lister_id: ListerId,
        source: Option<ListerId>,
        dests: Vec<ListerId>,
        source_color: String,
        dest_color: String,
    },
    /// Reply to `Ping`.
    Pong,
    /// Ask this Lister to raise and focus its window.
    Present,
    /// Global roles changed. There is at most one SOURCE and zero or more
    /// DEST Listers (a locked DEST persists; the unlocked one is transient).
    RoleChanged {
        source: Option<ListerId>,
        dests: Vec<ListerId>,
    },
    /// Progress tick for a running job.
    JobProgress {
        job_id: u64,
        op: Op,
        current_file: PathBuf,
        bytes_done: u64,
        bytes_total: u64,
        files_done: u64,
        files_total: u64,
    },
    /// A job finished.
    JobDone {
        job_id: u64,
        outcome: Outcome,
        errors: Vec<(PathBuf, String)>,
    },
    /// A destination entry exists; the Lister must prompt and reply.
    ConflictPrompt {
        req_id: u64,
        job_id: u64,
        path: PathBuf,
        src: FileMeta,
        dst: FileMeta,
    },
    /// A directory changed on disk; reload if showing it.
    InvalidatePath(PathBuf),
    /// Graceful-exit request.
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{recv, send};
    use std::io::Cursor;

    fn roundtrip_tohub(msg: ToHub) {
        let mut buf = Vec::new();
        send(&mut buf, &msg).unwrap();
        let got: Option<ToHub> = recv(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, Some(msg));
    }

    fn roundtrip_tolister(msg: ToLister) {
        let mut buf = Vec::new();
        send(&mut buf, &msg).unwrap();
        let got: Option<ToLister> = recv(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, Some(msg));
    }

    #[test]
    fn every_tohub_variant_roundtrips() {
        roundtrip_tohub(ToHub::Register { pid: 42, initial_path: "/home/x".into() });
        roundtrip_tohub(ToHub::Ping);
        roundtrip_tohub(ToHub::RaiseToolbar);
        roundtrip_tohub(ToHub::Goodbye);
        roundtrip_tohub(ToHub::SetRole(Role::Dest));
        roundtrip_tohub(ToHub::LockRole);
        roundtrip_tohub(ToHub::UnlockRole);
        roundtrip_tohub(ToHub::FocusGained);
        roundtrip_tohub(ToHub::PathChanged("/tmp".into()));
        roundtrip_tohub(ToHub::RunJob {
            op: Op::Copy,
            srcs: vec!["/a".into(), "/b".into()],
            dest_dir: Some("/dst".into()),
        });
        roundtrip_tohub(ToHub::RunJob { op: Op::Move, srcs: vec![], dest_dir: None });
        roundtrip_tohub(ToHub::SpawnLister { start_path: Some("/etc".into()) });
        roundtrip_tohub(ToHub::ConflictReply { req_id: 7, resolution: Resolution::OverwriteAll });
    }

    #[test]
    fn every_tolister_variant_roundtrips() {
        roundtrip_tolister(ToLister::Welcome {
            lister_id: ListerId(1),
            source: Some(ListerId(1)),
            dests: vec![],
            source_color: "#90EE90".to_string(),
            dest_color: "#FFB6C1".to_string(),
        });
        roundtrip_tolister(ToLister::Pong);
        roundtrip_tolister(ToLister::Present);
        roundtrip_tolister(ToLister::RoleChanged {
            source: None,
            dests: vec![ListerId(3), ListerId(4)],
        });
        roundtrip_tolister(ToLister::JobProgress {
            job_id: 1,
            op: Op::Move,
            current_file: "/a/b".into(),
            bytes_done: 10,
            bytes_total: 100,
            files_done: 1,
            files_total: 9,
        });
        roundtrip_tolister(ToLister::JobDone {
            job_id: 1,
            outcome: Outcome::Ok { copied: 9, skipped: 0 },
            errors: vec![("/x".into(), "permission denied".into())],
        });
        roundtrip_tolister(ToLister::ConflictPrompt {
            req_id: 2,
            job_id: 1,
            path: "/dst/f".into(),
            src: FileMeta { size: 1, modified_unix: 10 },
            dst: FileMeta { size: 2, modified_unix: 20 },
        });
        roundtrip_tolister(ToLister::InvalidatePath("/dst".into()));
        roundtrip_tolister(ToLister::Shutdown);
    }
}
