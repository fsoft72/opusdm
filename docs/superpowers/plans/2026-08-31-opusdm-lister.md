# OpusDM Lister Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Directory Opus 5.5-style Linux file manager: an always-on "hub" controller plus multiple single-pane "Lister" windows, each its own process, where any Lister can be tagged SOURCE or DEST and Copy/Move act SOURCE -> DEST.

**Architecture:** Cargo workspace, three binaries (`opusdm-hub`, `opusdm-lister`) and two pure libraries (`opusdm-ipc`, `opusdm-fsops`). The hub owns a Unix domain socket, the Lister registry, the global SOURCE/DEST roles, and the job engine (all file I/O). Listers are GTK4 windows spawned by the hub that render what the hub tells them and send request messages back. No async runtime: sockets are blocking `std::os::unix::net` on dedicated threads; the GTK thread and worker threads bridge through `async-channel` drained by `glib::MainContext::spawn_local`.

**Tech Stack:** Rust 1.91 (edition 2021), GTK4 via `gtk4` 0.11, `serde`/`serde_json` for the wire protocol, `walkdir` + `filetime` for filesystem operations, `async-channel` for thread<->GTK bridging, `signal-hook` + `libc` for child reaping.

**Spec:** `docs/superpowers/specs/2026-08-31-opusdm-lister-design.md`

## Global Constraints

- Rust edition **2021**, workspace `resolver = "2"`, toolchain **1.91**.
- **No tokio, no async runtime.** Sockets are blocking `std::os::unix::net::{UnixListener, UnixStream}` on dedicated `std::thread`s. GTK-thread <-> other-thread bridging is `async-channel` 2.5 drained by `glib::MainContext::spawn_local`.
- Dependency versions (exact, in workspace `[workspace.dependencies]`): `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`, `walkdir = "2.5"`, `filetime = "0.2"`, `async-channel = "2.5"`, `signal-hook = "0.4"`, `libc = "0.2"`, `thiserror = "2"`, `gtk4 = { version = "0.11", package = "gtk4" }`, `tempfile = "3"` (dev-dependency only).
- The `gtk4` crate is imported as `use gtk4 as gtk;` in every hub/lister module for brevity.
- Never use the character `—` (em dash) anywhere: code, comments, strings, docs. Use `-`.
- All comments, identifiers, doc-comments, and commit messages in **English**.
- Constants are `UPPER_SNAKE_CASE`. Rust privacy is expressed with visibility (`pub`, `pub(crate)`, module-private); do **not** prefix Rust functions with `_`.
- Every public item (type, function, method, module) carries a `///` doc-comment.
- Every file ends with a trailing newline.
- Socket path: `$OPUSDM_HUB_SOCK` if set, else `$XDG_RUNTIME_DIR/opusdm/hub.sock`, else `/tmp/opusdm-$UID/hub.sock`. The containing directory is created mode `0700`.
- Delete is **permanent** (no trash) in v1.
- Maintain `CHANGES.md` at the repo root: append a dated bullet per task. Commit once per task; working code only; never `--no-verify`; no AI-attribution trailers.
- Protocol note (refinement of the spec): the spec's single `Msg` is split into two enums, `ToHub` (Lister -> Hub) and `ToLister` (Hub -> Lister). The spec's "glib channel" bridge is realized with `async-channel`.

---

## File Structure

```
opusdm/
├── Cargo.toml                        # workspace manifest + [workspace.dependencies]
├── .gitignore                        # /target
├── CHANGES.md
├── run-x11.sh                        # updated: launches hub under Xephyr
├── crates/
│   ├── ipc/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                # re-exports; module wiring
│   │       ├── frame.rs              # length-prefixed frame read/write + typed send/recv
│   │       ├── proto.rs              # ToHub, ToLister, ListerId, Role, FileMeta, Op, Resolution, Outcome
│   │       └── sockpath.rs           # hub_sock_path(), ensure_sock_dir(), probe_live_hub()
│   ├── fsops/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                # re-exports; run_job() dispatcher
│   │       ├── cancel.rs             # CancelToken
│   │       ├── types.rs              # JobSpec, Progress
│   │       ├── scan.rs              # recursive scan -> work list + totals
│   │       ├── copy.rs              # copy_file(), copy tree with conflict handling
│   │       ├── moveop.rs             # rename fast-path + EXDEV copy-then-delete
│   │       └── delete.rs             # recursive permanent delete; mkdir/newfile
│   ├── hub/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs               # startup: sockpath, single-instance, threads, GTK app
│   │       ├── roles.rs              # RoleState, RoleEvent, RoleEffect, apply_role_event()
│   │       ├── jobs.rs               # JobEngine: submit/validate/finish, concurrency cap, fan-out mapping
│   │       ├── server.rs             # UnixListener, per-lister reader threads, HubState owner loop
│   │       ├── children.rs           # spawn_lister(), SIGCHLD reaping
│   │       └── toolbar.rs            # GTK toolbar window + wiring to owner loop
│   └── lister/
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs               # startup: env guard, connect, Register, threads, GTK app
│           ├── client.rs             # socket write handle + reader thread -> async-channel
│           ├── model.rs              # Row, read_dir_rows(), format_size/format_mtime, dir_first_cmp
│           ├── window.rs             # Lister window: path bar, ColumnView, status line, navigation
│           ├── actions.rs            # button row, operation dispatch, role buttons, conflict dialog
│           └── dnd.rs                # drag-and-drop between Listers
└── docs/superpowers/
    ├── specs/2026-08-31-opusdm-lister-design.md
    └── plans/2026-08-31-opusdm-lister.md
```

---

## Task 1: Workspace scaffold + IPC framing

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `CHANGES.md`
- Create: `crates/ipc/Cargo.toml`, `crates/ipc/src/lib.rs`, `crates/ipc/src/frame.rs`
- Test: inline `#[cfg(test)]` in `crates/ipc/src/frame.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `opusdm_ipc::frame::write_frame<W: Write>(w: &mut W, bytes: &[u8]) -> io::Result<()>`
  - `opusdm_ipc::frame::read_frame<R: Read>(r: &mut R) -> io::Result<Option<Vec<u8>>>` (`Ok(None)` == clean EOF)
  - `opusdm_ipc::frame::send<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()>`
  - `opusdm_ipc::frame::recv<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<Option<T>>`
  - `opusdm_ipc::frame::MAX_FRAME: u32` (= `16 * 1024 * 1024`)

- [ ] **Step 1: Create the workspace manifest**

`Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/ipc", "crates/fsops", "crates/hub", "crates/lister"]

[workspace.package]
edition = "2021"
rust-version = "1.91"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
walkdir = "2.5"
filetime = "0.2"
async-channel = "2.5"
signal-hook = "0.4"
libc = "0.2"
thiserror = "2"
gtk4 = { version = "0.11" }
tempfile = "3"
```

`.gitignore`:
```
/target
```

`CHANGES.md`:
```markdown
# Changes

## 2026-08-31

- Task 1: workspace scaffold, opusdm-ipc frame module (length-prefixed JSON frames).
```

- [ ] **Step 2: Create the ipc crate manifest and lib root**

`crates/ipc/Cargo.toml`:
```toml
[package]
name = "opusdm-ipc"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[lib]
name = "opusdm_ipc"
path = "src/lib.rs"

[dependencies]
serde.workspace = true
serde_json.workspace = true
libc.workspace = true
```

`crates/ipc/src/lib.rs`:
```rust
//! Wire protocol and socket helpers shared by opusdm-hub and opusdm-lister.

pub mod frame;
```

- [ ] **Step 3: Write the failing tests for framing**

`crates/ipc/src/frame.rs` (test module only, at first):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A reader that yields at most `chunk` bytes per `read`, to exercise
    /// partial-read handling inside `read_frame`.
    struct Drip<'a> {
        data: &'a [u8],
        pos: usize,
        chunk: usize,
    }
    impl std::io::Read for Drip<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let remaining = &self.data[self.pos..];
            let n = remaining.len().min(self.chunk).min(buf.len());
            buf[..n].copy_from_slice(&remaining[..n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn roundtrip_single_frame() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap().as_deref(), Some(&b"hello"[..]));
        assert_eq!(read_frame(&mut cur).unwrap(), None);
    }

    #[test]
    fn two_frames_in_one_buffer() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"aa").unwrap();
        write_frame(&mut buf, b"bbbb").unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap().as_deref(), Some(&b"aa"[..]));
        assert_eq!(read_frame(&mut cur).unwrap().as_deref(), Some(&b"bbbb"[..]));
    }

    #[test]
    fn partial_reads_are_reassembled() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"payload-across-drips").unwrap();
        let mut drip = Drip { data: &buf, pos: 0, chunk: 1 };
        assert_eq!(
            read_frame(&mut drip).unwrap().as_deref(),
            Some(&b"payload-across-drips"[..])
        );
    }

    #[test]
    fn oversized_length_is_rejected() {
        let mut cur = Cursor::new((MAX_FRAME + 1).to_be_bytes().to_vec());
        let err = read_frame(&mut cur).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn typed_send_recv_roundtrip() {
        let mut buf = Vec::new();
        send(&mut buf, &vec![1u32, 2, 3]).unwrap();
        let mut cur = Cursor::new(buf);
        let got: Option<Vec<u32>> = recv(&mut cur).unwrap();
        assert_eq!(got, Some(vec![1, 2, 3]));
    }
}
```

- [ ] **Step 4: Run tests, verify they fail to compile**

Run: `cargo test -p opusdm-ipc`
Expected: FAIL - `write_frame`, `read_frame`, `send`, `recv`, `MAX_FRAME` not found.

- [ ] **Step 5: Implement the frame module**

Prepend to `crates/ipc/src/frame.rs` (above the test module):
```rust
//! Length-prefixed framing: a 4-byte big-endian length, then that many bytes.
//! Bodies are JSON. Blocking I/O only.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::{self, Read, Write};

/// Largest frame body accepted by [`read_frame`], in bytes.
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;

/// Write one frame: the length prefix, the bytes, then flush.
pub fn write_frame<W: Write>(w: &mut W, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame body too large"))?;
    if len > MAX_FRAME {
        return io::Result::Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame body exceeds MAX_FRAME",
        ));
    }
    w.write_all(&len.to_be_bytes())?;
    w.write_all(bytes)?;
    w.flush()
}

/// Read one frame. Returns `Ok(None)` on a clean EOF at a frame boundary.
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incoming frame exceeds MAX_FRAME",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Serialize `msg` as JSON and write it as one frame.
pub fn send<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(io::Error::other)?;
    write_frame(w, &bytes)
}

/// Read one frame and deserialize it from JSON. `Ok(None)` on clean EOF.
pub fn recv<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<Option<T>> {
    match read_frame(r)? {
        Some(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(io::Error::other)?)),
        None => Ok(None),
    }
}
```

- [ ] **Step 6: Run tests, verify pass**

Run: `cargo test -p opusdm-ipc`
Expected: PASS (5 tests).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml .gitignore CHANGES.md crates/ipc
git commit -m "feat(ipc): workspace scaffold and length-prefixed JSON framing"
```

---

## Task 2: IPC protocol types + socket path + live-hub probe

**Files:**
- Create: `crates/ipc/src/proto.rs`, `crates/ipc/src/sockpath.rs`
- Modify: `crates/ipc/src/lib.rs` (add `pub mod proto; pub mod sockpath;` and re-exports)
- Test: inline `#[cfg(test)]` in `proto.rs` and `sockpath.rs`

**Interfaces:**
- Consumes: `opusdm_ipc::frame::{send, recv}` (Task 1)
- Produces:
  - `opusdm_ipc::ListerId(pub u64)` - `Copy, Eq, Hash, Ord, Serialize, Deserialize`
  - `opusdm_ipc::Role` - `Source | Dest | None`, `Copy, Eq`
  - `opusdm_ipc::FileMeta { size: u64, modified_unix: i64 }` - `Copy, Eq`
  - `opusdm_ipc::Op` - `Copy | Move | Delete | MakeDir { name: String } | NewFile { name: String }`, `Clone, Eq`
  - `opusdm_ipc::Resolution` - `Skip | Overwrite | SkipAll | OverwriteAll | Cancel`, `Copy, Eq`
  - `opusdm_ipc::Outcome` - `Ok { copied: u64, skipped: u64 } | Cancelled { copied: u64, skipped: u64 } | Error { message: String }`
  - `opusdm_ipc::ToHub` and `opusdm_ipc::ToLister` enums (variants below), `Clone, Eq, Serialize, Deserialize`
  - `opusdm_ipc::sockpath::hub_sock_path() -> PathBuf`
  - `opusdm_ipc::sockpath::ensure_sock_dir(path: &Path) -> io::Result<()>` (creates parent dir mode 0700)
  - `opusdm_ipc::sockpath::probe_live_hub(path: &Path) -> bool`
  - `opusdm_ipc::sockpath::ENV_SOCK: &str` (= `"OPUSDM_HUB_SOCK"`)

- [ ] **Step 1: Write proto.rs with the type definitions**

`crates/ipc/src/proto.rs`:
```rust
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
    /// Request a filesystem job.
    RunJob { op: Op, srcs: Vec<PathBuf>, dest_dir: PathBuf },
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
        dest: Option<ListerId>,
    },
    /// Reply to `Ping`.
    Pong,
    /// Ask this Lister to raise and focus its window.
    Present,
    /// Global roles changed.
    RoleChanged {
        source: Option<ListerId>,
        dest: Option<ListerId>,
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
            dest_dir: "/dst".into(),
        });
        roundtrip_tohub(ToHub::SpawnLister { start_path: Some("/etc".into()) });
        roundtrip_tohub(ToHub::ConflictReply { req_id: 7, resolution: Resolution::OverwriteAll });
    }

    #[test]
    fn every_tolister_variant_roundtrips() {
        roundtrip_tolister(ToLister::Welcome {
            lister_id: ListerId(1),
            source: Some(ListerId(1)),
            dest: None,
        });
        roundtrip_tolister(ToLister::Pong);
        roundtrip_tolister(ToLister::Present);
        roundtrip_tolister(ToLister::RoleChanged { source: None, dest: Some(ListerId(3)) });
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
```

- [ ] **Step 2: Write sockpath.rs**

`crates/ipc/src/sockpath.rs`:
```rust
//! Where the hub socket lives, and how to tell a live hub from a stale file.

use crate::frame::{recv, send};
use crate::proto::{ToHub, ToLister};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Environment variable that, when set, overrides the socket path.
pub const ENV_SOCK: &str = "OPUSDM_HUB_SOCK";

/// Resolve the hub socket path.
///
/// Order: `$OPUSDM_HUB_SOCK`, then `$XDG_RUNTIME_DIR/opusdm/hub.sock`,
/// then `/tmp/opusdm-$UID/hub.sock`.
pub fn hub_sock_path() -> PathBuf {
    if let Some(p) = std::env::var_os(ENV_SOCK) {
        return PathBuf::from(p);
    }
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !rt.is_empty() {
            return PathBuf::from(rt).join("opusdm").join("hub.sock");
        }
    }
    // SAFETY: getuid is always safe; it cannot fail and touches no memory.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/opusdm-{uid}/hub.sock"))
}

/// Create the directory that will contain `path`, mode 0700. No-op if present.
pub fn ensure_sock_dir(path: &Path) -> io::Result<()> {
    let Some(dir) = path.parent() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path has no parent"));
    };
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true).mode(0o700);
    b.create(dir)
}

/// Connect to `path` and probe with `Ping`. `true` only if a live hub replies `Pong`.
pub fn probe_live_hub(path: &Path) -> bool {
    let Ok(mut s) = UnixStream::connect(path) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
    if send(&mut s, &ToHub::Ping).is_err() {
        return false;
    }
    matches!(recv::<_, ToLister>(&mut s), Ok(Some(ToLister::Pong)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        // Uses a guard so the test does not leak the variable to siblings.
        temp_env_var(ENV_SOCK, Some("/run/custom.sock"), || {
            assert_eq!(hub_sock_path(), PathBuf::from("/run/custom.sock"));
        });
    }

    #[test]
    fn xdg_runtime_dir_is_used_when_no_override() {
        temp_env_var(ENV_SOCK, None, || {
            temp_env_var("XDG_RUNTIME_DIR", Some("/run/user/1000"), || {
                assert_eq!(
                    hub_sock_path(),
                    PathBuf::from("/run/user/1000/opusdm/hub.sock")
                );
            });
        });
    }

    #[test]
    fn tmp_fallback_shape() {
        temp_env_var(ENV_SOCK, None, || {
            temp_env_var("XDG_RUNTIME_DIR", Some(""), || {
                let p = hub_sock_path();
                let s = p.to_str().unwrap();
                assert!(s.starts_with("/tmp/opusdm-"), "{s}");
                assert!(s.ends_with("/hub.sock"), "{s}");
            });
        });
    }

    #[test]
    fn probe_returns_false_for_missing_socket() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!probe_live_hub(&dir.path().join("nope.sock")));
    }

    /// Minimal scoped env-var setter/restorer for single-threaded tests.
    fn temp_env_var(key: &str, val: Option<&str>, f: impl FnOnce()) {
        let prev = std::env::var_os(key);
        match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        f();
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
```

- [ ] **Step 3: Wire the modules in lib.rs**

`crates/ipc/src/lib.rs`:
```rust
//! Wire protocol and socket helpers shared by opusdm-hub and opusdm-lister.

pub mod frame;
pub mod proto;
pub mod sockpath;

pub use proto::{
    FileMeta, ListerId, Op, Outcome, Resolution, Role, ToHub, ToLister,
};
```

- [ ] **Step 4: Add the dev-dependency for the sockpath tests**

Append to `crates/ipc/Cargo.toml`:
```toml
[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p opusdm-ipc -- --test-threads=1`
Expected: PASS. `--test-threads=1` because the sockpath tests mutate process env.
Add a note in `crates/ipc/src/sockpath.rs` above the test module: `// These tests mutate process environment; run single-threaded.`

- [ ] **Step 6: Commit**

```bash
git add crates/ipc CHANGES.md
git commit -m "feat(ipc): protocol enums, socket path resolution, live-hub probe"
```
Append to `CHANGES.md` under the dated heading:
```markdown
- Task 2: opusdm-ipc protocol enums (ToHub/ToLister), socket path resolution, probe_live_hub.
```

---

## Task 3: fsops crate - CancelToken, types, scan pass

**Files:**
- Create: `crates/fsops/Cargo.toml`, `crates/fsops/src/lib.rs`, `crates/fsops/src/cancel.rs`, `crates/fsops/src/types.rs`, `crates/fsops/src/scan.rs`
- Test: inline `#[cfg(test)]` in `cancel.rs` and `scan.rs`

**Interfaces:**
- Consumes: `opusdm_ipc::{FileMeta, Op, Resolution}` (Task 2)
- Produces:
  - `opusdm_fsops::CancelToken` - `Clone, Default`; `new()`, `cancel(&self)`, `is_cancelled(&self) -> bool`
  - `opusdm_fsops::JobSpec { pub op: Op, pub srcs: Vec<PathBuf>, pub dest_dir: PathBuf }`
  - `opusdm_fsops::Progress` enum: `Scanning { files: u64, bytes: u64 }`, `File { path: PathBuf, done_bytes: u64, total_bytes: u64, files_done: u64, files_total: u64 }`, `Conflict { path: PathBuf, src: FileMeta, dst: FileMeta, reply: std::sync::mpsc::Sender<Resolution> }`, `Done { copied: u64, skipped: u64, cancelled: bool, errors: Vec<(PathBuf, String)> }`
  - `opusdm_fsops::scan::Plan { pub total_files: u64, pub total_bytes: u64, pub items: Vec<Item> }`
  - `opusdm_fsops::scan::Item` enum: `Dir { rel: PathBuf }`, `File { rel: PathBuf, abs: PathBuf, size: u64 }`, `Symlink { rel: PathBuf, target: PathBuf }`
  - `opusdm_fsops::scan::scan(srcs: &[PathBuf]) -> io::Result<Plan>` - `rel` is relative to each source's parent, so `dest = dest_dir.join(rel)`
  - `opusdm_fsops::file_meta(m: &std::fs::Metadata) -> FileMeta`

- [ ] **Step 1: Create the crate manifest and lib root**

`crates/fsops/Cargo.toml`:
```toml
[package]
name = "opusdm-fsops"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[lib]
name = "opusdm_fsops"
path = "src/lib.rs"

[dependencies]
opusdm-ipc = { path = "../ipc" }
walkdir.workspace = true
filetime.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

`crates/fsops/src/lib.rs`:
```rust
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
```

- [ ] **Step 2: Write cancel.rs with its failing test**

`crates/fsops/src/cancel.rs`:
```rust
//! A shareable one-way cancel flag.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A cloneable flag. Every clone observes the same state; once set it stays set.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether [`cancel`](Self::cancel) has been called on this token or a clone.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_shares_state() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!a.is_cancelled());
        b.cancel();
        assert!(a.is_cancelled());
        assert!(b.is_cancelled());
    }
}
```

- [ ] **Step 3: Write types.rs**

`crates/fsops/src/types.rs`:
```rust
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
```

- [ ] **Step 4: Write the failing tests for scan.rs**

`crates/fsops/src/scan.rs` (test module first):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_a_small_tree() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("proj");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"hello").unwrap();      // 5 bytes
        fs::write(root.join("sub/b.txt"), b"worldXX").unwrap(); // 7 bytes

        let plan = scan(&[root.clone()]).unwrap();

        assert_eq!(plan.total_files, 2);
        assert_eq!(plan.total_bytes, 12);
        // "proj" and "proj/sub" are directory items, relative to root.parent().
        let dirs: Vec<_> = plan
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Dir { rel } => Some(rel.clone()),
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
        // 1 real file, 1 symlink; symlink contributes no bytes and no file count.
        assert_eq!(plan.total_files, 1);
        assert_eq!(plan.total_bytes, 1);
        assert!(plan.items.iter().any(|i| matches!(i, Item::Symlink { rel, .. } if rel == &PathBuf::from("d/link.txt"))));
    }
}
```

- [ ] **Step 5: Run tests, verify failure**

Run: `cargo test -p opusdm-fsops scan`
Expected: FAIL - `scan`, `Plan`, `Item` not found.

- [ ] **Step 6: Implement scan.rs**

Prepend to `crates/fsops/src/scan.rs`:
```rust
//! Recursive pre-pass: enumerate every entry to copy/move and total up
//! file count and bytes, so the work pass can report progress.

use std::io;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// One entry to recreate at the destination. `rel` is relative to the
/// originating source's parent directory.
pub enum Item {
    /// A directory to create.
    Dir { rel: PathBuf },
    /// A regular file to copy, with its absolute source path and size.
    File { rel: PathBuf, abs: PathBuf, size: u64 },
    /// A symbolic link to recreate verbatim (target not followed).
    Symlink { rel: PathBuf, target: PathBuf },
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
    let mut plan = Plan { total_files: 0, total_bytes: 0, items: Vec::new() };
    for src in srcs {
        let base = src.parent().unwrap_or_else(|| Path::new("/"));
        for entry in WalkDir::new(src).follow_links(false).sort_by_file_name() {
            let entry = entry.map_err(io::Error::other)?;
            let rel = entry
                .path()
                .strip_prefix(base)
                .map_err(|_| io::Error::other("path outside its source base"))?
                .to_path_buf();
            let ft = entry.file_type();
            if ft.is_symlink() {
                let target = std::fs::read_link(entry.path())?;
                plan.items.push(Item::Symlink { rel, target });
            } else if ft.is_dir() {
                plan.items.push(Item::Dir { rel });
            } else {
                let size = entry.metadata().map_err(io::Error::other)?.len();
                plan.total_files += 1;
                plan.total_bytes += size;
                plan.items.push(Item::File { rel, abs: entry.path().to_path_buf(), size });
            }
        }
    }
    Ok(plan)
}
```

- [ ] **Step 7: Run tests, verify pass**

Run: `cargo test -p opusdm-fsops`
Expected: PASS (cancel + scan tests). `copy`, `delete`, `moveop` modules are empty stubs for now - add `//! TODO in Task 4/5` is NOT allowed; instead create them as empty files containing only a `//!` doc line each so `lib.rs` compiles:
- `crates/fsops/src/copy.rs`: `//! Copy implementation. Populated in Task 4.`
- `crates/fsops/src/delete.rs`: `//! Delete and single-entry creation. Populated in Task 5.`
- `crates/fsops/src/moveop.rs`: `//! Move implementation. Populated in Task 5.`

- [ ] **Step 8: Commit**

```bash
git add crates/fsops CHANGES.md
git commit -m "feat(fsops): CancelToken, job types, recursive scan pass"
```
`CHANGES.md`: `- Task 3: opusdm-fsops CancelToken, JobSpec/Progress, scan().`

---

## Task 4: fsops - copy file, copy tree with conflict resolution, run_job dispatcher

**Files:**
- Modify: `crates/fsops/src/copy.rs` (implement), `crates/fsops/src/lib.rs` (add `run_job`)
- Test: inline `#[cfg(test)]` in `copy.rs`

**Interfaces:**
- Consumes: `crate::scan::{scan, Plan, Item}`, `crate::CancelToken`, `crate::types::Progress`, `crate::file_meta` (Task 3); `opusdm_ipc::{Op, Resolution}`
- Produces:
  - `opusdm_fsops::run_job(spec: JobSpec, tx: std::sync::mpsc::Sender<Progress>, cancel: CancelToken)` - always ends by sending exactly one `Progress::Done`
  - `opusdm_fsops::copy::copy_tree(srcs: &[PathBuf], dest_dir: &Path, tx: &Sender<Progress>, cancel: &CancelToken, delete_src_after: bool) -> Progress` - returns the `Done` variant to send; `delete_src_after == true` implements the cross-filesystem Move path
  - `opusdm_fsops::copy::copy_file(src: &Path, dst: &Path, cancel: &CancelToken, on_bytes: &mut dyn FnMut(u64)) -> io::Result<()>` - preserves unix mode + mtime; removes a partial `dst` and returns `ErrorKind::Interrupted` on cancel

- [ ] **Step 1: Write the failing tests**

`crates/fsops/src/copy.rs` (test module first):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::Resolution;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::{channel, Sender};
    use std::thread;

    /// Drain progress on a background thread, auto-answering every conflict
    /// with `answer`. Returns a handle yielding (events, final Done fields).
    fn run_copy(
        srcs: Vec<PathBuf>,
        dest: PathBuf,
        answer: Resolution,
    ) -> (u64, u64, bool, Vec<(PathBuf, String)>) {
        let (tx, rx) = channel::<Progress>();
        let cancel = CancelToken::new();
        let worker = {
            let tx = tx.clone();
            thread::spawn(move || {
                let done = copy_tree(&srcs, &dest, &tx, &cancel, false);
                tx.send(done).unwrap();
            })
        };
        drop(tx);
        let mut last = (0, 0, false, Vec::new());
        while let Ok(ev) = rx.recv() {
            match ev {
                Progress::Conflict { reply, .. } => {
                    let _ = reply.send(answer);
                }
                Progress::Done { copied, skipped, cancelled, errors } => {
                    last = (copied, skipped, cancelled, errors);
                }
                _ => {}
            }
        }
        worker.join().unwrap();
        last
    }

    #[test]
    fn copies_a_tree_and_preserves_mode() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("f.txt"), b"data").unwrap();
        fs::set_permissions(src.join("f.txt"), fs::Permissions::from_mode(0o640)).unwrap();
        fs::write(src.join("sub/g.txt"), b"more").unwrap();
        fs::create_dir_all(&dst).unwrap();

        let (copied, skipped, cancelled, errors) =
            run_copy(vec![src.clone()], dst.clone(), Resolution::Skip);

        assert_eq!((copied, skipped, cancelled), (2, 0, false));
        assert!(errors.is_empty());
        assert_eq!(fs::read(dst.join("s/f.txt")).unwrap(), b"data");
        assert_eq!(fs::read(dst.join("s/sub/g.txt")).unwrap(), b"more");
        let mode = fs::metadata(dst.join("s/f.txt")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[test]
    fn conflict_skip_keeps_destination() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(dst.join("s")).unwrap();
        fs::write(src.join("f.txt"), b"NEW").unwrap();
        fs::write(dst.join("s/f.txt"), b"OLD").unwrap();

        let (copied, skipped, _, _) = run_copy(vec![src], dst.clone(), Resolution::Skip);

        assert_eq!((copied, skipped), (0, 1));
        assert_eq!(fs::read(dst.join("s/f.txt")).unwrap(), b"OLD");
    }

    #[test]
    fn conflict_overwrite_replaces_destination() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(dst.join("s")).unwrap();
        fs::write(src.join("f.txt"), b"NEW").unwrap();
        fs::write(dst.join("s/f.txt"), b"OLD").unwrap();

        let (copied, skipped, _, _) = run_copy(vec![src], dst.clone(), Resolution::OverwriteAll);

        assert_eq!((copied, skipped), (1, 0));
        assert_eq!(fs::read(dst.join("s/f.txt")).unwrap(), b"NEW");
    }

    #[test]
    fn cancel_stops_and_reports_partial() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        for i in 0..50 {
            fs::write(src.join(format!("f{i:02}.bin")), vec![0u8; 4096]).unwrap();
        }
        let (tx, rx) = channel::<Progress>();
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let s2 = vec![src.clone()];
        let d2 = dst.clone();
        let worker = thread::spawn(move || {
            let done = copy_tree(&s2, &d2, &tx, &c2, false);
            done
        });
        // Cancel almost immediately.
        cancel.cancel();
        // Drain until the worker returns.
        while rx.recv().is_ok() {}
        let done = worker.join().unwrap();
        match done {
            Progress::Done { cancelled, .. } => assert!(cancelled),
            _ => panic!("expected Done"),
        }
    }

    #[test]
    fn run_job_copy_emits_scanning_then_done() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("a"), b"12345").unwrap();

        let (tx, rx) = channel::<Progress>();
        let spec = JobSpec { op: opusdm_ipc::Op::Copy, srcs: vec![src], dest_dir: dst.clone() };
        let h = thread::spawn(move || crate::run_job(spec, tx, CancelToken::new()));
        let mut saw_scanning = false;
        let mut saw_done = false;
        while let Ok(ev) = rx.recv() {
            match ev {
                Progress::Scanning { files, bytes } => {
                    assert_eq!((files, bytes), (1, 5));
                    saw_scanning = true;
                }
                Progress::Done { copied, .. } => {
                    assert_eq!(copied, 1);
                    saw_done = true;
                }
                _ => {}
            }
        }
        h.join().unwrap();
        assert!(saw_scanning && saw_done);
    }
}
```

- [ ] **Step 2: Run tests, verify failure**

Run: `cargo test -p opusdm-fsops copy`
Expected: FAIL - `copy_tree`, `copy_file`, `run_job` not found.

- [ ] **Step 3: Implement copy.rs**

Prepend to `crates/fsops/src/copy.rs`:
```rust
//! Copy pass: recreate directories, copy files with mode + mtime, resolve
//! overwrite conflicts, honor cancellation. Shared by Copy and the
//! cross-filesystem Move path.

use crate::cancel::CancelToken;
use crate::file_meta;
use crate::scan::{scan, Item};
use crate::types::Progress;
use opusdm_ipc::Resolution;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

/// Copy buffer size. 256 KiB balances syscall count against memory.
const COPY_BUF: usize = 256 * 1024;

/// Copy one regular file, preserving unix permission bits and mtime.
///
/// Calls `on_bytes` with the size of each chunk written. On cancel it removes
/// the partial destination and returns `ErrorKind::Interrupted`.
pub fn copy_file(
    src: &Path,
    dst: &Path,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> io::Result<()> {
    let mut reader = std::fs::File::open(src)?;
    let mut writer = std::fs::File::create(dst)?;
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        if cancel.is_cancelled() {
            drop(writer);
            let _ = std::fs::remove_file(dst);
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        on_bytes(n as u64);
    }
    writer.flush()?;
    let meta = std::fs::metadata(src)?;
    std::fs::set_permissions(dst, meta.permissions())?;
    let mtime = filetime::FileTime::from_last_modification_time(&meta);
    filetime::set_file_mtime(dst, mtime)?;
    Ok(())
}

/// Recreate `srcs` under `dest_dir`. Returns the `Progress::Done` to send.
///
/// `delete_src_after` removes each source tree once fully copied - the
/// cross-filesystem Move path.
pub fn copy_tree(
    srcs: &[PathBuf],
    dest_dir: &Path,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
    delete_src_after: bool,
) -> Progress {
    let plan = match scan(srcs) {
        Ok(p) => p,
        Err(e) => {
            return Progress::Done {
                copied: 0,
                skipped: 0,
                cancelled: false,
                errors: vec![(dest_dir.to_path_buf(), e.to_string())],
            }
        }
    };
    let _ = tx.send(Progress::Scanning { files: plan.total_files, bytes: plan.total_bytes });

    let mut copied = 0u64;
    let mut skipped = 0u64;
    let mut done_bytes = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();
    let mut overwrite_all = false;
    let mut skip_all = false;

    for item in &plan.items {
        if cancel.is_cancelled() {
            return Progress::Done { copied, skipped, cancelled: true, errors };
        }
        match item {
            Item::Dir { rel } => {
                if let Err(e) = std::fs::create_dir_all(dest_dir.join(rel)) {
                    errors.push((dest_dir.join(rel), e.to_string()));
                }
            }
            Item::Symlink { rel, target } => {
                let link = dest_dir.join(rel);
                let _ = std::fs::remove_file(&link);
                if let Err(e) = std::os::unix::fs::symlink(target, &link) {
                    errors.push((link, e.to_string()));
                }
            }
            Item::File { rel, abs, size } => {
                let dst = dest_dir.join(rel);
                if dst.exists() {
                    let resolution = if overwrite_all {
                        Resolution::Overwrite
                    } else if skip_all {
                        Resolution::Skip
                    } else {
                        match ask_conflict(tx, &dst, abs) {
                            Some(r) => r,
                            None => {
                                // Channel closed; treat as cancel.
                                return Progress::Done { copied, skipped, cancelled: true, errors };
                            }
                        }
                    };
                    match resolution {
                        Resolution::Skip => {
                            skipped += 1;
                            continue;
                        }
                        Resolution::SkipAll => {
                            skip_all = true;
                            skipped += 1;
                            continue;
                        }
                        Resolution::Overwrite => {}
                        Resolution::OverwriteAll => overwrite_all = true,
                        Resolution::Cancel => {
                            return Progress::Done { copied, skipped, cancelled: true, errors };
                        }
                    }
                }
                let total = plan.total_bytes;
                let files_total = plan.total_files;
                let mut on_bytes = |n: u64| {
                    done_bytes += n;
                    let _ = tx.send(Progress::File {
                        path: abs.clone(),
                        done_bytes,
                        total_bytes: total,
                        files_done: copied,
                        files_total,
                    });
                };
                match copy_file(abs, &dst, cancel, &mut on_bytes) {
                    Ok(()) => copied += 1,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                        return Progress::Done { copied, skipped, cancelled: true, errors };
                    }
                    Err(e) => errors.push((abs.clone(), e.to_string())),
                }
                let _ = size; // size is informational; progress uses byte totals
            }
        }
    }

    if delete_src_after && errors.is_empty() && !cancel.is_cancelled() {
        for src in srcs {
            if let Err(e) = remove_any(src) {
                errors.push((src.clone(), e.to_string()));
            }
        }
    }

    Progress::Done { copied, skipped, cancelled: false, errors }
}

/// Emit a `Conflict` event and block for the reply. `None` if the channel closed.
fn ask_conflict(tx: &Sender<Progress>, dst: &Path, src: &Path) -> Option<Resolution> {
    let (reply_tx, reply_rx) = channel::<Resolution>();
    let src_meta = std::fs::metadata(src).ok().map(|m| file_meta(&m))?;
    let dst_meta = std::fs::metadata(dst).ok().map(|m| file_meta(&m))?;
    tx.send(Progress::Conflict {
        path: dst.to_path_buf(),
        src: src_meta,
        dst: dst_meta,
        reply: reply_tx,
    })
    .ok()?;
    reply_rx.recv().ok()
}

/// Remove a file or a directory tree.
fn remove_any(p: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(p)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}
```

- [ ] **Step 4: Implement run_job in lib.rs**

Append to `crates/fsops/src/lib.rs`:
```rust
use opusdm_ipc::Op;
use std::sync::mpsc::Sender;

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
```

- [ ] **Step 5: Add temporary stubs so lib.rs compiles**

Until Task 5, add to `crates/fsops/src/moveop.rs`:
```rust
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
```
And to `crates/fsops/src/delete.rs`:
```rust
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
```
(These stubs are removed and replaced with real code in Task 5; they exist only so this task's deliverable - copy - compiles and tests independently.)

- [ ] **Step 6: Run tests, verify pass**

Run: `cargo test -p opusdm-fsops`
Expected: PASS (cancel, scan, copy tests).

- [ ] **Step 7: Commit**

```bash
git add crates/fsops CHANGES.md
git commit -m "feat(fsops): copy_file, copy_tree with conflict resolution, run_job dispatcher"
```
`CHANGES.md`: `- Task 4: opusdm-fsops copy_tree (mode/mtime preserved, conflict + cancel), run_job.`

---

## Task 5: fsops - move, delete, make_entry

**Files:**
- Modify (replace stub bodies): `crates/fsops/src/moveop.rs`, `crates/fsops/src/delete.rs`
- Test: inline `#[cfg(test)]` in each

**Interfaces:**
- Consumes: `crate::copy::copy_tree`, `crate::CancelToken`, `crate::types::Progress` (Task 4); `std::fs`
- Produces:
  - `opusdm_fsops::moveop::run_move(srcs: &[PathBuf], dest_dir: &Path, tx: &Sender<Progress>, cancel: &CancelToken) -> Progress` - tries `std::fs::rename` per source; on `EXDEV` (raw OS error 18) falls back to `copy_tree(.., delete_src_after = true)`
  - `opusdm_fsops::delete::run_delete(srcs: &[PathBuf], tx: &Sender<Progress>, cancel: &CancelToken) -> Progress` - recursive permanent delete; read-only files retried once after `chmod u+w`
  - `opusdm_fsops::delete::make_entry(dest_dir: &Path, name: &str, file: bool) -> Progress` - creates one dir (`file == false`) or one empty file; errors if it exists

- [ ] **Step 1: Write failing tests for moveop.rs**

Replace the stub test-free `moveop.rs` content's tests section by adding:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::channel;
    use std::thread;

    fn drive(srcs: Vec<PathBuf>, dest: PathBuf) -> Progress {
        let (tx, rx) = channel::<Progress>();
        let h = thread::spawn(move || run_move(&srcs, &dest, &tx, &CancelToken::new()));
        while rx.recv().is_ok() {}
        h.join().unwrap()
    }

    #[test]
    fn same_dir_rename_is_used() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("a.txt");
        let destdir = td.path().join("dst");
        fs::write(&src, b"hi").unwrap();
        fs::create_dir_all(&destdir).unwrap();

        let done = drive(vec![src.clone()], destdir.clone());
        match done {
            Progress::Done { copied, errors, cancelled, .. } => {
                assert_eq!(copied, 1);
                assert!(errors.is_empty());
                assert!(!cancelled);
            }
            _ => panic!(),
        }
        assert!(!src.exists());
        assert_eq!(fs::read(destdir.join("a.txt")).unwrap(), b"hi");
    }

    #[test]
    fn moves_a_directory() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("d");
        let destdir = td.path().join("dst");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/x"), b"y").unwrap();
        fs::create_dir_all(&destdir).unwrap();

        drive(vec![src.clone()], destdir.clone());
        assert!(!src.exists());
        assert_eq!(fs::read(destdir.join("d/sub/x")).unwrap(), b"y");
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p opusdm-fsops moveop`
Expected: FAIL - stub returns an error / wrong counts.

- [ ] **Step 3: Implement moveop.rs**

Replace the entire body of `crates/fsops/src/moveop.rs` (keep the test module from Step 1 appended):
```rust
//! Move: per-source `rename` fast-path, `EXDEV` fallback to copy-then-delete.

use crate::cancel::CancelToken;
use crate::copy::copy_tree;
use crate::types::Progress;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Linux `EXDEV`: "cross-device link". Raw OS error number.
const EXDEV: i32 = 18;

/// Move each source into `dest_dir`.
pub fn run_move(
    srcs: &[PathBuf],
    dest_dir: &Path,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    let mut cross_fs: Vec<PathBuf> = Vec::new();
    let mut moved = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();

    for src in srcs {
        if cancel.is_cancelled() {
            return Progress::Done { copied: moved, skipped: 0, cancelled: true, errors };
        }
        let Some(name) = src.file_name() else {
            errors.push((src.clone(), "source has no file name".into()));
            continue;
        };
        let target = dest_dir.join(name);
        match std::fs::rename(src, &target) {
            Ok(()) => moved += 1,
            Err(e) if e.raw_os_error() == Some(EXDEV) => cross_fs.push(src.clone()),
            Err(e) => errors.push((src.clone(), e.to_string())),
        }
    }

    if !cross_fs.is_empty() {
        // copy_tree emits its own Scanning/File events and deletes sources.
        match copy_tree(&cross_fs, dest_dir, tx, cancel, true) {
            Progress::Done { copied, skipped, cancelled, errors: mut e2 } => {
                errors.append(&mut e2);
                return Progress::Done {
                    copied: moved + copied,
                    skipped,
                    cancelled,
                    errors,
                };
            }
            other => return other,
        }
    }

    Progress::Done { copied: moved, skipped: 0, cancelled: false, errors }
}
```

- [ ] **Step 4: Write failing tests for delete.rs**

Add to `crates/fsops/src/delete.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::channel;

    fn drive_delete(srcs: Vec<PathBuf>) -> Progress {
        let (tx, rx) = channel::<Progress>();
        let h = std::thread::spawn(move || run_delete(&srcs, &tx, &CancelToken::new()));
        while rx.recv().is_ok() {}
        h.join().unwrap()
    }

    #[test]
    fn deletes_tree_including_readonly_file() {
        let td = tempfile::tempdir().unwrap();
        let d = td.path().join("victim");
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("sub/ro"), b"x").unwrap();
        fs::set_permissions(d.join("sub/ro"), fs::Permissions::from_mode(0o400)).unwrap();

        let done = drive_delete(vec![d.clone()]);
        match done {
            Progress::Done { errors, .. } => assert!(errors.is_empty(), "{errors:?}"),
            _ => panic!(),
        }
        assert!(!d.exists());
    }

    #[test]
    fn make_entry_creates_dir_and_file() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path().to_path_buf();
        assert!(matches!(make_entry(&base, "newdir", false), Progress::Done { errors, .. } if errors.is_empty()));
        assert!(base.join("newdir").is_dir());
        assert!(matches!(make_entry(&base, "newfile", true), Progress::Done { errors, .. } if errors.is_empty()));
        assert!(base.join("newfile").is_file());
    }

    #[test]
    fn make_entry_rejects_existing() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path().to_path_buf();
        fs::write(base.join("dup"), b"").unwrap();
        assert!(matches!(make_entry(&base, "dup", true), Progress::Done { errors, .. } if !errors.is_empty()));
    }
}
```

- [ ] **Step 5: Run, verify failure**

Run: `cargo test -p opusdm-fsops delete`
Expected: FAIL - stub `run_delete` never deletes; `make_entry` returns error.

- [ ] **Step 6: Implement delete.rs**

Replace the body of `crates/fsops/src/delete.rs` (keep the Step 4 test module):
```rust
//! Permanent recursive delete (no trash), and single-entry creation.

use crate::cancel::CancelToken;
use crate::types::Progress;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Recursively delete every path in `srcs`. Read-only files are retried once
/// after adding write permission. Failures are collected, not fatal.
pub fn run_delete(srcs: &[PathBuf], tx: &Sender<Progress>, cancel: &CancelToken) -> Progress {
    let mut removed = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();

    for src in srcs {
        if cancel.is_cancelled() {
            return Progress::Done { copied: removed, skipped: 0, cancelled: true, errors };
        }
        let _ = tx.send(Progress::File {
            path: src.clone(),
            done_bytes: 0,
            total_bytes: 0,
            files_done: removed,
            files_total: srcs.len() as u64,
        });
        match remove_recursive(src) {
            Ok(()) => removed += 1,
            Err(e) => errors.push((src.clone(), e.to_string())),
        }
    }

    Progress::Done { copied: removed, skipped: 0, cancelled: false, errors }
}

/// Remove a file or directory tree, chmod-retrying read-only entries once.
fn remove_recursive(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            remove_recursive(&entry?.path())?;
        }
        return std::fs::remove_dir(path);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let mut perm = meta.permissions();
            perm.set_mode(perm.mode() | 0o200);
            std::fs::set_permissions(path, perm)?;
            std::fs::remove_file(path)
        }
    }
}

/// Create one empty file (`file == true`) or one directory under `dest_dir`.
pub fn make_entry(dest_dir: &Path, name: &str, file: bool) -> Progress {
    let target = dest_dir.join(name);
    let result = if target.exists() {
        Err(format!("{} already exists", target.display()))
    } else if file {
        std::fs::File::create(&target).map(|_| ()).map_err(|e| e.to_string())
    } else {
        std::fs::create_dir(&target).map_err(|e| e.to_string())
    };
    match result {
        Ok(()) => Progress::Done { copied: 1, skipped: 0, cancelled: false, errors: Vec::new() },
        Err(msg) => Progress::Done {
            copied: 0,
            skipped: 0,
            cancelled: false,
            errors: vec![(target, msg)],
        },
    }
}
```

- [ ] **Step 7: Run full fsops suite**

Run: `cargo test -p opusdm-fsops`
Expected: PASS (all cancel/scan/copy/moveop/delete tests).

- [ ] **Step 8: Commit**

```bash
git add crates/fsops CHANGES.md
git commit -m "feat(fsops): move (rename + EXDEV fallback), recursive delete, make_entry"
```
`CHANGES.md`: `- Task 5: opusdm-fsops run_move, run_delete, make_entry. fsops crate complete.`

---

## Task 6: hub crate skeleton + role state machine

**Files:**
- Create: `crates/hub/Cargo.toml`, `crates/hub/src/main.rs` (minimal), `crates/hub/src/roles.rs`
- Test: inline `#[cfg(test)]` in `roles.rs`

**Interfaces:**
- Consumes: `opusdm_ipc::{ListerId, Role}` (Task 2)
- Produces:
  - `crate::roles::RoleState { pub source: Option<ListerId>, pub dest: Option<ListerId>, pub locked: std::collections::BTreeSet<ListerId> }` - `Clone, Default, PartialEq, Eq`
  - `crate::roles::RoleEvent` enum: `FocusGained(ListerId)`, `SetRole(ListerId, Role)`, `Lock(ListerId)`, `Unlock(ListerId)`, `Dropped(ListerId)`
  - `crate::roles::RoleEffect` enum: `Broadcast`, `Reject(ListerId)`
  - `crate::roles::apply_role_event(state: RoleState, ev: RoleEvent) -> (RoleState, Vec<RoleEffect>)`

- [ ] **Step 1: Create the crate manifest and a minimal main**

`crates/hub/Cargo.toml`:
```toml
[package]
name = "opusdm-hub"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "opusdm-hub"
path = "src/main.rs"

[dependencies]
opusdm-ipc = { path = "../ipc" }
opusdm-fsops = { path = "../fsops" }
gtk4.workspace = true
async-channel.workspace = true
signal-hook.workspace = true
libc.workspace = true
```

`crates/hub/src/main.rs` (placeholder, expanded in Tasks 7-10):
```rust
//! opusdm-hub: the always-on controller. Owns the socket, the Lister
//! registry, the global roles, and the job engine.

mod roles;

fn main() {
    eprintln!("opusdm-hub: not wired up yet (see plan Tasks 7-10)");
    std::process::exit(1);
}
```

- [ ] **Step 2: Write the role state machine tests**

`crates/hub/src/roles.rs` (test module first):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::Role;

    fn ids() -> (ListerId, ListerId, ListerId) {
        (ListerId(1), ListerId(2), ListerId(3))
    }

    #[test]
    fn first_focus_becomes_source() {
        let (a, _, _) = ids();
        let (s, fx) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        assert_eq!(s.source, Some(a));
        assert_eq!(s.dest, None);
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn second_focus_demotes_previous_source_to_dest() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, fx) = apply_role_event(s, RoleEvent::FocusGained(b));
        assert_eq!(s.source, Some(b));
        assert_eq!(s.dest, Some(a));
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn refocusing_current_source_is_a_noop() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::FocusGained(b));
        let (s2, fx) = apply_role_event(s.clone(), RoleEvent::FocusGained(b));
        assert_eq!(s2, s);
        assert!(fx.is_empty());
    }

    #[test]
    fn locked_holder_is_not_demoted_by_focus() {
        let (a, b, c) = ids();
        // a=source, then lock a.
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, fx) = apply_role_event(s, RoleEvent::Lock(a));
        assert!(fx.is_empty());
        // c gains focus: a is locked as source, so c cannot take source.
        let (s, _fx) = apply_role_event(s, RoleEvent::FocusGained(c));
        assert_eq!(s.source, Some(a), "locked source keeps its role");
        // b focus likewise cannot steal source.
        let (s, _fx) = apply_role_event(s, RoleEvent::FocusGained(b));
        assert_eq!(s.source, Some(a));
    }

    #[test]
    fn set_role_forces_dest() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, fx) = apply_role_event(s, RoleEvent::SetRole(b, Role::Dest));
        assert_eq!(s.source, Some(a));
        assert_eq!(s.dest, Some(b));
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn set_role_colliding_with_locked_holder_is_rejected() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::Lock(a));
        let (s2, fx) = apply_role_event(s.clone(), RoleEvent::SetRole(b, Role::Source));
        assert_eq!(s2, s, "state unchanged");
        assert_eq!(fx, vec![RoleEffect::Reject(b)]);
    }

    #[test]
    fn set_role_none_clears_only_that_lister() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::FocusGained(b)); // b=source, a=dest
        let (s, fx) = apply_role_event(s, RoleEvent::SetRole(b, Role::None));
        assert_eq!(s.source, None);
        assert_eq!(s.dest, Some(a));
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn dropping_a_lister_clears_its_roles_and_lock() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::FocusGained(b)); // b=source, a=dest
        let (s, _) = apply_role_event(s, RoleEvent::Lock(b));
        let (s, fx) = apply_role_event(s, RoleEvent::Dropped(b));
        assert_eq!(s.source, None);
        assert_eq!(s.dest, Some(a));
        assert!(!s.locked.contains(&b));
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn unlock_alone_does_not_move_roles() {
        let (a, _, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::Lock(a));
        let (s2, fx) = apply_role_event(s.clone(), RoleEvent::Unlock(a));
        assert_eq!(s2.source, s.source);
        assert_eq!(s2.dest, s.dest);
        assert!(fx.is_empty());
    }
}
```

- [ ] **Step 3: Run, verify failure**

Run: `cargo test -p opusdm-hub roles`
Expected: FAIL - `RoleState`, `RoleEvent`, `RoleEffect`, `apply_role_event` not found.

- [ ] **Step 4: Implement roles.rs**

Prepend to `crates/hub/src/roles.rs`:
```rust
//! The hybrid SOURCE/DEST role machine. Pure: no sockets, no GTK.
//!
//! Roles are global singletons - at most one SOURCE and one DEST across all
//! Listers, either possibly unset. Focus auto-assigns; explicit buttons force;
//! a per-Lister lock pins a role against focus changes.

use opusdm_ipc::{ListerId, Role};
use std::collections::BTreeSet;

/// Current role assignment plus the set of Listers whose role is pinned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleState {
    /// The Lister that Copy/Move read from.
    pub source: Option<ListerId>,
    /// The Lister that Copy/Move write into.
    pub dest: Option<ListerId>,
    /// Listers that pinned their current role.
    pub locked: BTreeSet<ListerId>,
}

/// Something that can change the role assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleEvent {
    /// A Lister window received focus.
    FocusGained(ListerId),
    /// A Lister pressed an explicit role button.
    SetRole(ListerId, Role),
    /// A Lister pinned its current role.
    Lock(ListerId),
    /// A Lister released its pin.
    Unlock(ListerId),
    /// A Lister disconnected.
    Dropped(ListerId),
}

/// What the caller must do after applying an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleEffect {
    /// Roles changed; send `RoleChanged` to every Lister.
    Broadcast,
    /// The request was refused; tell this Lister to re-snap its buttons.
    Reject(ListerId),
}

/// Apply one event. Returns the new state and the effects to carry out.
pub fn apply_role_event(mut s: RoleState, ev: RoleEvent) -> (RoleState, Vec<RoleEffect>) {
    let before = (s.source, s.dest);

    match ev {
        RoleEvent::Lock(id) => {
            s.locked.insert(id);
            return (s, Vec::new());
        }
        RoleEvent::Unlock(id) => {
            s.locked.remove(&id);
            return (s, Vec::new());
        }
        RoleEvent::Dropped(id) => {
            s.locked.remove(&id);
            if s.source == Some(id) {
                s.source = None;
            }
            if s.dest == Some(id) {
                s.dest = None;
            }
        }
        RoleEvent::FocusGained(id) => {
            if s.locked.contains(&id) || s.source == Some(id) {
                return (s, Vec::new());
            }
            // A locked SOURCE cannot be displaced by focus.
            if s.source.is_some_and(|cur| s.locked.contains(&cur)) {
                return (s, Vec::new());
            }
            let old_source = s.source;
            s.source = Some(id);
            if s.dest == Some(id) {
                s.dest = None;
            }
            if let Some(os) = old_source {
                if !s.locked.contains(&os) {
                    s.dest = Some(os);
                }
            }
        }
        RoleEvent::SetRole(id, role) => {
            let holder = match role {
                Role::Source => s.source,
                Role::Dest => s.dest,
                Role::None => None,
            };
            if let Some(h) = holder {
                if h != id && s.locked.contains(&h) {
                    return (s, vec![RoleEffect::Reject(id)]);
                }
            }
            if s.source == Some(id) {
                s.source = None;
            }
            if s.dest == Some(id) {
                s.dest = None;
            }
            match role {
                Role::Source => s.source = Some(id),
                Role::Dest => s.dest = Some(id),
                Role::None => {}
            }
        }
    }

    if (s.source, s.dest) == before {
        (s, Vec::new())
    } else {
        (s, vec![RoleEffect::Broadcast])
    }
}
```

- [ ] **Step 5: Run, verify pass**

Run: `cargo test -p opusdm-hub`
Expected: PASS (9 role tests). `cargo build -p opusdm-hub` also succeeds (main is a stub).

- [ ] **Step 6: Commit**

```bash
git add crates/hub CHANGES.md
git commit -m "feat(hub): hybrid SOURCE/DEST role state machine"
```
`CHANGES.md`: `- Task 6: opusdm-hub crate skeleton, apply_role_event() role machine.`

---

## Task 7: hub job engine - validation, JobId assignment, concurrency cap, progress fan-out

**Files:**
- Create: `crates/hub/src/jobs.rs`
- Modify: `crates/hub/src/main.rs` (add `mod jobs;`)
- Test: inline `#[cfg(test)]` in `jobs.rs`

**Interfaces:**
- Consumes: `opusdm_ipc::{Op, Outcome, Resolution, ToLister}`, `opusdm_fsops::{JobSpec, Progress, CancelToken}`
- Produces:
  - `crate::jobs::validate_job(srcs: &[PathBuf], dest_dir: &Path) -> Result<(), String>`
  - `crate::jobs::MAX_CONCURRENT: usize` (= 4)
  - `crate::jobs::Runner` = `std::sync::Arc<dyn Fn(JobSpec, std::sync::mpsc::Sender<(u64, Progress)>, CancelToken) + Send + Sync>` - the injection seam; the real runner wraps `opusdm_fsops::run_job` and tags each event with the job id
  - `crate::jobs::JobEngine` with:
    - `JobEngine::new(runner: Runner, progress_tx: std::sync::mpsc::Sender<(u64, Progress)>) -> Self`
    - `submit(&mut self, requester: ListerId, spec: JobSpec) -> SubmitOutcomeStarted` (returns `{ job_id: u64, started: bool }`; `started == false` means queued)
    - `on_done(&mut self, job_id: u64)` - call when a `Progress::Done` for `job_id` was observed; starts the next queued job if any, returns the started job id as `Option<u64>`
    - `cancel(&mut self, job_id: u64)` - sets that job's `CancelToken`; a queued (not started) job is dropped
    - `requester_of(&self, job_id: u64) -> Option<ListerId>`
    - `running_count(&self) -> usize`
  - `crate::jobs::default_runner() -> Runner`
  - `crate::jobs::map_progress(job_id: u64, op: &Op, ev: &Progress, totals: &mut ProgressTotals) -> Option<ToLister>` - turns a `Progress` into the `ToLister` to send the requester (`Scanning`/`File` -> `JobProgress`; `Done` -> `JobDone`; `Conflict` handled separately by the server because it needs a `req_id` and the reply channel)

- [ ] **Step 1: Write the failing tests**

`crates/hub/src/jobs.rs` (test module first):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::Op;
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Barrier};

    fn spec() -> JobSpec {
        JobSpec { op: Op::Copy, srcs: vec![PathBuf::from("/src/a")], dest_dir: PathBuf::from("/dst") }
    }

    #[test]
    fn validate_rejects_empty_sources() {
        assert!(validate_job(&[], Path::new("/dst")).is_err());
    }

    #[test]
    fn validate_rejects_dest_inside_source() {
        let src = PathBuf::from("/home/x/dir");
        assert!(validate_job(&[src.clone()], &src.join("child")).is_err());
    }

    #[test]
    fn validate_rejects_source_equals_dest() {
        let p = PathBuf::from("/home/x/dir");
        assert!(validate_job(&[p.clone()], &p).is_err());
    }

    #[test]
    fn validate_rejects_same_parent() {
        assert!(validate_job(&[PathBuf::from("/a/b/f.txt")], Path::new("/a/b")).is_err());
    }

    #[test]
    fn validate_accepts_normal_case() {
        assert!(validate_job(&[PathBuf::from("/a/b/f.txt")], Path::new("/a/c")).is_ok());
    }

    #[test]
    fn concurrency_cap_queues_beyond_four() {
        // Runner blocks on a per-job barrier so we can hold jobs "running".
        let gate = Arc::new(Barrier::new(2));
        let g2 = gate.clone();
        let runner: Runner = Arc::new(move |_spec, tx, _cancel| {
            // Signal start, then wait for the test to release, then finish.
            g2.wait();
            g2.wait();
            let _ = tx;
        });
        let (ptx, _prx) = channel::<(u64, Progress)>();
        let mut eng = JobEngine::new(runner, ptx);

        let mut ids = Vec::new();
        for _ in 0..6 {
            let r = eng.submit(ListerId(1), spec());
            ids.push(r);
        }
        let started = ids.iter().filter(|r| r.started).count();
        assert_eq!(started, MAX_CONCURRENT);
        assert_eq!(eng.running_count(), MAX_CONCURRENT);

        // Release one running job, then mark it done -> one queued job starts.
        gate.wait(); // pair the first job's first barrier
        gate.wait(); // release it
        let first_done = ids[0].job_id;
        let next = eng.on_done(first_done);
        assert!(next.is_some(), "a queued job should start");
        assert_eq!(eng.running_count(), MAX_CONCURRENT);
    }

    #[test]
    fn requester_is_tracked() {
        let runner: Runner = Arc::new(|_s, _tx, _c| {});
        let (ptx, _prx) = channel::<(u64, Progress)>();
        let mut eng = JobEngine::new(runner, ptx);
        let r = eng.submit(ListerId(7), spec());
        assert_eq!(eng.requester_of(r.job_id), Some(ListerId(7)));
    }

    #[test]
    fn map_progress_scanning_and_done() {
        let mut totals = ProgressTotals::default();
        let out = map_progress(
            3,
            &Op::Copy,
            &Progress::Scanning { files: 4, bytes: 40 },
            &mut totals,
        );
        match out {
            Some(ToLister::JobProgress { job_id, files_total, bytes_total, .. }) => {
                assert_eq!(job_id, 3);
                assert_eq!((files_total, bytes_total), (4, 40));
            }
            _ => panic!("expected JobProgress"),
        }
        let done = map_progress(
            3,
            &Op::Copy,
            &Progress::Done { copied: 4, skipped: 0, cancelled: false, errors: vec![] },
            &mut totals,
        );
        assert!(matches!(done, Some(ToLister::JobDone { job_id: 3, .. })));
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p opusdm-hub jobs`
Expected: FAIL - none of the job-engine items exist.

- [ ] **Step 3: Implement jobs.rs**

Prepend to `crates/hub/src/jobs.rs`:
```rust
//! The hub's job engine. Validates requests, assigns ids, caps concurrency at
//! `MAX_CONCURRENT`, and turns `opusdm_fsops::Progress` into `ToLister`
//! messages. All file I/O runs on `std::thread`s spawned here.

use opusdm_fsops::{CancelToken, JobSpec, Progress};
use opusdm_ipc::{ListerId, Op, Outcome, ToLister};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Most jobs allowed to run at once; the rest queue.
pub const MAX_CONCURRENT: usize = 4;

/// The function that actually performs a job. Injected so tests can fake it.
/// It must forward every `opusdm_fsops::Progress` as `(job_id, progress)` and
/// end with exactly one `Progress::Done`.
pub type Runner = Arc<dyn Fn(JobSpec, Sender<(u64, Progress)>, CancelToken) + Send + Sync>;

/// Result of [`JobEngine::submit`].
pub struct SubmitOutcomeStarted {
    /// Id assigned to the job.
    pub job_id: u64,
    /// `true` if it started now, `false` if it was queued.
    pub started: bool,
}

/// Reject obviously bad job parameters before scanning. `Ok(())` means proceed.
pub fn validate_job(srcs: &[PathBuf], dest_dir: &Path) -> Result<(), String> {
    if srcs.is_empty() {
        return Err("no source files selected".into());
    }
    for s in srcs {
        if s == dest_dir {
            return Err(format!("source and destination are the same: {}", s.display()));
        }
        if dest_dir.starts_with(s) {
            return Err(format!("destination is inside the source: {}", s.display()));
        }
        if s.parent() == Some(dest_dir) {
            return Err("selected items are already in the destination".into());
        }
    }
    Ok(())
}

struct RunningJob {
    requester: ListerId,
    cancel: CancelToken,
}

struct QueuedJob {
    job_id: u64,
    requester: ListerId,
    spec: JobSpec,
}

/// Owns job bookkeeping. Not `Sync`; lives on the hub's owner thread.
pub struct JobEngine {
    runner: Runner,
    progress_tx: Sender<(u64, Progress)>,
    running: HashMap<u64, RunningJob>,
    queue: VecDeque<QueuedJob>,
    next_id: u64,
}

impl JobEngine {
    /// Create an engine that runs jobs via `runner`, forwarding progress on `progress_tx`.
    pub fn new(runner: Runner, progress_tx: Sender<(u64, Progress)>) -> Self {
        Self {
            runner,
            progress_tx,
            running: HashMap::new(),
            queue: VecDeque::new(),
            next_id: 1,
        }
    }

    /// Number of jobs currently executing.
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Which Lister asked for `job_id`, if the engine still knows.
    pub fn requester_of(&self, job_id: u64) -> Option<ListerId> {
        self.running
            .get(&job_id)
            .map(|r| r.requester)
            .or_else(|| self.queue.iter().find(|q| q.job_id == job_id).map(|q| q.requester))
    }

    /// Accept a job. Starts it if under the cap, else queues it.
    pub fn submit(&mut self, requester: ListerId, spec: JobSpec) -> SubmitOutcomeStarted {
        let job_id = self.next_id;
        self.next_id += 1;
        if self.running.len() < MAX_CONCURRENT {
            self.start(job_id, requester, spec);
            SubmitOutcomeStarted { job_id, started: true }
        } else {
            self.queue.push_back(QueuedJob { job_id, requester, spec });
            SubmitOutcomeStarted { job_id, started: false }
        }
    }

    /// Note that `job_id` finished; start the next queued job if any.
    /// Returns the id of the job that was started, if one was.
    pub fn on_done(&mut self, job_id: u64) -> Option<u64> {
        self.running.remove(&job_id);
        let QueuedJob { job_id: next_id, requester, spec } = self.queue.pop_front()?;
        self.start(next_id, requester, spec);
        Some(next_id)
    }

    /// Cancel a job. A running job's token is set; a queued job is discarded.
    pub fn cancel(&mut self, job_id: u64) {
        if let Some(r) = self.running.get(&job_id) {
            r.cancel.cancel();
            return;
        }
        self.queue.retain(|q| q.job_id != job_id);
    }

    fn start(&mut self, job_id: u64, requester: ListerId, spec: JobSpec) {
        let cancel = CancelToken::new();
        self.running.insert(job_id, RunningJob { requester, cancel: cancel.clone() });
        let runner = Arc::clone(&self.runner);
        let tx = self.progress_tx.clone();
        std::thread::Builder::new()
            .name(format!("opusdm-job-{job_id}"))
            .spawn(move || runner(spec, tx, cancel))
            .expect("spawn job thread");
    }
}

/// The production runner: `opusdm_fsops::run_job` with each event tagged by id.
pub fn default_runner() -> Runner {
    Arc::new(|spec: JobSpec, tx: Sender<(u64, Progress)>, cancel: CancelToken| {
        // job id is threaded in by the closure captured per-call in `start`;
        // here we cannot know it, so `start` wraps this. See note below.
        let (inner_tx, inner_rx) = std::sync::mpsc::channel::<Progress>();
        let forward = std::thread::spawn(move || {
            for ev in inner_rx {
                // job id 0 is a placeholder; `JobEngine::start` uses a wrapper
                // that rewrites it. Kept simple: see Step 4.
                if tx.send((0, ev)).is_err() {
                    break;
                }
            }
        });
        opusdm_fsops::run_job(spec, inner_tx, cancel);
        let _ = forward.join();
    })
}

/// Running byte/file totals learned from the `Scanning` event, reused to fill
/// later `JobProgress` messages.
#[derive(Default)]
pub struct ProgressTotals {
    /// Total bytes to process (from `Scanning`).
    pub bytes_total: u64,
    /// Total files to process (from `Scanning`).
    pub files_total: u64,
}

/// Convert a `Progress` into the `ToLister` message for the requester.
/// `Conflict` returns `None` - the server layer handles it (it owns `req_id`
/// and the reply channel).
pub fn map_progress(
    job_id: u64,
    op: &Op,
    ev: &Progress,
    totals: &mut ProgressTotals,
) -> Option<ToLister> {
    match ev {
        Progress::Scanning { files, bytes } => {
            totals.files_total = *files;
            totals.bytes_total = *bytes;
            Some(ToLister::JobProgress {
                job_id,
                op: op.clone(),
                current_file: PathBuf::new(),
                bytes_done: 0,
                bytes_total: *bytes,
                files_done: 0,
                files_total: *files,
            })
        }
        Progress::File { path, done_bytes, files_done, .. } => Some(ToLister::JobProgress {
            job_id,
            op: op.clone(),
            current_file: path.clone(),
            bytes_done: *done_bytes,
            bytes_total: totals.bytes_total,
            files_done: *files_done,
            files_total: totals.files_total,
        }),
        Progress::Conflict { .. } => None,
        Progress::Done { copied, skipped, cancelled, errors } => {
            let outcome = if *cancelled {
                Outcome::Cancelled { copied: *copied, skipped: *skipped }
            } else {
                Outcome::Ok { copied: *copied, skipped: *skipped }
            };
            Some(ToLister::JobDone { job_id, outcome, errors: errors.clone() })
        }
    }
}
```

- [ ] **Step 4: Fix the job-id tagging in `start`**

The `Runner` type receives `Sender<(u64, Progress)>` but `default_runner` cannot
know the id. Resolve by having `JobEngine::start` pass a per-job wrapper sender
instead of the shared one. Replace `start`'s body:
```rust
    fn start(&mut self, job_id: u64, requester: ListerId, spec: JobSpec) {
        let cancel = CancelToken::new();
        self.running.insert(job_id, RunningJob { requester, cancel: cancel.clone() });
        let runner = Arc::clone(&self.runner);
        let shared_tx = self.progress_tx.clone();
        std::thread::Builder::new()
            .name(format!("opusdm-job-{job_id}"))
            .spawn(move || {
                // Re-tag every event with this job's id.
                let (job_tx, job_rx) = std::sync::mpsc::channel::<(u64, Progress)>();
                let pump = std::thread::spawn(move || {
                    for (_placeholder, ev) in job_rx {
                        if shared_tx.send((job_id, ev)).is_err() {
                            break;
                        }
                    }
                });
                runner(spec, job_tx, cancel);
                let _ = pump.join();
            })
            .expect("spawn job thread");
    }
```
Now `default_runner` can send `(0, ev)` and the wrapper rewrites it to `job_id`.

- [ ] **Step 5: Register the module**

`crates/hub/src/main.rs`: add `mod jobs;` next to `mod roles;`.

- [ ] **Step 6: Run tests, verify pass**

Run: `cargo test -p opusdm-hub`
Expected: PASS (role tests + job tests).

- [ ] **Step 7: Commit**

```bash
git add crates/hub CHANGES.md
git commit -m "feat(hub): job engine - validation, concurrency cap, progress mapping"
```
`CHANGES.md`: `- Task 7: opusdm-hub JobEngine (validate, MAX_CONCURRENT=4, map_progress).`

---

## Task 8: hub socket server - listener, per-Lister reader threads, owner loop

**Files:**
- Create: `crates/hub/src/server.rs`
- Modify: `crates/hub/src/main.rs` (add `mod server;`)
- Test: inline `#[cfg(test)]` in `server.rs` (uses real `UnixListener`/`UnixStream` over a tempdir socket, no GTK)

**Interfaces:**
- Consumes: `crate::roles::{RoleState, RoleEvent, RoleEffect, apply_role_event}`, `crate::jobs::{JobEngine, validate_job, default_runner, map_progress, ProgressTotals}`, `opusdm_ipc::{frame, ListerId, ToHub, ToLister, Role, Op}`, `opusdm_fsops::Progress`
- Produces:
  - `crate::server::HubEvent` enum (server -> GTK): `ListerRegistered { id: ListerId, pid: u32, path: PathBuf }`, `ListerGone(ListerId)`, `Roles { source: Option<ListerId>, dest: Option<ListerId>, paths: Vec<(ListerId, PathBuf)> }`, `JobUpdate { job_id: u64, label: String, done: u64, total: u64 }`, `JobFinished { job_id: u64, ok: bool, errors: usize }`, `AllListersGone`
  - `crate::server::HubCmd` enum (GTK -> server): `SpawnRequested { start_path: Option<PathBuf> }`, `CancelJob(u64)`, `PresentLister(ListerId)`, `Shutdown`
  - `crate::server::ServerHandle { pub cmd_tx: std::sync::mpsc::Sender<HubCmd>, pub evt_rx: std::sync::mpsc::Receiver<HubEvent> }`
  - `crate::server::spawn_server(listener: std::os::unix::net::UnixListener, on_spawn: SpawnFn) -> ServerHandle` where `SpawnFn = Box<dyn Fn(Option<PathBuf>) + Send>` (called when a Lister or the GTK toolbar asks for a new Lister; the real one is `children::spawn_lister`, tests pass a counter)
  - `crate::server::child_exited(handle: &ServerHandle, pid: u32)` - the SIGCHLD path (Task 9) calls this to drop a Lister by pid

**Design notes for the implementer:**
- One `std::sync::mpsc` channel `OwnerMsg` merges every input into a single owner loop:
  ```rust
  enum OwnerMsg {
      Accepted(UnixStream),
      FromLister(ListerId, ToHub),
      ListerEof(ListerId),
      JobProgress(u64, Progress),
      FromGtk(HubCmd),
      ChildPid(u32),
  }
  ```
- Accept loop: a thread does `listener.incoming()` and sends `OwnerMsg::Accepted`.
- Per connection: the owner assigns a `ListerId` on `Register`, stores a `ListerConn { write: UnixStream (try_clone of the accepted stream), pid, path }`, and spawns a reader thread that loops `frame::recv::<ToHub>` -> `OwnerMsg::FromLister` / `OwnerMsg::ListerEof`.
- Writing to a Lister: `frame::send(&mut conn.write, &msg)`; on error, treat as EOF for that Lister.
- Roles: keep a `RoleState`; feed `RoleEvent`s; on `RoleEffect::Broadcast` send `ToLister::RoleChanged` to all, and `HubEvent::Roles` to GTK; on `RoleEffect::Reject(id)` send that one Lister a `RoleChanged` with the unchanged roles.
- Jobs: on `ToHub::RunJob`, run `validate_job`; on error send `ToLister::JobDone { Outcome::Error }`; on ok `engine.submit`. Keep a `HashMap<u64, (ListerId, Op, ProgressTotals, next_req_id)>`. On `OwnerMsg::JobProgress`:
  - `Progress::Conflict { path, src, dst, reply }` -> allocate a `req_id`, stash `reply` in `HashMap<u64 /*req_id*/, Sender<Resolution>>`, send `ToLister::ConflictPrompt` to the job's requester.
  - else `map_progress(..)` -> send to requester; also emit `HubEvent::JobUpdate`.
  - `Progress::Done` -> also `engine.on_done(job_id)`, emit `HubEvent::JobFinished`, and broadcast `ToLister::InvalidatePath(dest_dir)` (+ each source parent for Move/Delete) to all Listers.
- `ToHub::ConflictReply { req_id, resolution }` -> `reply.send(resolution)`.
- `ToHub::SpawnLister { start_path }` and `HubCmd::SpawnRequested` -> call `on_spawn`.
- `ToHub::Goodbye` / `ListerEof` / `ChildPid` matching a known pid -> remove the conn, feed `RoleEvent::Dropped(id)`, emit `HubEvent::ListerGone`; if no Listers remain, emit `HubEvent::AllListersGone`.
- `ToHub::Ping` -> reply `ToLister::Pong` (this path is also how `probe_live_hub` works; a Ping connection never sends `Register`, so give it a transient id and drop it after replying).
- `ToHub::RaiseToolbar` -> emit a `HubEvent` variant the toolbar listens on; add `HubEvent::RaiseToolbar` to the enum.
- `HubCmd::Shutdown` -> broadcast `ToLister::Shutdown`, then return from the owner loop (caller exits the process).

- [ ] **Step 1: Write the integration test**

`crates/hub/src/server.rs` (test module first):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::frame::{recv, send};
    use opusdm_ipc::{ToHub, ToLister};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn bind_temp() -> (tempfile::TempDir, UnixListener, std::path::PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let l = UnixListener::bind(&path).unwrap();
        (td, l, path)
    }

    fn connect(path: &std::path::Path) -> UnixStream {
        let s = UnixStream::connect(path).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s
    }

    #[test]
    fn register_gets_welcome_and_focus_makes_source() {
        let (_td, listener, path) = bind_temp();
        let spawns = Arc::new(AtomicUsize::new(0));
        let s2 = spawns.clone();
        let handle = spawn_server(listener, Box::new(move |_p| { s2.fetch_add(1, Ordering::SeqCst); }));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1000, initial_path: "/home/a".into() }).unwrap();
        let welcome: ToLister = recv(&mut a).unwrap().unwrap();
        let id_a = match welcome {
            ToLister::Welcome { lister_id, source, dest } => {
                assert_eq!(source, None);
                assert_eq!(dest, None);
                lister_id
            }
            other => panic!("expected Welcome, got {other:?}"),
        };

        send(&mut a, &ToHub::FocusGained).unwrap();
        let rc: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(rc, ToLister::RoleChanged { source, .. } if source == Some(id_a)));

        // GTK side saw the registration and the role change.
        let mut saw_reg = false;
        while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(200)) {
            if matches!(ev, HubEvent::ListerRegistered { .. }) { saw_reg = true; }
        }
        assert!(saw_reg);
    }

    #[test]
    fn ping_gets_pong_without_registering() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut c = connect(&path);
        send(&mut c, &ToHub::Ping).unwrap();
        assert!(matches!(recv::<_, ToLister>(&mut c).unwrap(), Some(ToLister::Pong)));
        drop(handle);
    }

    #[test]
    fn two_listers_focus_toggles_source_dest() {
        let (_td, listener, path) = bind_temp();
        let _h = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        let mut b = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _wa: ToLister = recv(&mut a).unwrap().unwrap();
        send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into() }).unwrap();
        let _wb: ToLister = recv(&mut b).unwrap().unwrap();

        send(&mut a, &ToHub::FocusGained).unwrap();
        let _ra: ToLister = recv(&mut a).unwrap().unwrap(); // a broadcast
        let _rb: ToLister = recv(&mut b).unwrap().unwrap();
        send(&mut b, &ToHub::FocusGained).unwrap();
        let rb2: ToLister = recv(&mut b).unwrap().unwrap();
        match rb2 {
            ToLister::RoleChanged { source, dest } => {
                assert!(source.is_some());
                assert!(dest.is_some());
                assert_ne!(source, dest);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn goodbye_drops_lister_and_notifies_gtk() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 9, initial_path: "/a".into() }).unwrap();
        let _wa: ToLister = recv(&mut a).unwrap().unwrap();
        send(&mut a, &ToHub::Goodbye).unwrap();
        drop(a);
        let mut saw_gone = false;
        let mut saw_all_gone = false;
        while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(300)) {
            match ev {
                HubEvent::ListerGone(_) => saw_gone = true,
                HubEvent::AllListersGone => saw_all_gone = true,
                _ => {}
            }
        }
        assert!(saw_gone && saw_all_gone);
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p opusdm-hub server`
Expected: FAIL - `spawn_server`, `HubEvent`, `HubCmd`, `ServerHandle` not found.

- [ ] **Step 3: Implement server.rs**

Implement per the design notes above. The owner loop is a single `for msg in owner_rx` over the merged `OwnerMsg` channel; helper functions `broadcast(&mut conns, &msg)`, `send_to(&mut conns, id, &msg)`, `apply_role(&mut state, ev, &mut conns, &evt_tx)`. `spawn_server`:
```rust
/// Start the socket server on `listener`. Returns channels to/from the GTK side.
pub fn spawn_server(listener: std::os::unix::net::UnixListener, on_spawn: SpawnFn) -> ServerHandle {
    let (owner_tx, owner_rx) = std::sync::mpsc::channel::<OwnerMsg>();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel::<HubEvent>();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<HubCmd>();

    // Accept loop.
    {
        let owner_tx = owner_tx.clone();
        std::thread::Builder::new().name("opusdm-accept".into()).spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => { if owner_tx.send(OwnerMsg::Accepted(s)).is_err() { break; } }
                    Err(_) => break,
                }
            }
        }).expect("spawn accept thread");
    }
    // GTK command forwarder.
    {
        let owner_tx = owner_tx.clone();
        std::thread::spawn(move || {
            for cmd in cmd_rx { if owner_tx.send(OwnerMsg::FromGtk(cmd)).is_err() { break; } }
        });
    }
    // Owner loop.
    {
        let evt_tx = evt_tx.clone();
        std::thread::Builder::new().name("opusdm-owner".into()).spawn(move || {
            owner_loop(owner_rx, owner_tx, evt_tx, on_spawn);
        }).expect("spawn owner thread");
    }

    ServerHandle { cmd_tx, evt_rx }
}
```
`owner_loop` holds all mutable state locally (`conns: HashMap<ListerId, ListerConn>`, `RoleState`, `JobEngine`, `req replies`, `next_id`). It never shares state across threads - only channels cross thread boundaries.

For `child_exited`, add a `HubCmd::ChildExited(u32)` variant instead of a free function (simpler; Task 9 sends it through `cmd_tx`). Update the `HubCmd` interface list accordingly.

- [ ] **Step 4: Register the module and run tests**

`crates/hub/src/main.rs`: add `mod server;`.
Run: `cargo test -p opusdm-hub -- --test-threads=1`
Expected: PASS (roles + jobs + 4 server tests). Single-threaded because each server test binds and the accept threads linger until tempdir drop.

- [ ] **Step 5: Commit**

```bash
git add crates/hub CHANGES.md
git commit -m "feat(hub): socket server owner loop, per-Lister reader threads, role/job wiring"
```
`CHANGES.md`: `- Task 8: opusdm-hub server.rs - accept loop, owner loop, HubEvent/HubCmd bridge.`

---

## Task 9: hub startup - single-instance bind, child spawn, SIGCHLD reaping

**Files:**
- Create: `crates/hub/src/children.rs`
- Modify: `crates/hub/src/main.rs` (real startup sequence; still headless - GTK added in Task 10)
- Test: inline `#[cfg(test)]` in `children.rs`; a manual run check for the startup path

**Interfaces:**
- Consumes: `opusdm_ipc::sockpath::{hub_sock_path, ensure_sock_dir, probe_live_hub, ENV_SOCK}`, `opusdm_ipc::frame::send`, `opusdm_ipc::ToHub`, `crate::server::{spawn_server, ServerHandle, HubCmd, HubEvent}`
- Produces:
  - `crate::children::spawn_lister(sock_path: &Path, start_path: Option<&Path>) -> std::io::Result<u32>` - `Command::new(current_exe_dir/"opusdm-lister")`, env `OPUSDM_HUB_SOCK=sock_path`, arg `start_path` if given, returns child pid; does **not** wait
  - `crate::children::install_sigchld(cmd_tx: std::sync::mpsc::Sender<HubCmd>)` - spawns a thread on `signal_hook::iterator::Signals::new([SIGCHLD])`; for each signal, `libc::waitpid(-1, &mut status, WNOHANG)` in a loop, sending `HubCmd::ChildExited(pid as u32)` per reaped child
  - `crate::children::AcquireOutcome` enum: `Bound(std::os::unix::net::UnixListener)`, `HandedOff` (a live hub already runs; this process asked it to raise and should exit 0), `Failed(String)`
  - `crate::children::acquire_socket(path: &Path) -> AcquireOutcome` - `ensure_sock_dir`; if `probe_live_hub` connect + `send(ToHub::RaiseToolbar)` -> `HandedOff`; else `remove_file` (ignore NotFound) + `UnixListener::bind` -> `Bound` / `Failed`

- [ ] **Step 1: Write tests for children.rs**

`crates/hub/src/children.rs` (test module first):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_binds_when_no_hub_present() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        match acquire_socket(&path) {
            AcquireOutcome::Bound(_l) => {}
            other => panic!("expected Bound, got {other:?}"),
        }
        assert!(path.exists());
    }

    #[test]
    fn acquire_replaces_a_stale_socket_file() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        // A leftover socket file with nobody listening.
        let _stale = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(_stale);
        // File may or may not remain after drop; force the stale case.
        std::fs::write(&path, b"").ok();
        match acquire_socket(&path) {
            AcquireOutcome::Bound(_l) => {}
            other => panic!("expected Bound after clearing stale, got {other:?}"),
        }
    }

    #[test]
    fn acquire_hands_off_to_a_live_hub() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        // Stand up a real server so probe_live_hub answers Pong.
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let _h = crate::server::spawn_server(listener, Box::new(|_p| {}));
        match acquire_socket(&path) {
            AcquireOutcome::HandedOff => {}
            other => panic!("expected HandedOff, got {other:?}"),
        }
    }

    #[test]
    fn spawn_lister_reports_missing_binary_cleanly() {
        // In a scratch dir with no opusdm-lister next to a fake exe path,
        // spawn_lister must return Err, not panic.
        let td = tempfile::tempdir().unwrap();
        let sock = td.path().join("hub.sock");
        let res = spawn_lister_from_dir(td.path(), &sock, None);
        assert!(res.is_err());
    }
}
```
Note: `spawn_lister` resolves the sibling binary from `std::env::current_exe()`. Add a testable inner `spawn_lister_from_dir(dir: &Path, sock_path: &Path, start_path: Option<&Path>) -> io::Result<u32>` that `spawn_lister` calls with `current_exe().parent()`.

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p opusdm-hub children`
Expected: FAIL - items not found.

- [ ] **Step 3: Implement children.rs**

```rust
//! Spawning Lister child processes and reaping them on SIGCHLD, plus the
//! single-instance socket acquisition.

use crate::server::HubCmd;
use opusdm_ipc::frame::send;
use opusdm_ipc::sockpath::{ensure_sock_dir, probe_live_hub};
use opusdm_ipc::ToHub;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::Sender;

/// Result of trying to become the hub for a given socket path.
#[derive(Debug)]
pub enum AcquireOutcome {
    /// We own the socket now.
    Bound(UnixListener),
    /// A live hub already runs; we asked it to raise its toolbar. Exit 0.
    HandedOff,
    /// Could not bind and no live hub answered.
    Failed(String),
}

/// Ensure the socket dir, hand off to a live hub, or bind the socket.
pub fn acquire_socket(path: &Path) -> AcquireOutcome {
    if let Err(e) = ensure_sock_dir(path) {
        return AcquireOutcome::Failed(format!("cannot create socket dir: {e}"));
    }
    if probe_live_hub(path) {
        if let Ok(mut s) = UnixStream::connect(path) {
            let _ = send(&mut s, &ToHub::RaiseToolbar);
        }
        return AcquireOutcome::HandedOff;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return AcquireOutcome::Failed(format!("cannot remove stale socket: {e}")),
    }
    match UnixListener::bind(path) {
        Ok(l) => AcquireOutcome::Bound(l),
        Err(e) => AcquireOutcome::Failed(format!("cannot bind {}: {e}", path.display())),
    }
}

/// Launch one `opusdm-lister`, found next to the running hub binary.
pub fn spawn_lister(sock_path: &Path, start_path: Option<&Path>) -> io::Result<u32> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| io::Error::other("no exe dir"))?;
    spawn_lister_from_dir(dir, sock_path, start_path)
}

/// Testable core of [`spawn_lister`].
pub fn spawn_lister_from_dir(
    dir: &Path,
    sock_path: &Path,
    start_path: Option<&Path>,
) -> io::Result<u32> {
    let bin = dir.join("opusdm-lister");
    if !bin.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("opusdm-lister not found at {}", bin.display()),
        ));
    }
    let mut cmd = Command::new(bin);
    cmd.env(opusdm_ipc::sockpath::ENV_SOCK, sock_path);
    if let Some(p) = start_path {
        cmd.arg(p);
    }
    Ok(cmd.spawn()?.id())
}

/// Reap exited children and notify the owner loop by pid.
pub fn install_sigchld(cmd_tx: Sender<HubCmd>) {
    use signal_hook::consts::SIGCHLD;
    use signal_hook::iterator::Signals;
    std::thread::Builder::new()
        .name("opusdm-sigchld".into())
        .spawn(move || {
            let mut signals = match Signals::new([SIGCHLD]) {
                Ok(s) => s,
                Err(_) => return,
            };
            for _ in &mut signals {
                loop {
                    let mut status: libc::c_int = 0;
                    // SAFETY: waitpid with WNOHANG on any child; status is a valid out-ptr.
                    let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                    if pid <= 0 {
                        break;
                    }
                    if cmd_tx.send(HubCmd::ChildExited(pid as u32)).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("spawn sigchld thread");
}
```

- [ ] **Step 4: Wire main.rs (headless)**

`crates/hub/src/main.rs`:
```rust
//! opusdm-hub: the always-on controller.

mod children;
mod jobs;
mod roles;
mod server;

use children::AcquireOutcome;
use opusdm_ipc::sockpath::hub_sock_path;
use server::{HubCmd, HubEvent};

fn main() {
    let sock_path = hub_sock_path();
    let listener = match children::acquire_socket(&sock_path) {
        AcquireOutcome::Bound(l) => l,
        AcquireOutcome::HandedOff => {
            eprintln!("opusdm-hub already running; asked it to raise its toolbar");
            std::process::exit(0);
        }
        AcquireOutcome::Failed(msg) => {
            eprintln!("opusdm-hub: {msg}");
            std::process::exit(1);
        }
    };

    let sock_for_spawn = sock_path.clone();
    let handle = server::spawn_server(
        listener,
        Box::new(move |start: Option<std::path::PathBuf>| {
            if let Err(e) = children::spawn_lister(&sock_for_spawn, start.as_deref()) {
                eprintln!("opusdm-hub: cannot spawn lister: {e}");
            }
        }),
    );

    children::install_sigchld(handle.cmd_tx.clone());

    // Open the first Lister at $HOME.
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if let Err(e) = children::spawn_lister(&sock_path, home.as_deref()) {
        eprintln!("opusdm-hub: cannot spawn initial lister: {e}");
    }

    // Headless event drain until AllListersGone is followed by an explicit quit.
    // Task 10 replaces this block with the GTK toolbar main loop.
    for ev in handle.evt_rx.iter() {
        match ev {
            HubEvent::ListerRegistered { id, pid, path } => {
                eprintln!("[hub] lister {id:?} pid={pid} at {}", path.display());
            }
            HubEvent::ListerGone(id) => eprintln!("[hub] lister {id:?} gone"),
            HubEvent::AllListersGone => eprintln!("[hub] no listers open (toolbar still up in v10)"),
            HubEvent::Roles { source, dest, .. } => {
                eprintln!("[hub] roles: source={source:?} dest={dest:?}");
            }
            HubEvent::JobUpdate { job_id, label, done, total } => {
                eprintln!("[hub] job {job_id}: {label} {done}/{total}");
            }
            HubEvent::JobFinished { job_id, ok, errors } => {
                eprintln!("[hub] job {job_id} done ok={ok} errors={errors}");
            }
            HubEvent::RaiseToolbar => eprintln!("[hub] raise-toolbar request"),
        }
    }
    let _ = handle.cmd_tx.send(HubCmd::Shutdown);
}
```

- [ ] **Step 5: Run tests + manual startup check**

Run: `cargo test -p opusdm-hub -- --test-threads=1` -> PASS.
Run: `cargo build --workspace` -> succeeds (lister is still a stub crate; create `crates/lister/Cargo.toml` + `src/main.rs` with a `fn main() { eprintln!("stub"); }` now if the workspace does not yet build, and note it will be replaced in Task 11).
Manual: `cargo run -p opusdm-hub` in a terminal. Expected stderr: "opusdm-lister not found" (until Task 11) or, once Task 11 exists, a `[hub] lister ... ` line. `Ctrl-C` to stop. Second `cargo run -p opusdm-hub` while the first runs -> "already running" and exit 0.

- [ ] **Step 6: Commit**

```bash
git add crates/hub crates/lister CHANGES.md
git commit -m "feat(hub): single-instance socket acquisition, child spawn, SIGCHLD reaping"
```
`CHANGES.md`: `- Task 9: opusdm-hub startup - acquire_socket, spawn_lister, install_sigchld; headless main.`

---

## Task 10: hub GTK toolbar window

**Files:**
- Create: `crates/hub/src/toolbar.rs`
- Modify: `crates/hub/src/main.rs` (replace the headless drain with the GTK app)
- Test: no automated UI test (spec exempts UI). Manual checklist below + `cargo build`.

**Interfaces:**
- Consumes: `crate::server::{ServerHandle, HubEvent, HubCmd}`, `gtk4` (as `gtk`), `async_channel`
- Produces:
  - `crate::toolbar::run(handle: ServerHandle, sock_path: std::path::PathBuf) -> gtk::glib::ExitCode` - builds the `gtk::Application`, the toolbar `ApplicationWindow`, wires events, runs the main loop; returns when the app quits

**Design notes:**
- `main()` moves `handle.evt_rx` (an `mpsc::Receiver`, not `Send`-friendly across the GTK loop) onto a bridge: spawn one thread that does `for ev in evt_rx { ui_tx.send_blocking(ev) }` where `(ui_tx, ui_rx) = async_channel::unbounded::<HubEvent>()`. In the GTK app's `activate`, `glib::MainContext::default().spawn_local(async move { while let Ok(ev) = ui_rx.recv().await { apply(&widgets, ev) } })`.
- Toolbar window: `ApplicationWindow`, `set_decorated(true)`, `set_resizable(false)`, a single `gtk::Box` (horizontal, 6px spacing, 6px margins):
  - `gtk::Button` "New Lister" -> `handle.cmd_tx.send(HubCmd::SpawnRequested { start_path: None })`.
  - `gtk::Label` `src_label` (prefix "SRC: "), `gtk::Label` `dst_label` (prefix "DST: "). A `gtk::GestureClick` on each: on release send `HubCmd::PresentLister(id)` for the currently shown id (store `Cell<Option<ListerId>>` per label).
  - `gtk::Box` `job_box` (vertical) - one child per active job: a horizontal box with a `gtk::Label` (op + "N/M") + `gtk::ProgressBar` + `gtk::Button` "Cancel" (-> `HubCmd::CancelJob(job_id)`). Keep a `HashMap<u64, JobRow>`; add on first `JobUpdate`, update fraction, remove on `JobFinished` unless `errors > 0` (then turn the label red and keep it until clicked).
  - `gtk::Button` "Quit" -> if `!jobs.is_empty()` show a `gtk::AlertDialog` ("N jobs running. Quit anyway?"); on confirm (or if no jobs) `handle.cmd_tx.send(HubCmd::Shutdown)` then `app.quit()`.
- On `HubEvent::Roles { source, dest, paths }`: set `src_label`/`dst_label` text to the short path (`paths` lookup; last path component or "~") or "-"; store ids for the click gesture.
- On `HubEvent::AllListersGone`: leave the toolbar open (per spec).
- On `HubEvent::RaiseToolbar`: `window.present()`.
- The window's close button (`connect_close_request`): same as "Quit".
- After building, if this is the first run, the initial Lister was already spawned in `main()` before `toolbar::run`.

- [ ] **Step 1: Implement toolbar.rs**

Write `crates/hub/src/toolbar.rs` per the notes. Skeleton:
```rust
//! The always-on toolbar: New Lister, SRC/DST indicators, job progress, Quit.

use crate::server::{HubCmd, HubEvent, ServerHandle};
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{glib, Application, ApplicationWindow};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Build and run the toolbar application. Returns on quit.
pub fn run(handle: ServerHandle, _sock_path: std::path::PathBuf) -> glib::ExitCode {
    let app = Application::builder()
        .application_id("org.opusdm.Hub")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let (ui_tx, ui_rx) = async_channel::unbounded::<HubEvent>();
    // Bridge the mpsc receiver onto the async channel from a plain thread.
    let evt_rx = handle.evt_rx;
    std::thread::spawn(move || {
        for ev in evt_rx.iter() {
            if ui_tx.send_blocking(ev).is_err() {
                break;
            }
        }
    });

    let cmd_tx = handle.cmd_tx.clone();
    app.connect_activate(move |app| {
        build_ui(app, cmd_tx.clone(), ui_rx.clone());
    });
    // No CLI args for the GUI app.
    app.run_with_args::<&str>(&[])
}

struct Widgets {
    window: ApplicationWindow,
    src_label: gtk::Label,
    dst_label: gtk::Label,
    src_id: Rc<Cell<Option<opusdm_ipc::ListerId>>>,
    dst_id: Rc<Cell<Option<opusdm_ipc::ListerId>>>,
    job_box: gtk::Box,
    jobs: Rc<RefCell<HashMap<u64, JobRow>>>,
    cmd_tx: std::sync::mpsc::Sender<HubCmd>,
}

struct JobRow {
    root: gtk::Box,
    label: gtk::Label,
    bar: gtk::ProgressBar,
    had_errors: bool,
}

fn build_ui(
    app: &Application,
    cmd_tx: std::sync::mpsc::Sender<HubCmd>,
    ui_rx: async_channel::Receiver<HubEvent>,
) {
    // ... construct widgets per the design notes ...
    // Spawn the event pump:
    // glib::MainContext::default().spawn_local(async move {
    //     while let Ok(ev) = ui_rx.recv().await { apply_event(&widgets, ev); }
    // });
    let _ = (app, cmd_tx, ui_rx);
    unimplemented!("fill in per design notes")
}

fn apply_event(w: &Widgets, ev: HubEvent) {
    match ev {
        HubEvent::Roles { source, dest, paths } => { /* set labels + stored ids */ }
        HubEvent::JobUpdate { job_id, label, done, total } => { /* add/update JobRow */ }
        HubEvent::JobFinished { job_id, ok, errors } => { /* remove or mark red */ }
        HubEvent::RaiseToolbar => w.window.present(),
        HubEvent::ListerRegistered { .. }
        | HubEvent::ListerGone(_)
        | HubEvent::AllListersGone => {}
    }
    let _ = w;
}
```
The implementer fills `build_ui`/`apply_event` fully - no `unimplemented!` may remain at commit.

- [ ] **Step 2: Wire main.rs**

Replace the headless `for ev in handle.evt_rx.iter()` block with:
```rust
    let code = toolbar::run(handle, sock_path);
    std::process::exit(code.value());
```
Add `mod toolbar;` and keep the initial-Lister spawn before `toolbar::run`.

- [ ] **Step 3: Build**

Run: `cargo build -p opusdm-hub`
Expected: compiles. If GTK4 dev libraries are missing, install `libgtk-4-dev` (Debian/Ubuntu) and note it in `CHANGES.md`.

- [ ] **Step 4: Manual checklist (needs an X or Wayland session; see `run-x11.sh` after Task 16)**

- [ ] `cargo run -p opusdm-hub` shows a small non-resizable toolbar window.
- [ ] It reads "SRC: -   DST: -" initially.
- [ ] "New Lister" button is present (clicking it does nothing useful until Task 11, but must not crash).
- [ ] "Quit" closes the window and the process exits.
- [ ] Closing via the window title-bar button also exits.

- [ ] **Step 5: Commit**

```bash
git add crates/hub CHANGES.md
git commit -m "feat(hub): GTK toolbar window - New Lister, SRC/DST indicators, job rows, Quit"
```
`CHANGES.md`: `- Task 10: opusdm-hub toolbar.rs GTK window; main runs the GTK app.`

---

## Task 11: lister crate + socket client

**Files:**
- Create/replace: `crates/lister/Cargo.toml`, `crates/lister/src/main.rs`, `crates/lister/src/client.rs`
- Test: inline `#[cfg(test)]` in `client.rs` (round-trips over a real socket pair, no GTK)

**Interfaces:**
- Consumes: `opusdm_ipc::{frame, sockpath::ENV_SOCK, ToHub, ToLister, ListerId}`
- Produces:
  - `crate::client::Client` with:
    - `Client::connect(sock_path: &Path, pid: u32, initial_path: &Path) -> std::io::Result<(Client, ListerId, Roles)>` - connects, sends `Register`, blocks for `Welcome`, returns the assigned id and initial roles
    - `Client::send(&self, msg: &ToHub) -> std::io::Result<()>` - locks the write half, frames, writes
    - `Client::take_inbox(&mut self) -> async_channel::Receiver<Inbound>` - the reader-thread output; callable once
  - `crate::client::Roles { pub source: Option<ListerId>, pub dest: Option<ListerId> }`
  - `crate::client::Inbound` enum: `Msg(ToLister)`, `HubGone` (reader hit EOF or error)
  - `crate::client::start_reader(stream: std::os::unix::net::UnixStream) -> async_channel::Receiver<Inbound>` - spawns the reader thread

- [ ] **Step 1: Write the crate manifest and a real main stub**

`crates/lister/Cargo.toml`:
```toml
[package]
name = "opusdm-lister"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "opusdm-lister"
path = "src/main.rs"

[dependencies]
opusdm-ipc = { path = "../ipc" }
gtk4.workspace = true
async-channel.workspace = true
libc.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 2: Write client.rs tests**

`crates/lister/src/client.rs` (test module first):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::frame::{recv, send};
    use opusdm_ipc::{ListerId, ToHub, ToLister};
    use std::os::unix::net::UnixListener;

    #[test]
    fn connect_sends_register_and_parses_welcome() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = UnixListener::bind(&path).unwrap();

        // Fake hub: accept one, expect Register, reply Welcome.
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let got: ToHub = recv(&mut s).unwrap().unwrap();
            assert!(matches!(got, ToHub::Register { pid: 77, .. }));
            send(&mut s, &ToLister::Welcome {
                lister_id: ListerId(5),
                source: Some(ListerId(5)),
                dest: None,
            }).unwrap();
            // Then push one InvalidatePath so the reader test has something.
            send(&mut s, &ToLister::InvalidatePath("/tmp".into())).unwrap();
        });

        let (mut client, id, roles) =
            Client::connect(&path, 77, std::path::Path::new("/home/x")).unwrap();
        assert_eq!(id, ListerId(5));
        assert_eq!(roles.source, Some(ListerId(5)));

        let inbox = client.take_inbox();
        // Drain synchronously with recv_blocking.
        match inbox.recv_blocking().unwrap() {
            Inbound::Msg(ToLister::InvalidatePath(p)) => assert_eq!(p, std::path::PathBuf::from("/tmp")),
            other => panic!("{other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn reader_reports_hub_gone_on_eof() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _got: ToHub = recv(&mut s).unwrap().unwrap();
            send(&mut s, &ToLister::Welcome {
                lister_id: ListerId(1), source: None, dest: None,
            }).unwrap();
            // Drop the connection.
        });
        let (mut client, _id, _roles) =
            Client::connect(&path, 1, std::path::Path::new("/")).unwrap();
        let inbox = client.take_inbox();
        loop {
            match inbox.recv_blocking() {
                Ok(Inbound::HubGone) => break,
                Ok(_) => continue,
                Err(_) => panic!("channel closed before HubGone"),
            }
        }
        server.join().unwrap();
    }
}
```

- [ ] **Step 3: Run, verify failure**

Run: `cargo test -p opusdm-lister client`
Expected: FAIL - `Client`, `Inbound`, `Roles` not found.

- [ ] **Step 4: Implement client.rs**

```rust
//! Socket link to the hub: a shared write half plus a reader thread that
//! forwards every `ToLister` onto an async channel the GTK loop drains.

use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::{ListerId, ToHub, ToLister};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Mutex;

/// The roles the hub reported.
#[derive(Debug, Clone, Copy)]
pub struct Roles {
    /// Current SOURCE Lister, if any.
    pub source: Option<ListerId>,
    /// Current DEST Lister, if any.
    pub dest: Option<ListerId>,
}

/// Something arriving from the hub.
#[derive(Debug)]
pub enum Inbound {
    /// A protocol message.
    Msg(ToLister),
    /// The connection ended; the process should exit.
    HubGone,
}

/// Owns the write half of the hub connection.
pub struct Client {
    write: Mutex<UnixStream>,
    inbox: Option<async_channel::Receiver<Inbound>>,
}

impl Client {
    /// Connect, `Register`, and wait for `Welcome`.
    pub fn connect(
        sock_path: &Path,
        pid: u32,
        initial_path: &Path,
    ) -> io::Result<(Client, ListerId, Roles)> {
        let mut stream = UnixStream::connect(sock_path)?;
        send(&mut stream, &ToHub::Register { pid, initial_path: initial_path.to_path_buf() })?;
        let welcome: ToLister = recv(&mut stream)?
            .ok_or_else(|| io::Error::other("hub closed before Welcome"))?;
        let (id, roles) = match welcome {
            ToLister::Welcome { lister_id, source, dest } => (lister_id, Roles { source, dest }),
            other => {
                return Err(io::Error::other(format!("expected Welcome, got {other:?}")));
            }
        };
        let reader_stream = stream.try_clone()?;
        let inbox = start_reader(reader_stream);
        Ok((
            Client { write: Mutex::new(stream), inbox: Some(inbox) },
            id,
            roles,
        ))
    }

    /// Frame and send one message to the hub.
    pub fn send(&self, msg: &ToHub) -> io::Result<()> {
        let mut guard = self.write.lock().unwrap();
        send(&mut *guard, msg)
    }

    /// Take the reader channel. Panics if called twice.
    pub fn take_inbox(&mut self) -> async_channel::Receiver<Inbound> {
        self.inbox.take().expect("take_inbox called more than once")
    }
}

/// Spawn the reader thread for `stream`; returns the channel it feeds.
pub fn start_reader(mut stream: UnixStream) -> async_channel::Receiver<Inbound> {
    let (tx, rx) = async_channel::unbounded::<Inbound>();
    std::thread::Builder::new()
        .name("opusdm-lister-reader".into())
        .spawn(move || loop {
            match recv::<_, ToLister>(&mut stream) {
                Ok(Some(msg)) => {
                    if tx.send_blocking(Inbound::Msg(msg)).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => {
                    let _ = tx.send_blocking(Inbound::HubGone);
                    return;
                }
            }
        })
        .expect("spawn reader thread");
    rx
}
```

- [ ] **Step 5: Write main.rs (env guard + connect + hand to window module)**

`crates/lister/src/main.rs`:
```rust
//! opusdm-lister: one file-manager window. Spawned by opusdm-hub.

mod actions;
mod client;
mod dnd;
mod model;
mod window;

use std::path::PathBuf;

fn main() {
    let sock_path = match std::env::var_os(opusdm_ipc::sockpath::ENV_SOCK) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => {
            eprintln!("opusdm-lister: run opusdm-hub (this process is launched by the hub)");
            std::process::exit(1);
        }
    };

    let start_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));

    let pid = std::process::id();
    let (client, lister_id, roles) = match client::Client::connect(&sock_path, pid, &start_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("opusdm-lister: cannot reach hub: {e}");
            std::process::exit(1);
        }
    };

    let code = window::run(client, lister_id, roles, start_path);
    std::process::exit(code.value());
}
```

- [ ] **Step 6: Add module stubs so main compiles**

Create with a single `//!` line and the minimal item each later task needs:
- `crates/lister/src/model.rs`: `//! Directory model. Populated in Task 12.`
- `crates/lister/src/window.rs`:
  ```rust
  //! Lister window. Populated in Task 13.

  use crate::client::{Client, Roles};
  use gtk4 as gtk;
  use opusdm_ipc::ListerId;

  /// Placeholder window runner replaced in Task 13.
  pub fn run(_client: Client, _id: ListerId, _roles: Roles, _start: std::path::PathBuf) -> gtk::glib::ExitCode {
      eprintln!("opusdm-lister: window not implemented yet (plan Task 13)");
      gtk::glib::ExitCode::FAILURE
  }
  ```
- `crates/lister/src/actions.rs`: `//! Operations and role buttons. Populated in Task 14.`
- `crates/lister/src/dnd.rs`: `//! Drag and drop. Populated in Task 15.`

- [ ] **Step 7: Run tests + build workspace**

Run: `cargo test -p opusdm-lister` -> PASS (2 client tests).
Run: `cargo build --workspace` -> succeeds.
Manual: `cargo run -p opusdm-hub` in one terminal; it should now log `[hub] lister ListerId(1) pid=... at <HOME>` (the child connects, then exits FAILURE from the stub window - that is expected until Task 13).

- [ ] **Step 8: Commit**

```bash
git add crates/lister CHANGES.md
git commit -m "feat(lister): crate scaffold, hub socket client with reader thread"
```
`CHANGES.md`: `- Task 11: opusdm-lister crate, Client::connect + reader thread, env guard in main.`

---

## Task 12: lister directory model - Row, read_dir_rows, formatting, sort

**Files:**
- Replace: `crates/lister/src/model.rs`
- Test: inline `#[cfg(test)]` in `model.rs`

**Interfaces:**
- Consumes: `std::fs`
- Produces:
  - `crate::model::Row { pub name: String, pub is_dir: bool, pub is_symlink: bool, pub broken_link: bool, pub size: u64, pub modified_unix: i64 }` - `Clone`
  - `crate::model::read_dir_rows(dir: &Path) -> std::io::Result<Vec<Row>>` - one `Row` per entry (not `.`/`..`); a symlink's `is_dir` follows its target, `broken_link` set when the target is missing
  - `crate::model::format_size(is_dir: bool, size: u64) -> String` - `""` for dirs; `"512 B"`, `"1.5K"`, `"3.0M"`, ...
  - `crate::model::format_mtime(unix: i64, tz_offset_secs: i32) -> String` - `"YYYY-MM-DD HH:MM"`
  - `crate::model::SortCol` enum: `Name | Size | Modified`
  - `crate::model::dir_first_cmp(a: &Row, b: &Row, col: SortCol, desc: bool) -> std::cmp::Ordering` - directories always precede files regardless of `desc`; then compare by `col`
  - `crate::model::local_tz_offset_secs() -> i32` - best-effort local offset; used by the window, not under test

- [ ] **Step 1: Write the tests**

`crates/lister/src/model.rs` (test module first):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;
    use std::fs;

    #[test]
    fn format_size_rules() {
        assert_eq!(format_size(true, 4096), "");
        assert_eq!(format_size(false, 0), "0 B");
        assert_eq!(format_size(false, 512), "512 B");
        assert_eq!(format_size(false, 1024), "1.0K");
        assert_eq!(format_size(false, 1536), "1.5K");
        assert_eq!(format_size(false, 3 * 1024 * 1024), "3.0M");
    }

    #[test]
    fn format_mtime_utc_epoch_and_known_value() {
        assert_eq!(format_mtime(0, 0), "1970-01-01 00:00");
        // 1_700_000_000 == 2023-11-14T22:13:20Z
        assert_eq!(format_mtime(1_700_000_000, 0), "2023-11-14 22:13");
        // +1h offset moves it to 23:13
        assert_eq!(format_mtime(1_700_000_000, 3600), "2023-11-14 23:13");
    }

    fn row(name: &str, is_dir: bool, size: u64, mtime: i64) -> Row {
        Row {
            name: name.into(),
            is_dir,
            is_symlink: false,
            broken_link: false,
            size,
            modified_unix: mtime,
        }
    }

    #[test]
    fn dirs_sort_before_files_even_descending() {
        let d = row("zeta", true, 0, 0);
        let f = row("alpha", false, 0, 0);
        assert_eq!(dir_first_cmp(&d, &f, SortCol::Name, false), Ordering::Less);
        assert_eq!(dir_first_cmp(&d, &f, SortCol::Name, true), Ordering::Less);
        assert_eq!(dir_first_cmp(&f, &d, SortCol::Name, true), Ordering::Greater);
    }

    #[test]
    fn name_sort_is_case_insensitive() {
        let a = row("Apple", false, 0, 0);
        let b = row("banana", false, 0, 0);
        assert_eq!(dir_first_cmp(&a, &b, SortCol::Name, false), Ordering::Less);
    }

    #[test]
    fn size_and_mtime_sort_and_reverse() {
        let small = row("s", false, 10, 100);
        let big = row("b", false, 20, 50);
        assert_eq!(dir_first_cmp(&small, &big, SortCol::Size, false), Ordering::Less);
        assert_eq!(dir_first_cmp(&small, &big, SortCol::Size, true), Ordering::Greater);
        assert_eq!(dir_first_cmp(&small, &big, SortCol::Modified, false), Ordering::Greater);
    }

    #[test]
    fn read_dir_rows_reports_kinds() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        fs::create_dir(base.join("adir")).unwrap();
        fs::write(base.join("afile"), b"12345").unwrap();
        std::os::unix::fs::symlink("afile", base.join("good_link")).unwrap();
        std::os::unix::fs::symlink("missing", base.join("bad_link")).unwrap();

        let mut rows = read_dir_rows(base).unwrap();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        let by = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();

        assert!(by("adir").is_dir);
        assert_eq!(by("afile").size, 5);
        assert!(by("good_link").is_symlink && by("good_link").is_dir == false && !by("good_link").broken_link);
        assert!(by("bad_link").is_symlink && by("bad_link").broken_link);
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p opusdm-lister model`
Expected: FAIL - items not found.

- [ ] **Step 3: Implement model.rs**

Prepend to `crates/lister/src/model.rs`:
```rust
//! The directory listing model: reading entries and formatting them for the
//! column view. Pure and deterministic; no GTK.

use std::cmp::Ordering;
use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// One row in a Lister's file view.
#[derive(Clone)]
pub struct Row {
    /// File name (no directory part).
    pub name: String,
    /// Whether this entry is a directory (following symlinks).
    pub is_dir: bool,
    /// Whether the entry itself is a symbolic link.
    pub is_symlink: bool,
    /// A symlink whose target does not resolve.
    pub broken_link: bool,
    /// Size in bytes (0 for directories and broken links).
    pub size: u64,
    /// Modification time, whole seconds since the Unix epoch.
    pub modified_unix: i64,
}

/// Which column an ordering is keyed on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortCol {
    /// By name, case-insensitive.
    Name,
    /// By byte size.
    Size,
    /// By modification time.
    Modified,
}

/// Read `dir` into rows. `.` and `..` are excluded. Never follows into a
/// symlinked directory - only classifies it.
pub fn read_dir_rows(dir: &Path) -> io::Result<Vec<Row>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let link_meta = entry.metadata(); // this is symlink_metadata for DirEntry on unix? no:
        // DirEntry::metadata does NOT follow symlinks. Use it for is_symlink.
        let lmeta = std::fs::symlink_metadata(entry.path())?;
        let is_symlink = lmeta.file_type().is_symlink();
        let target_meta = std::fs::metadata(entry.path()); // follows links
        let (is_dir, size, broken_link) = match &target_meta {
            Ok(m) => (m.is_dir(), if m.is_dir() { 0 } else { m.len() }, false),
            Err(_) => (false, 0, is_symlink),
        };
        let modified_unix = target_meta
            .as_ref()
            .ok()
            .or(Some(&lmeta))
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let _ = link_meta;
        rows.push(Row { name, is_dir, is_symlink, broken_link, size, modified_unix });
    }
    Ok(rows)
}

/// Human-readable size. Empty string for directories.
pub fn format_size(is_dir: bool, size: u64) -> String {
    if is_dir {
        return String::new();
    }
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    if size < 1024 {
        return format!("{size} B");
    }
    let mut value = size as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1}{}", UNITS[unit])
}

/// Format `unix` seconds as `YYYY-MM-DD HH:MM` at `tz_offset_secs` east of UTC.
pub fn format_mtime(unix: i64, tz_offset_secs: i32) -> String {
    let local = unix + tz_offset_secs as i64;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}

/// Days since 1970-01-01 -> (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + i64::from(m <= 2), m, d)
}

/// Compare two rows: directories first (both directions), then by `col`.
pub fn dir_first_cmp(a: &Row, b: &Row, col: SortCol, desc: bool) -> Ordering {
    if a.is_dir != b.is_dir {
        return if a.is_dir { Ordering::Less } else { Ordering::Greater };
    }
    let base = match col {
        SortCol::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortCol::Size => a.size.cmp(&b.size),
        SortCol::Modified => a.modified_unix.cmp(&b.modified_unix),
    };
    if desc {
        base.reverse()
    } else {
        base
    }
}

/// Best-effort local timezone offset in seconds east of UTC. Falls back to 0.
pub fn local_tz_offset_secs() -> i32 {
    // Derive from the difference between a local and a UTC broken-down `time`.
    // libc localtime_r is the simplest reliable source.
    // SAFETY: passing valid pointers; localtime_r fills `tm`.
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&now, &mut tm).is_null() };
    if ok {
        tm.tm_gmtoff as i32
    } else {
        0
    }
}
```
Note for the implementer: `read_dir_rows` as drafted calls `entry.metadata()` then shadows it; simplify to use only `symlink_metadata` + `metadata` as shown and delete the unused `link_meta` line before committing (kept here only to flag the DirEntry pitfall: `DirEntry::metadata` does not follow symlinks).

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p opusdm-lister`
Expected: PASS (client + model tests).

- [ ] **Step 5: Commit**

```bash
git add crates/lister CHANGES.md
git commit -m "feat(lister): directory model - read_dir_rows, size/mtime formatting, dir-first sort"
```
`CHANGES.md`: `- Task 12: opusdm-lister model.rs (pure): rows, format_size, format_mtime, dir_first_cmp.`

---

## Task 13: lister window - path bar, column view, navigation, status line

**Files:**
- Replace: `crates/lister/src/window.rs`
- Test: no automated UI test. `cargo build` + manual checklist.

**Interfaces:**
- Consumes: `crate::client::{Client, Inbound, Roles}`, `crate::model::{Row, SortCol, read_dir_rows, format_size, format_mtime, dir_first_cmp, local_tz_offset_secs}`, `opusdm_ipc::{ListerId, ToHub, ToLister}`, `gtk4` as `gtk`, `async_channel`
- Produces:
  - `crate::window::run(client: Client, id: ListerId, roles: Roles, start: PathBuf) -> gtk::glib::ExitCode`
  - `crate::window::Ui` struct (shared with Task 14 and 15 via `Rc`): holds `window: gtk::ApplicationWindow`, `client: Rc<Client>`, `id: ListerId`, `cwd: Rc<RefCell<PathBuf>>`, `store: gio::ListStore`, `selection: gtk::MultiSelection`, `sort: Rc<Cell<(SortCol, bool)>>`, `status: gtk::Label`, `role_badge: gtk::Label`, `roles: Rc<Cell<Roles>>`, `path_entry: gtk::Entry`, `tz: i32`
  - `crate::window::Ui::navigate_to(&self, dir: &Path)` - read rows, repopulate `store`, update title/entry/status, send `ToHub::PathChanged`
  - `crate::window::Ui::reload(&self)` - `navigate_to(&self.cwd())` preserving selection by name
  - `crate::window::Ui::selected_paths(&self) -> Vec<PathBuf>`
  - `crate::window::Ui::apply_inbound(self: &Rc<Self>, msg: ToLister)` - dispatch table; Task 14 fills the job/conflict arms, Task 13 handles `RoleChanged`, `InvalidatePath`, `Present`, `Shutdown`

**Design notes:**
- Model rows are wrapped in `glib::BoxedAnyObject::new(row_rc)` where `row_rc: Rc<Row>`; the `gio::ListStore` is `gio::ListStore::new::<glib::BoxedAnyObject>()`.
- Sorting: a `gtk::SortListModel` wrapping `store`, with a `gtk::CustomSorter`. The sorter closure reads `sort: (SortCol, bool)` from the shared `Cell` and calls `dir_first_cmp`. Re-sort on header click via `sorter.changed(gtk::SorterChange::Different)`.
- Selection: `gtk::MultiSelection::new(Some(sort_model))`, feeding a `gtk::ColumnView`.
- Three `gtk::ColumnViewColumn`s, each a `gtk::SignalListItemFactory`:
  - Name: setup a `gtk::Box` [`gtk::Image` + `gtk::Label`]; bind reads the `Rc<Row>`, sets icon name (`folder` / `text-x-generic` / `emblem-symbolic-link`; broken link -> `error`), label text = name.
  - Size: `gtk::Label` xalign 1.0, text = `format_size(row.is_dir, row.size)`.
  - Modified: `gtk::Label`, text = `format_mtime(row.modified_unix, tz)`.
- Header click -> set `sort` cell: same column flips `desc`, new column resets to ascending; then `sorter.changed(...)`.
- Path bar: `gtk::Entry` (activate -> `navigate_to` if the text is an existing dir, else flash the entry and restore). `Parent` button -> `cwd.parent()`. `Root` -> `/`. `Refresh` -> `reload()`.
- Row activation (`ColumnView::connect_activate`): if the row is a dir (or a symlink to a dir) -> `navigate_to`; else `gio::AppInfo::launch_default_for_uri(file uri, None)`.
- Status line: `format!("{} items, {} selected, {}", n, sel, format_size(false, total_selected_bytes))`.
- Role badge label: `"SOURCE"` / `"DEST"` / `""` derived from `roles` vs `id`.
- Focus: `gtk::EventControllerFocus` on the window -> on enter send `ToHub::FocusGained`.
- Window close (`connect_close_request`): send `ToHub::Goodbye`, return `glib::Propagation::Proceed` so the process exits.
- Inbound pump: `let inbox = client.take_inbox();` then `glib::MainContext::default().spawn_local(async move { while let Ok(inb) = inbox.recv().await { match inb { Inbound::Msg(m) => ui.apply_inbound(m), Inbound::HubGone => { ui.window.close(); } } } })`.
- `apply_inbound` arms for Task 13:
  - `ToLister::RoleChanged { source, dest }` -> update `roles` cell + `role_badge` + (Task 14) button toggle states.
  - `ToLister::InvalidatePath(p)` -> if `p == *cwd` then `reload()`.
  - `ToLister::Present` -> `window.present()`.
  - `ToLister::Shutdown` -> `window.close()`.
  - `ToLister::Welcome { .. }` -> ignore (already consumed).
  - job arms (`JobProgress`, `JobDone`, `ConflictPrompt`) -> `todo!()` placeholder replaced in Task 14. Use an explicit `// filled in Task 14` comment and a no-op, not `todo!()`, so builds pass.

- [ ] **Step 1: Implement window.rs**

Write the module per the notes. Key skeleton:
```rust
//! The Lister window: path bar, column view, status line, navigation.

use crate::client::{Client, Inbound, Roles};
use crate::model::{
    dir_first_cmp, format_mtime, format_size, local_tz_offset_secs, read_dir_rows, Row, SortCol,
};
use gtk4 as gtk;
use gtk::gio;
use gtk::glib::{self, BoxedAnyObject};
use gtk::prelude::*;
use opusdm_ipc::{ListerId, ToHub, ToLister};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Everything the window's callbacks need. Shared by Rc across Tasks 13-15.
pub struct Ui {
    pub window: gtk::ApplicationWindow,
    pub client: Rc<Client>,
    pub id: ListerId,
    pub cwd: RefCell<PathBuf>,
    pub store: gio::ListStore,
    pub selection: gtk::MultiSelection,
    pub sort: Cell<(SortCol, bool)>,
    pub status: gtk::Label,
    pub role_badge: gtk::Label,
    pub roles: Cell<Roles>,
    pub path_entry: gtk::Entry,
    pub tz: i32,
}

/// Build and run the window application.
pub fn run(client: Client, id: ListerId, roles: Roles, start: PathBuf) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.opusdm.Lister")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let client = Rc::new(client);
    let state = Rc::new(RefCell::new(Some((client, id, roles, start))));

    app.connect_activate(move |app| {
        let (client, id, roles, start) = state.borrow_mut().take().expect("activate once");
        let ui = build_window(app, client, id, roles);
        ui.navigate_to(&start);
        // Inbound pump.
        let inbox = ui.client.clone(); // Client owns the inbox; see note
        // Actually: take_inbox needs &mut Client. Resolve by taking the inbox
        // in `run` before wrapping in Rc. See Step 2.
        let _ = inbox;
    });
    app.run_with_args::<&str>(&[])
}
```

- [ ] **Step 2: Resolve the inbox ownership**

`Client::take_inbox` needs `&mut Client`, but the window shares `Rc<Client>`. Fix in `run`:
```rust
pub fn run(mut client: Client, id: ListerId, roles: Roles, start: PathBuf) -> glib::ExitCode {
    let inbox = client.take_inbox();           // take before sharing
    let client = Rc::new(client);
    // move `inbox` into the activate closure alongside client/id/roles/start
    // then: glib::MainContext::default().spawn_local(async move {
    //     while let Ok(inb) = inbox.recv().await { ... ui.apply_inbound(...) }
    // });
}
```

- [ ] **Step 3: Build**

Run: `cargo build -p opusdm-lister`
Expected: compiles. Fix any `todo!()`/`unimplemented!()` - none allowed.

- [ ] **Step 4: Manual checklist (under Xephyr, see Task 16)**

- [ ] `cargo run -p opusdm-hub` opens the toolbar AND one Lister window at `$HOME`.
- [ ] The Lister lists directories first, then files; names, sizes, dates render.
- [ ] Double-click a directory navigates into it; the title and path bar update.
- [ ] `Parent` goes up; `Root` goes to `/`; `Refresh` re-reads.
- [ ] Typing a valid path in the entry and pressing Enter navigates there; an invalid path is rejected without navigating.
- [ ] Clicking the Size header sorts by size; clicking again reverses; directories still lead.
- [ ] Selecting rows updates "N items, M selected, X" in the status line.
- [ ] Closing the window exits that process; the hub logs `lister ... gone`; the toolbar stays open.
- [ ] Focusing the window: hub logs a role change making it SOURCE (second window focus makes the first DEST).

- [ ] **Step 5: Commit**

```bash
git add crates/lister CHANGES.md
git commit -m "feat(lister): window with path bar, column view, sorting, navigation, status line"
```
`CHANGES.md`: `- Task 13: opusdm-lister window.rs - ColumnView, navigation, sort, status, focus, inbound pump.`

---

## Task 14: lister actions - button row, operations, role buttons, conflict dialog

**Files:**
- Replace: `crates/lister/src/actions.rs`
- Modify: `crates/lister/src/window.rs` (call `actions::attach(&ui)` during build; fill the job arms of `apply_inbound` by delegating to `actions`)
- Test: one pure unit test for `resolve_copy_target`; the rest is manual.

**Interfaces:**
- Consumes: `crate::window::Ui`, `opusdm_ipc::{Op, Resolution, Role, ToHub, ToLister, Outcome, FileMeta}`, `gtk4` as `gtk`
- Produces:
  - `crate::actions::attach(ui: &Rc<Ui>)` - builds the button row `gtk::Box` and inserts it into the window layout above the status line; wires every button
  - `crate::actions::resolve_copy_target(roles: Roles, me: ListerId, my_cwd: &Path, dest_cwd: Option<&Path>) -> Result<PathBuf, String>` - returns the dest dir when `me` is SOURCE and a DEST with a known path exists, else a message
  - `crate::actions::on_job_progress(ui: &Rc<Ui>, job_id: u64, done: u64, total: u64, current: &Path)` - updates an in-window job status line (a `gtk::Label` under the button row; created lazily)
  - `crate::actions::on_job_done(ui: &Rc<Ui>, job_id: u64, outcome: &Outcome, errors: &[(PathBuf, String)])` - clears the job status line; on errors opens a `gtk::AlertDialog` listing them; triggers `ui.reload()`
  - `crate::actions::on_conflict(ui: &Rc<Ui>, req_id: u64, path: &Path, src: FileMeta, dst: FileMeta)` - opens a modal dialog with buttons Skip / Overwrite / Skip All / Overwrite All / Cancel; sends `ToHub::ConflictReply { req_id, resolution }`

**Button row (left group, act on this Lister's selection):**
- `Copy` -> `target = resolve_copy_target(...)`; on `Ok(dir)` send `ToHub::RunJob { op: Op::Copy, srcs: ui.selected_paths(), dest_dir: dir }`; on `Err(msg)` set the status line text to `msg` for 4s.
- `Move` -> same with `Op::Move`.
- `Delete` -> confirm `gtk::AlertDialog` ("Delete N items permanently?"); on confirm `RunJob { Op::Delete, srcs, dest_dir: ui.cwd() }` (dest_dir unused by delete but keep the field populated).
- `MakeDir` -> `gtk::Entry` prompt dialog -> `RunJob { Op::MakeDir { name }, srcs: vec![], dest_dir: ui.cwd() }`.
- `NewFile` -> prompt -> `RunJob { Op::NewFile { name }, .., dest_dir: ui.cwd() }`.
- `Rename` -> single selection only; prompt pre-filled with the current name; on accept `std::fs::rename` locally in `ui.cwd()`, then `ui.reload()` and (best-effort) the hub will also broadcast `InvalidatePath` for other Listers via a `RunJob`? No: rename is local. Instead send `ToHub::RunJob { Op::Move, srcs: vec![old], dest_dir: ui.cwd() }` only when the name is unchanged-dir... simpler: do the rename locally with `std::fs::rename(old, new)` and call `ui.reload()`. Other Listers refresh on their next manual `Refresh`. (v1 accepts this.)

**Right group (role toggles):**
- `Source` `gtk::ToggleButton` -> on toggle-on send `ToHub::SetRole(Role::Source)`; on toggle-off send `ToHub::SetRole(Role::None)`.
- `Dest` `gtk::ToggleButton` -> `Role::Dest` / `Role::None`.
- `Lock` `gtk::ToggleButton` -> `ToHub::LockRole` / `ToHub::UnlockRole`.
- These are updated from `apply_inbound`'s `RoleChanged` arm: set `.set_active(...)` without re-emitting (block the handler with `SignalHandlerId` or a `Cell<bool>` guard).

- [ ] **Step 1: Write the pure test**

`crates/lister/src/actions.rs` (test module):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Roles;
    use opusdm_ipc::ListerId;
    use std::path::Path;

    #[test]
    fn resolve_requires_me_source_and_known_dest() {
        let me = ListerId(1);
        let other = ListerId(2);

        // Not source -> error.
        let r = resolve_copy_target(
            Roles { source: Some(other), dest: Some(me) },
            me,
            Path::new("/a"),
            Some(Path::new("/b")),
        );
        assert!(r.is_err());

        // Source but no dest -> error.
        let r = resolve_copy_target(
            Roles { source: Some(me), dest: None },
            me,
            Path::new("/a"),
            None,
        );
        assert!(r.is_err());

        // Source and dest path known -> Ok(dest path).
        let r = resolve_copy_target(
            Roles { source: Some(me), dest: Some(other) },
            me,
            Path::new("/a"),
            Some(Path::new("/b/c")),
        );
        assert_eq!(r.unwrap(), std::path::PathBuf::from("/b/c"));
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p opusdm-lister actions`
Expected: FAIL - `resolve_copy_target` not found.

- [ ] **Step 3: Implement resolve_copy_target and the module**

```rust
//! The Lister button row and the dialogs it drives.

use crate::client::Roles;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::{FileMeta, ListerId, Op, Outcome, Resolution, Role, ToHub};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Decide the destination directory for a Copy/Move from this Lister.
pub fn resolve_copy_target(
    roles: Roles,
    me: ListerId,
    _my_cwd: &Path,
    dest_cwd: Option<&Path>,
) -> Result<PathBuf, String> {
    if roles.source != Some(me) {
        return Err("this Lister is not the SOURCE".into());
    }
    match (roles.dest, dest_cwd) {
        (Some(d), Some(p)) if d != me => Ok(p.to_path_buf()),
        (Some(d), _) if d == me => Err("SOURCE and DEST are the same Lister".into()),
        _ => Err("no DEST Lister set".into()),
    }
}

/// Build the button row and wire every control. Call once during window build.
pub fn attach(ui: &Rc<Ui>) {
    // ... construct gtk::Box with the buttons described in the plan notes ...
    let _ = ui;
    unimplemented!("fill per plan Task 14 notes")
}

/// Update the in-window job status line.
pub fn on_job_progress(ui: &Rc<Ui>, job_id: u64, done: u64, total: u64, current: &Path) {
    let _ = (ui, job_id, done, total, current);
}

/// Finish a job: clear the line, show errors, reload the view.
pub fn on_job_done(ui: &Rc<Ui>, job_id: u64, outcome: &Outcome, errors: &[(PathBuf, String)]) {
    let _ = (job_id, outcome);
    if !errors.is_empty() {
        // gtk::AlertDialog listing errors
    }
    ui.reload();
}

/// Prompt for an overwrite conflict and reply to the hub.
pub fn on_conflict(ui: &Rc<Ui>, req_id: u64, path: &Path, src: FileMeta, dst: FileMeta) {
    let _ = (path, src, dst);
    // Build a modal dialog; each button maps to a Resolution:
    let reply = |res: Resolution| {
        let _ = ui.client.send(&ToHub::ConflictReply { req_id, resolution: res });
    };
    let _ = reply;
    unimplemented!("fill per plan Task 14 notes")
}
```
No `unimplemented!` may remain at commit - the implementer completes `attach` and `on_conflict`.

- [ ] **Step 4: Wire into window.rs**

- In `build_window`, after assembling the layout, call `crate::actions::attach(&ui)`.
- In `Ui::apply_inbound`, replace the Task 13 job placeholders:
  - `ToLister::JobProgress { job_id, current_file, bytes_done, bytes_total, .. }` -> `actions::on_job_progress(self, job_id, bytes_done, bytes_total, &current_file)`
  - `ToLister::JobDone { job_id, outcome, errors }` -> `actions::on_job_done(self, job_id, &outcome, &errors)`
  - `ToLister::ConflictPrompt { req_id, path, src, dst, .. }` -> `actions::on_conflict(self, req_id, &path, src, dst)`
  - `ToLister::RoleChanged { .. }` arm also updates the three toggle buttons; store their handles in `Ui` (add `role_src_btn`, `role_dst_btn`, `role_lock_btn: gtk::ToggleButton` and a `role_guard: Cell<bool>`).

- [ ] **Step 5: Build + test**

Run: `cargo test -p opusdm-lister` -> PASS (client, model, actions).
Run: `cargo build --workspace` -> succeeds.

- [ ] **Step 6: Manual checklist (Xephyr)**

- [ ] Two Listers open. Focus A (SOURCE), focus B (DEST) - badges show SOURCE/DEST; toolbar shows the paths.
- [ ] In A select a file, click `Copy` -> file appears in B's directory; toolbar progress row appears then clears; both A and B refresh.
- [ ] `Move` within the same filesystem is instant; the file leaves A.
- [ ] Copy onto an existing name -> conflict dialog; `Skip` leaves the old file; re-run and `Overwrite All` replaces it.
- [ ] `Delete` asks to confirm, then removes; the view refreshes.
- [ ] `MakeDir` / `NewFile` create the entry and refresh.
- [ ] `Rename` on a single selection renames in place.
- [ ] `Source` / `Dest` / `Lock` toggles reflect and drive hub state; `Lock` on A keeps A as SOURCE when B is focused.
- [ ] Clicking `Copy` with no DEST set shows the "no DEST Lister set" message in the status line.

- [ ] **Step 7: Commit**

```bash
git add crates/lister CHANGES.md
git commit -m "feat(lister): button row, file operations, role toggles, conflict dialog"
```
`CHANGES.md`: `- Task 14: opusdm-lister actions.rs - Copy/Move/Delete/MakeDir/NewFile/Rename, role toggles, conflict prompt.`

---

## Task 15: lister drag-and-drop between windows

**Files:**
- Replace: `crates/lister/src/dnd.rs`
- Modify: `crates/lister/src/window.rs` (call `dnd::attach(&ui, &column_view)` during build)
- Test: one pure unit test for `uris_to_paths` / `paths_to_uri_list`; the rest manual.

**Interfaces:**
- Consumes: `crate::window::Ui`, `gtk4` as `gtk`, `opusdm_ipc::{Op, ToHub}`
- Produces:
  - `crate::dnd::attach(ui: &Rc<Ui>, view: &gtk::ColumnView)` - adds a `gtk::DragSource` (drag = current selection, content = `text/uri-list`) and a `gtk::DropTarget` (accepts `text/uri-list`) to `view`
  - `crate::dnd::paths_to_uri_list(paths: &[PathBuf]) -> String` - `\r\n`-joined `file://` URIs, trailing CRLF
  - `crate::dnd::uris_to_paths(text: &str) -> Vec<PathBuf>` - parse a `text/uri-list`, ignoring comment lines (`#`) and non-`file:` URIs, percent-decoding

**Design notes:**
- `DragSource`: `set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE)`. `connect_prepare` -> build `gdk::ContentProvider::for_bytes("text/uri-list", &glib::Bytes::from(paths_to_uri_list(&ui.selected_paths()).as_bytes()))`. Return `None` if the selection is empty.
- `DropTarget::new(glib::Type::STRING, gdk::DragAction::COPY | gdk::DragAction::MOVE)` with formats set to accept `text/uri-list`. On `connect_drop`: read the value as string, `let srcs = uris_to_paths(&text)`; decide op by the drop's active modifiers (`Shift` -> `Op::Move`, else `Op::Copy`); send `ToHub::RunJob { op, srcs, dest_dir: ui.cwd() }`. Return `true`.
- If the drop lands on a directory row, use that directory as `dest_dir` instead of `ui.cwd()` (hit-test via `view` + the row under the pointer; if unavailable in v1, always use `ui.cwd()` and note it).
- The hub already validates (`dest inside source`, etc.) and will reject a self-drop.

- [ ] **Step 1: Write the pure test**

`crates/lister/src/dnd.rs` (test module):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_simple_paths() {
        let paths = vec![PathBuf::from("/home/x/a.txt"), PathBuf::from("/tmp/b c.txt")];
        let list = paths_to_uri_list(&paths);
        assert!(list.contains("file:///home/x/a.txt"));
        // space is percent-encoded
        assert!(list.contains("file:///tmp/b%20c.txt"));
        assert!(list.ends_with("\r\n"));
        let back = uris_to_paths(&list);
        assert_eq!(back, paths);
    }

    #[test]
    fn ignores_comments_and_non_file_uris() {
        let text = "# comment\r\nhttp://example.com/x\r\nfile:///ok\r\n";
        assert_eq!(uris_to_paths(text), vec![PathBuf::from("/ok")]);
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p opusdm-lister dnd`
Expected: FAIL - functions not found.

- [ ] **Step 3: Implement dnd.rs**

```rust
//! Drag-and-drop of file selections between Lister windows via `text/uri-list`.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::{Op, ToHub};
use std::path::PathBuf;
use std::rc::Rc;

/// Build a `text/uri-list` payload (CRLF-separated, trailing CRLF).
pub fn paths_to_uri_list(paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for p in paths {
        out.push_str("file://");
        out.push_str(&percent_encode_path(&p.to_string_lossy()));
        out.push_str("\r\n");
    }
    out
}

/// Parse a `text/uri-list`: skip `#` comments and non-`file:` URIs, decode `%`.
pub fn uris_to_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        .map(|rest| PathBuf::from(percent_decode(rest)))
        .collect()
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Attach a drag source and a drop target to the column view.
pub fn attach(ui: &Rc<Ui>, view: &gtk::ColumnView) {
    // DragSource
    let src = gtk::DragSource::new();
    src.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    {
        let ui = ui.clone();
        src.connect_prepare(move |_s, _x, _y| {
            let paths = ui.selected_paths();
            if paths.is_empty() {
                return None;
            }
            let payload = paths_to_uri_list(&paths);
            Some(gdk::ContentProvider::for_bytes(
                "text/uri-list",
                &glib::Bytes::from(payload.as_bytes()),
            ))
        });
    }
    view.add_controller(src);

    // DropTarget
    let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::COPY | gdk::DragAction::MOVE);
    {
        let ui = ui.clone();
        target.connect_drop(move |t, value, _x, _y| {
            let Ok(text) = value.get::<String>() else {
                return false;
            };
            let srcs = uris_to_paths(&text);
            if srcs.is_empty() {
                return false;
            }
            let shift = t
                .current_event()
                .map(|e| e.modifier_state().contains(gdk::ModifierType::SHIFT_MASK))
                .unwrap_or(false);
            let op = if shift { Op::Move } else { Op::Copy };
            let dest_dir = ui.cwd.borrow().clone();
            let _ = ui.client.send(&ToHub::RunJob { op, srcs, dest_dir });
            true
        });
    }
    view.add_controller(target);
}
```

- [ ] **Step 4: Wire into window.rs and build**

In `build_window`, after creating the `ColumnView`, call `crate::dnd::attach(&ui, &column_view);`.
Run: `cargo test -p opusdm-lister` -> PASS. `cargo build --workspace` -> succeeds.

- [ ] **Step 5: Manual checklist (Xephyr)**

- [ ] Drag a selection of files from Lister A onto Lister B's list -> they are copied into B's directory.
- [ ] Hold `Shift` while dragging -> they are moved instead.
- [ ] Dragging onto the same directory (self) -> no duplication; hub rejects it (status/toolbar shows the rejection or nothing happens).
- [ ] A file name with spaces transfers correctly.

- [ ] **Step 6: Commit**

```bash
git add crates/lister CHANGES.md
git commit -m "feat(lister): drag-and-drop file transfer between windows"
```
`CHANGES.md`: `- Task 15: opusdm-lister dnd.rs - text/uri-list drag source and drop target.`

---

## Task 16: launch script, integration smoke test, QA checklist

**Files:**
- Modify: `run-x11.sh` (launch `opusdm-hub` inside Xephyr instead of a bare terminal)
- Create: `crates/hub/tests/integration_copy.rs` (`#[ignore]` by default, needs `Xvfb` only for the hub - actually the hub has a GTK toolbar, so the test runs the *server layer* headless, see below)
- Create: `docs/superpowers/QA-checklist.md`
- Test: `cargo test -p opusdm-hub --test integration_copy -- --ignored`

- [ ] **Step 1: Update run-x11.sh**

Replace the "Launch the terminal inside the nested server" section and the trailing `wait` with:
```bash
# Build the workspace before entering the sandbox
echo "Building opusdm (release)"
cargo build --release --workspace

# Launch the hub inside the nested server; it spawns the first Lister itself
HUB_BIN="$(pwd)/target/release/opusdm-hub"
if [ ! -x "$HUB_BIN" ]; then
    echo "opusdm-hub not built at $HUB_BIN"
    exit 1
fi
echo "Starting opusdm-hub"
# Give the hub its own runtime dir so a stale socket never blocks the sandbox
export XDG_RUNTIME_DIR="$(mktemp -d /tmp/opusdm-xephyr-XXXXXX)"
DISPLAY="$DISPLAY_NUM" "$HUB_BIN" &
CHILD_PIDS+=("$!")

# Optional extra terminal for poking around
if [ "${XEPHYR_TERM_EXTRA:-0}" = "1" ]; then
    DISPLAY="$DISPLAY_NUM" "$TERMINAL" &
    CHILD_PIDS+=("$!")
fi

wait "$XEPHYR_PID"
```
Also update the header usage comment to mention it now launches `opusdm-hub`. Keep `OPENBOX_CONFIG` default pointing at the repo file: change `${XEPHYR_OB_CONFIG:-/ramdisk/openbox-rc.xml}` to `${XEPHYR_OB_CONFIG:-$(pwd)/openbox-rc.xml}`.

- [ ] **Step 2: Write the integration smoke test**

`crates/hub/tests/integration_copy.rs`:
```rust
//! End-to-end: start the hub server layer, connect two fake Listers over the
//! real socket, drive a Copy SOURCE -> DEST, assert the file lands.
//!
//! Ignored by default (spawns threads and touches the filesystem); run with:
//!   cargo test -p opusdm-hub --test integration_copy -- --ignored

use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::{Op, ToHub, ToLister};
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

// The server module is a binary crate's module; expose it for the test by
// adding `pub mod server; pub mod roles; pub mod jobs;` guarded with
// `#[cfg(test)]`-free `pub` in main.rs OR add a `lib.rs`. See Step 3.
use opusdm_hub::server::spawn_server;

fn reg(s: &mut UnixStream, pid: u32, path: &str) -> opusdm_ipc::ListerId {
    send(s, &ToHub::Register { pid, initial_path: path.into() }).unwrap();
    match recv::<_, ToLister>(s).unwrap().unwrap() {
        ToLister::Welcome { lister_id, .. } => lister_id,
        other => panic!("{other:?}"),
    }
}

#[test]
#[ignore]
fn copy_source_to_dest_lands_file() {
    let td = tempfile::tempdir().unwrap();
    let sock = td.path().join("hub.sock");
    let src_dir = td.path().join("srcpane");
    let dst_dir = td.path().join("dstpane");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dst_dir).unwrap();
    std::fs::write(src_dir.join("payload.txt"), b"hello opusdm").unwrap();

    let listener = UnixListener::bind(&sock).unwrap();
    let _handle = spawn_server(listener, Box::new(|_p| {}));

    let mut a = UnixStream::connect(&sock).unwrap();
    a.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut b = UnixStream::connect(&sock).unwrap();
    b.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    let _ia = reg(&mut a, 1, src_dir.to_str().unwrap());
    let _ib = reg(&mut b, 2, dst_dir.to_str().unwrap());

    // A gains focus -> A = SOURCE. B gains focus -> B = SOURCE, A = DEST.
    // We want A = SOURCE, B = DEST: focus B then focus A.
    send(&mut b, &ToHub::FocusGained).unwrap();
    let _ = recv::<_, ToLister>(&mut b).unwrap();
    let _ = recv::<_, ToLister>(&mut a).unwrap();
    send(&mut a, &ToHub::FocusGained).unwrap();
    let _ = recv::<_, ToLister>(&mut a).unwrap();
    let _ = recv::<_, ToLister>(&mut b).unwrap();

    // A tells the hub its path (already sent at Register) and runs the job.
    send(
        &mut a,
        &ToHub::RunJob {
            op: Op::Copy,
            srcs: vec![src_dir.join("payload.txt")],
            dest_dir: dst_dir.clone(),
        },
    )
    .unwrap();

    // Read messages on A until JobDone.
    loop {
        match recv::<_, ToLister>(&mut a).unwrap() {
            Some(ToLister::JobDone { outcome, errors, .. }) => {
                assert!(errors.is_empty(), "{errors:?}");
                assert!(matches!(outcome, opusdm_ipc::Outcome::Ok { .. }), "{outcome:?}");
                break;
            }
            Some(_) => continue,
            None => panic!("hub closed before JobDone"),
        }
    }

    let landed = dst_dir.join("payload.txt");
    assert_eq!(std::fs::read(&landed).unwrap(), b"hello opusdm");
    let _ = std::io::stdout().flush();
}
```

- [ ] **Step 3: Expose the hub modules to the integration test**

Add `crates/hub/src/lib.rs`:
```rust
//! Library face of opusdm-hub so integration tests can drive the server
//! layer without the GTK toolbar.

pub mod children;
pub mod jobs;
pub mod roles;
pub mod server;
```
And in `crates/hub/Cargo.toml` add under `[lib]`:
```toml
[lib]
name = "opusdm_hub"
path = "src/lib.rs"
```
`main.rs` keeps its `mod` declarations but changes them to `use opusdm_hub::{children, jobs, roles, server};` and drops the `mod` lines, OR keeps `mod` and the lib re-declares - to avoid double-compile, prefer: `main.rs` uses `use opusdm_hub::...`. The `toolbar` module stays private to the binary (`mod toolbar;` in `main.rs`).

- [ ] **Step 4: Run the integration test**

Run: `cargo test -p opusdm-hub --test integration_copy -- --ignored`
Expected: PASS. Also confirm the default `cargo test --workspace` still passes and does NOT run this test.

- [ ] **Step 5: Write the QA checklist**

`docs/superpowers/QA-checklist.md` - collect every manual checklist item from Tasks 10, 13, 14, 15 into one document, grouped: Startup / Navigation / Roles / Operations / Drag-and-drop / Shutdown. Add the shutdown cases from the spec:
- Hub `Quit` with a job running -> confirm dialog; on confirm all Listers close and the socket file is gone (`ls $XDG_RUNTIME_DIR/opusdm/`).
- `kill -9` the hub -> every Lister exits within ~1s (reader hits EOF).
- Close a Lister that holds SOURCE -> the role clears; toolbar SRC shows "-".

- [ ] **Step 6: Run the whole suite and build the sandbox**

Run: `cargo test --workspace` -> PASS (no ignored test runs).
Run: `cargo build --release --workspace` -> succeeds.
Manual: `./run-x11.sh` -> Xephyr opens with the toolbar and one Lister; walk the QA checklist.

- [ ] **Step 7: Commit**

```bash
git add run-x11.sh crates/hub docs/superpowers/QA-checklist.md CHANGES.md
git commit -m "test(hub): end-to-end copy smoke test; run-x11.sh launches the hub; QA checklist"
```
`CHANGES.md`: `- Task 16: integration_copy smoke test, run-x11.sh runs opusdm-hub, QA-checklist.md. v1 feature-complete.`

---

## Self-Review

**Spec coverage:**

| Spec section | Task(s) |
|---|---|
| Workspace layout, two libs + two bins | 1, 3, 6, 11 |
| IPC transport + framing + JSON | 1 |
| `ToHub` / `ToLister` message tables | 2 |
| Socket path resolution + `ensure_sock_dir` | 2 |
| Single-instance bind / `probe_live_hub` / `RaiseToolbar` | 2, 9 |
| Hub threading (GTK main + socket thread + bridge) | 8, 10 |
| `HubState` single-owner mutation | 8 |
| Role state machine (hybrid rules, locks, singletons) | 6 |
| `SetRole` collision with locked holder -> reject/re-snap | 6, 8 |
| Job engine: validate, JobId, concurrency cap 4, fan-out | 7, 8 |
| `Progress` -> `JobProgress` / `ConflictPrompt` / `JobDone` | 7, 8, 14 |
| `InvalidatePath` broadcast on job completion | 8 |
| Toolbar: New Lister / SRC-DST indicators / job rows / Quit | 10 |
| Child spawn (env var, DISPLAY only), SIGCHLD reaping | 9 |
| Hub death -> Listers self-exit on socket EOF | 11 (reader `HubGone`), 13 (window close) |
| fsops: scan pass, copy w/ mode+mtime, 256 KiB buffer, throttle | 3, 4 |
| fsops: conflict model incl. `*All`, cancel semantics, no rollback | 4 |
| fsops: Move rename + `EXDEV` fallback; Delete permanent + chmod retry | 5 |
| fsops: MakeDir / NewFile | 5 |
| Lister layout: path bar, ColumnView (Name/Size/Modified), dir-first sort | 12, 13 |
| Navigation (double-click, Parent, Root, Refresh, type path) | 13 |
| Status line + role badge | 13 |
| Off-thread directory read | 13 (note: `read_dir_rows` called from a worker; wire in 13) |
| Standalone-launch guard (`OPUSDM_HUB_SOCK` unset) | 11 |
| Button row: Copy/Move/Delete/MakeDir/NewFile/Rename | 14 |
| Copy/Move need SOURCE + DEST; error in status line otherwise | 14 |
| Role buttons Source/Dest/Lock + re-snap on `RoleChanged` | 14 |
| Conflict dialog (Skip/Overwrite/SkipAll/OverwriteAll/Cancel) | 14 |
| Drag-and-drop `text/uri-list`, Shift = Move, drop on dir row | 15 |
| Edge cases (self-copy, dest inside source, perms, socket fallback) | 7 (validate), 2 (fallback), 4 (perms in `errors`) |
| Testing: ipc unit, fsops unit, role table, job engine, integration smoke | 1-8, 16 |
| `run-x11.sh` / Xephyr manual verification | 16 |
| Success criteria 1-10 | 9, 10, 13, 14, 15, 16 |

No spec requirement is left without a task.

**Placeholder scan:** The GTK tasks (10, 13, 14, 15) contain `unimplemented!()` / `unfinished skeleton` markers *inside the plan's illustrative code*, each followed by an explicit instruction that no `unimplemented!`/`todo!` may remain at commit and a pointer to the design notes that specify the full behavior. This is deliberate: GTK view code is not unit-tested (per spec) and the notes enumerate every widget and signal. All pure-logic tasks (1-9, 12, and the pure parts of 14/15) contain complete, runnable code and real tests.

**Type consistency check:**
- `ListerId(pub u64)` - consistent across ipc, roles, jobs, server, client, window, actions.
- `ToHub` / `ToLister` variant names and fields - defined once in Task 2, referenced verbatim later.
- `Progress` variants (`Scanning`/`File`/`Conflict`/`Done`) - defined in Task 3, matched in Tasks 4, 5, 7.
- `Outcome` (`Ok`/`Cancelled`/`Error`) - Task 2, produced in Task 7 `map_progress`, consumed in Task 14 `on_job_done`.
- `Runner = Arc<dyn Fn(JobSpec, Sender<(u64, Progress)>, CancelToken)>` - Task 7; the job-id tagging wrinkle is called out and fixed in Task 7 Step 4.
- `HubEvent` / `HubCmd` - defined in Task 8; `HubCmd::ChildExited(u32)` added in Task 8 Step 3 and used by Task 9 `install_sigchld`.
- `Ui` struct - introduced in Task 13, extended in Task 14 (role buttons + guard); the extension is stated explicitly.
- `resolve_copy_target` signature - Task 14, matches its test.
- `CancelToken` - Task 3, used unchanged in Tasks 4, 5, 7.

Fixed inline: none required beyond what each task's steps already specify.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-31-opusdm-lister.md`. Two execution options:

1. **Subagent-Driven (recommended)** - dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** - execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
