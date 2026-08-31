# OpusDM Lister - Design (v1)

Amiga Directory Opus 5.5-style file manager for Linux, written in Rust + GTK4.

Scope of this spec: the **file manager (Lister) window** plus the **always-on
hub** that coordinates multiple Listers. This supersedes the earlier
`2026-08-31-opusdm-design.md` (a full WM + panel desktop environment), which is
out of scope here.

## Goals

- A single-pane file-manager window ("Lister") the user can open many times.
- Any Lister can be tagged SOURCE or DEST. Copy / Move act SOURCE -> DEST.
- Role selection is **hybrid**: auto by focus, plus explicit buttons and a
  per-Lister lock to pin a role against focus changes.
- One always-on controller app ("hub"). Closing the hub tears down everything.

## Non-goals (v1)

Dual-pane listers, tabs, session/layout persistence, multi-file pattern rename,
config file / themes, custom toolbars / button banks, archive handling
(lha/zip), FTP / network filesystems, inotify live-watch, trash (Delete is
permanent), undo, Wayland-specific tweaks (runs under XWayland).

## Architecture

Cargo workspace, three binaries + two plain libraries:

```
opusdm/
├── crates/
│   ├── ipc/      → lib "opusdm-ipc": wire types, framing, socket path, client/server helpers
│   ├── fsops/    → lib "opusdm-fsops": copy/move/delete/mkdir walkers, progress events, conflict model
│   ├── hub/      → bin "opusdm-hub": always-on controller (GTK4 toolbar + socket daemon)
│   └── lister/   → bin "opusdm-lister": one file-manager window (GTK4), spawned by the hub
└── docs/superpowers/specs/...
```

- `opusdm-ipc` and `opusdm-fsops` have no GTK and no socket I/O of their own
  (fsops takes channels); both are unit-testable in isolation.
- `hub` and `lister` share **only** `opusdm-ipc`. No direct dependency between
  them.

### Process topology

- **Hub** is the parent process. It owns:
  - the Unix domain socket,
  - the registry of live Listers,
  - the global SOURCE / DEST roles,
  - the job engine (all file I/O runs here).
  It has one GTK4 toolbar window.
- **Lister** is a child process. On launch it connects to the socket (path
  passed via env var), registers, receives a `ListerId`, then renders whatever
  the hub tells it. It holds no authority over roles or jobs - it sends
  requests and reacts to hub messages.
- Multiple Listers = multiple `opusdm-lister` processes, one window each.

### Hub threading

- **Main thread:** GTK4 application + toolbar window.
- **Socket thread:** `std::os::unix::net::UnixListener` accept loop; one
  reader thread per connected Lister; the job engine's blocking workers.
- **Bridge:** `async-channel` (or `std::sync::mpsc` + `glib` idle source) in
  both directions. Socket side pushes `HubEvent` to GTK; GTK pushes `HubCmd`
  to the socket side. GTK side is drained via `glib::MainContext` on the main
  loop.
- **Authoritative state** lives on the socket side, mutated by a single
  owner (one thread / one `mpsc` consumer loop) so there are no locks and
  event ordering is deterministic:

```rust
struct HubState {
    listers: HashMap<ListerId, ListerHandle>, // tx handle, pid, current_path, role, locked
    source: Option<ListerId>,
    dest: Option<ListerId>,
    jobs: HashMap<JobId, JobHandle>,          // status, CancellationToken
    next_lister_id: u64,
    next_job_id: u64,
}
```

### Lister threading

- **Main thread:** GTK4 window.
- **Socket reader thread:** plain `std::os::unix::net::UnixStream`; blocking
  read loop that decodes frames and forwards them to the GTK thread via a
  `glib` channel. No tokio in the Lister.
- **Directory-read worker:** `gio::spawn_blocking` (or a `std::thread`) walks
  `std::fs::read_dir`, builds row structs, returns them over a channel; the
  view model is swapped atomically.

## IPC protocol (`opusdm-ipc`)

**Transport:** Unix domain socket, stream. One persistent connection per
Lister.

**Framing:** `u32` big-endian length prefix + JSON body (`serde_json`). JSON
for v1 (debuggable, cheap). The frame reader must handle partial reads and
multiple frames per `recv`.

**Interaction model:** asynchronous message passing, not request/response RPC.
Either side may send a `Msg` at any time. A reply is correlated with an
optional `req_id: u64` (used only for conflict prompts).

**Socket path:** `$XDG_RUNTIME_DIR/opusdm/hub.sock`. If `XDG_RUNTIME_DIR` is
unset or unwritable, fall back to `/tmp/opusdm-$UID/hub.sock`. Directory is
created mode `0700`.

**Single-instance:** the hub tries to `bind`. On `EADDRINUSE`, it connects and
sends `Ping`:
- live reply -> another hub already runs; the new invocation sends
  `RaiseToolbar` (a hub-to-hub `Msg` variant) and exits 0.
- no reply / connection refused -> stale socket; `unlink` and rebind.

### Lister -> Hub

| Msg | Payload |
|---|---|
| `Register` | `pid: u32`, `initial_path: PathBuf` |
| `Ping` | (single-instance probe; hub replies `Pong`) |
| `Goodbye` | clean shutdown of this Lister |
| `SetRole` | `Source` \| `Dest` \| `None` (explicit button; hub applies hybrid rules) |
| `LockRole` / `UnlockRole` | pin / unpin this Lister's current role against focus steal |
| `FocusGained` | window received focus; hub applies auto SOURCE/DEST if unlocked |
| `PathChanged` | `PathBuf` (Lister navigated; hub updates `current_path`) |
| `RunJob` | `op: Op`, `srcs: Vec<PathBuf>`, `dest_dir: PathBuf` |
| `SpawnLister` | `start_path: Option<PathBuf>` ("New Lister" from a Lister menu) |
| `ConflictReply` | `req_id: u64`, `Resolution` |

### Hub -> Lister

| Msg | Payload |
|---|---|
| `Welcome` | `lister_id: ListerId`, current global roles |
| `Pong` | single-instance probe reply |
| `Present` | ask this Lister to raise / focus its window |
| `RoleChanged` | `source: Option<ListerId>`, `dest: Option<ListerId>` |
| `JobProgress` | `job_id`, `op`, `current_file: PathBuf`, `bytes_done`, `bytes_total`, `files_done`, `files_total` |
| `JobDone` | `job_id`, `Outcome` (`Ok { copied, skipped }` \| `Cancelled { partial }` \| `Error { message }`), `errors: Vec<(PathBuf, String)>` |
| `ConflictPrompt` | `req_id`, `job_id`, `path: PathBuf`, `src_meta: FileMeta`, `dst_meta: FileMeta` |
| `InvalidatePath` | `PathBuf` (a directory changed; Lister reloads if it is showing it) |
| `Shutdown` | graceful-exit request |

`ListerId` is a `u64` newtype. `FileMeta` = `{ size: u64, modified: SystemTime }`.

## Hub toolbar window

Thin always-on-top strip, no maximize, minimal height. GTK4.

- **`New Lister`** button -> spawn `opusdm-lister` child.
- **SOURCE indicator** and **DEST indicator**: each shows the holding Lister's
  short current path, or "-" if unassigned. Clicking an indicator asks that
  Lister to raise its window via the `Present` message (Hub -> Lister).
- **Job area**: one row per active job = label (op + counts) + progress bar +
  `Cancel`. Collapsed when idle. On a job that finished with errors, the row
  stays until clicked; click opens an error-list dialog.
- **`Quit`**:
  - if jobs are running -> confirm dialog ("N jobs running, quit anyway?"),
  - broadcast `Shutdown`, wait up to 2 s for processes to exit,
  - `SIGKILL` stragglers, `unlink` the socket, exit.

### Spawning Listers

`std::process::Command`. The child inherits `DISPLAY` / `WAYLAND_DISPLAY` and
gets `OPUSDM_HUB_SOCK=<path>` in its environment; nothing else is passed
through. The hub records the child `pid`.

### Child lifecycle

- Hub installs a `SIGCHLD` handler (via `signal-hook` on the socket thread, or
  `waitpid(WNOHANG)` polling): on child exit it removes the Lister from the
  registry and, if that Lister held SOURCE or DEST, clears that role and
  broadcasts `RoleChanged`. The hub keeps running with zero Listers.
- On **any** hub exit path, best-effort `Shutdown` broadcast.
- A Lister whose socket read returns EOF (hub gone) exits its process
  immediately - belt and braces so nothing is orphaned.

## Role state machine (hybrid)

Roles are global singletons: at most one SOURCE, at most one DEST, either may
be `None`. `SetRole` / focus events are fed to a **pure** function so it can be
table-tested without sockets or GTK:

```rust
fn apply_role_event(state: &RoleState, ev: RoleEvent) -> (RoleState, Vec<RoleEffect>)
```

Rules:

- `FocusGained(L)`, `L` unlocked:
  - `L` becomes SOURCE.
  - the previous SOURCE, if it is not `L` and is unlocked, becomes DEST.
  - a locked role holder keeps its role and is skipped entirely.
  - if `L` was the DEST and is now SOURCE, DEST becomes `None` unless the
    demoted old SOURCE fills it.
- `SetRole(L, role)` (explicit button):
  - if `role` is currently held by a **locked** other Lister -> reject;
    emit `RoleChanged` with unchanged state so the requesting UI re-snaps.
  - otherwise assign; if the other global role was held by `L`, clear it.
- `LockRole(L)` / `UnlockRole(L)`: toggle `locked` on `L`'s handle; no role
  movement.
- `ListerDropped(L)`: clear any role `L` held; emit `RoleChanged`.

`RoleEffect` values are `Broadcast(RoleChanged)` and `RejectTo(ListerId)`.

## Lister window

GTK4 top-level, resizable, window title = current path.

### Layout (top to bottom)

1. **Path bar**: editable `GtkEntry` (type a path, press Enter to navigate) +
   `Parent` + `Root` + `Refresh` buttons.
2. **File view**: `GtkColumnView` backed by `GtkSortListStore` +
   `GtkMultiSelection`. Columns:
   - **Name** - icon + text. Directories sort before files regardless of the
     active sort column.
   - **Size** - right-aligned, human units (`1.4M`); blank for directories.
   - **Modified** - `YYYY-MM-DD HH:MM`.
   Clicking a header sorts by that column; clicking again reverses.
3. **Button row** (Directory Opus style), acting on this Lister's selection:
   `Copy` `Move` `Delete` `MakeDir` `NewFile` `Rename` | `Source` `Dest` `Lock`
   (the last three are toggles reflecting hub state).
4. **Status line**: `N items, M selected, X bytes` + a role badge
   (`SOURCE` / `DEST` / none).

### Behaviour

- Double-click a directory / press Enter -> navigate; send `PathChanged`.
- Double-click a file -> `xdg-open`.
- `Parent` -> go to `current_path.parent()`; `Root` -> `/`.
- **`Copy` / `Move`**: this Lister must be SOURCE. Send
  `RunJob { op, srcs: <selected paths>, dest_dir: <DEST Lister's current_path> }`.
  If there is no DEST, or this Lister is not SOURCE -> inline error in the
  status line, no job sent. The hub re-validates before scanning.
- **`Delete`** -> confirm dialog (item count + total size) -> `RunJob { Delete }`.
- **`MakeDir` / `NewFile`** -> name prompt -> `RunJob` -> hub creates the
  entry -> `InvalidatePath` refreshes the view.
- **`Rename`** -> inline editable Name cell, single selection only in v1
  (multi-selection rename is a non-goal).
- **Drag and drop**: drag = selected rows, payload `text/uri-list`.
  - Drop on another Lister's file view -> that Lister sends `RunJob { Copy }`
    (with `Shift` held: `Move`) using its own `current_path` as `dest_dir`.
  - Drop on a directory row within the same Lister -> into that directory.
- `FocusIn` -> send `FocusGained`. `Source` / `Dest` buttons -> `SetRole`.
  `Lock` -> `LockRole` / `UnlockRole`.
- `RoleChanged` from hub -> update the badge and the toggle button states.
- `InvalidatePath(p)` -> if `p == current_path`, reload, preserving selection
  by entry name.

### Directory read

Off the GTK thread: a worker walks `std::fs::read_dir`, builds row structs
(name, is_dir, size, mtime, symlink flag), returns them over a channel; the
view model is replaced atomically. No live filesystem watch in v1 - refresh is
hub-driven (`InvalidatePath`) or manual (`Refresh` button).

### Standalone launch

If `OPUSDM_HUB_SOCK` is not set, `opusdm-lister` prints `run opusdm-hub` and
exits 1.

## fsops library + job engine

### `opusdm-fsops` (pure: no GTK, no sockets)

```rust
enum Op { Copy, Move, Delete, MakeDir { name: String }, NewFile { name: String } }

struct JobSpec { op: Op, srcs: Vec<PathBuf>, dest_dir: PathBuf }

enum Resolution { Skip, Overwrite, SkipAll, OverwriteAll, Cancel }

enum Progress {
    Scanning { files: u64, bytes: u64 },
    File { path: PathBuf, done_bytes: u64, total_bytes: u64, files_done: u64, files_total: u64 },
    Conflict { path: PathBuf, src: FileMeta, dst: FileMeta, reply: oneshot::Sender<Resolution> },
    Done { copied: u64, skipped: u64, errors: Vec<(PathBuf, String)> },
}

fn run_job(spec: JobSpec, progress_tx: Sender<Progress>, cancel: CancellationToken);
```

`oneshot` / `Sender` here are plain `std::sync::mpsc` plus a one-shot wrapper.
`CancellationToken` is a small `Arc<AtomicBool>` newtype defined in `fsops`
(no tokio anywhere in the workspace).

### Algorithm

1. **Scan pass** - walk `srcs` recursively, count files and total bytes, emit
   `Scanning`. Directories are recreated at the destination. Symlinks are
   copied as links (no target follow) in v1.
2. **Work pass** - depth-first. For each regular file, if the destination
   exists, emit `Conflict` and block on `reply`. `SkipAll` / `OverwriteAll`
   are cached and applied to the rest of the job. Copy uses `std::io::copy`
   with a 256 KiB buffer; emit `File` on buffer flushes, throttled to about
   20 events per second. Preserve mtime and unix mode. No `fsync` in v1.
3. **Move** - try `std::fs::rename` first (same-filesystem fast path). On
   `EXDEV`, copy then delete the source. A source directory is removed only
   after all of its children have succeeded.
4. **Delete** - recursive, permanent (no trash). `Conflict` is not used. A
   read-only file is retried once after `chmod +w`; still failing -> recorded
   as an error.
5. **MakeDir / NewFile** - create the single entry under `dest_dir`; error if
   it already exists.
6. **Cancel** - checked between files and inside the copy loop. On cancel:
   stop, remove the partially written destination file, emit
   `Done { .. }` with partial counts. Already-completed files are left in
   place (no rollback in v1).

### Errors

Never panic on I/O. Per-entry failures are collected as
`(PathBuf, errno-string)` into `Done.errors` and the job continues. A job
reports `Error` only on setup failure (destination not writable, all sources
vanished).

### Job engine (in the hub)

- `RunJob` -> **validate**: reject if `dest_dir` is inside a source tree, if
  a source equals the destination, if the requesting Lister is not SOURCE, or
  if there is no DEST. On rejection, send `JobDone { Error }` back to the
  Lister without scanning.
- On accept -> assign a `JobId`, create a `CancellationToken`, run
  `run_job` on a blocking worker thread.
- `Progress` events map to protocol messages:
  - `Scanning` / `File` -> `JobProgress` to the originating Lister and to the
    toolbar job list.
  - `Conflict` -> `ConflictPrompt { req_id }` to the originating Lister; the
    matching `ConflictReply` resolves the one-shot.
  - `Done` -> `JobDone` to the Lister and the toolbar, then `InvalidatePath`
    for `dest_dir` (and, for Move / Delete, each source's parent directory)
    broadcast to **all** Listers.
- Concurrency: each job is one blocking worker; at most 4 run at once, the
  rest queue.

## Edge cases

- No DEST set on Copy / Move -> status-line error, no job.
- Source equals destination, or destination is inside a source tree -> hub
  rejects `RunJob` before the scan.
- DEST Lister closes mid-job -> the job continues (the hub owns it);
  `InvalidatePath` simply finds no listener for that path.
- Permission denied on an entry -> recorded in `Done.errors`, job continues;
  toolbar row shows "completed with N errors", click -> error-list dialog.
- Socket directory unwritable and `/tmp` fallback also fails -> hub exits with
  a clear message on stderr.
- `opusdm-lister` started without `OPUSDM_HUB_SOCK` -> prints guidance, exits 1.
- Broken symlink in a listing -> shown with a marker, size blank.
- Very large directory (~100k entries) -> scan is already off-thread;
  `GtkColumnView` is virtualized, so rendering is unaffected.

## Testing

- **`opusdm-ipc`** (pure unit): encode/decode round-trip for every `Msg`;
  framing against partial reads and multiple frames per buffer; stale-socket
  detection logic.
- **`opusdm-fsops`** (pure unit, `tempfile`): copy a tree; move same-filesystem
  vs. simulated `EXDEV` (inject the rename function); delete a read-only file;
  every `Resolution` including `SkipAll` / `OverwriteAll`; cancel mid-copy
  leaves a consistent tree (no partial dest file); mtime and mode preserved;
  symlink copied as a link. This is the bulk of the suite.
- **Hub role state machine** (pure unit): table-test `apply_role_event` across
  all transitions - focus with and without locks, `SetRole` collisions with a
  locked holder, `ListerDropped` clearing a role.
- **Hub job engine** (unit with a scripted fake `run_job`): assert the
  `JobProgress` / `ConflictPrompt` / `InvalidatePath` fan-out and the
  4-job concurrency cap.
- **GTK / Lister UI**: no automated tests in v1. A manual checklist runs under
  Xephyr, driven by an updated `run-x11.sh` (mirrors the earlier session
  script).
- **Integration smoke** (`#[ignore]` by default): under `Xvfb`, spawn the hub
  and two Listers, drive a copy from a small test client on the socket, assert
  the files land. One happy-path test.

## Dependencies

- **workspace**: `serde`, `serde_json`, `thiserror`, `anyhow` (binaries only),
  `tracing` + `tracing-subscriber`.
- **ipc**: `serde`, `serde_json`.
- **fsops**: `walkdir` (or a hand-rolled recursive walk), `filetime`;
  `tempfile` (dev-dependency).
- **hub**: `gtk4`, `async-channel`, `signal-hook` (or `nix` for `waitpid`).
  No tokio - the socket layer is `std::os::unix::net` + reader threads. A
  small thread pool (hand-rolled or `std` threads) runs the blocking job
  workers.
- **lister**: `gtk4`, `gio`. Socket is `std::os::unix::net::UnixStream` + one
  reader thread bridged to GTK via a `glib` channel.

## Success criteria (v1)

1. `opusdm-hub` opens the toolbar plus one Lister at `$HOME`.
2. `New Lister` opens additional Listers, each its own process.
3. Focusing a Lister makes it SOURCE and the previous SOURCE becomes DEST
   (badges update). `Lock` pins a role against focus changes. `Source` /
   `Dest` buttons force a role.
4. Selecting files in the SOURCE Lister and pressing `Copy` puts them in the
   DEST Lister's directory; the toolbar shows progress; both Listers refresh.
5. `Move` on the same filesystem is instant (rename); across filesystems it
   copies then deletes.
6. A name collision raises a prompt with Skip / Overwrite / SkipAll /
   OverwriteAll / Cancel.
7. `Delete` asks for confirmation, then permanently deletes.
   `MakeDir` / `NewFile` / `Rename` (single selection) work and refresh.
8. Dragging files from one Lister to another copies them (`Shift` = move).
9. Closing a Lister ends only that process; any role it held is reassigned.
10. Hub `Quit` removes all Listers and cleans up the socket; killing the hub
    causes the Listers to self-exit.
