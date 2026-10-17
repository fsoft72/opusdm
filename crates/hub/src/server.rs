//! The socket server: an accept loop, one reader thread per Lister, and a
//! single owner loop that holds all mutable hub state. Only channels cross
//! thread boundaries.

use crate::channels::send_or_log;
use crate::jobs::{default_runner, map_progress, validate_op, JobEngine, ProgressTotals};
use crate::script_events::{EventGuard, ScriptEvent, SHUTDOWN_HOOK_TIMEOUT};
use crate::roles::{apply_role_event, RoleEffect, RoleEvent, RoleState};
use crate::settings;
use crate::desktop_settings;
use opusdm_fsops::{CancelToken, Progress};
use opusdm_ipc::bindings::{self, Bindings};
use opusdm_ipc::format::{self, FormatStore};
use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::function::{FnStep, FunctionId, SelectionCtx};
use opusdm_ipc::{DesktopSettings, ListerId, Op, Outcome, Resolution, ToHub, ToLister, Welcome};
use std::collections::HashMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod function_run;
mod function_runs;
mod persist;
pub use persist::FLUSH_TIMEOUT as PERSIST_FLUSH_TIMEOUT;

use function_run::{only_scripts, NO_SOURCE_ERROR};
use function_runs::FunctionRuns;
use persist::PersistMsg;

/// Called when a Lister or the toolbar asks for a new Lister window.
pub type SpawnFn = Box<dyn Fn(Option<PathBuf>) + Send>;

/// The home directory, or `/` when `HOME` is unset: the working directory of
/// a run that has no SOURCE Lister.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Events the server pushes to the GTK toolbar.
#[derive(Debug)]
pub enum HubEvent {
    /// A Lister registered.
    ListerRegistered { id: ListerId, pid: u32, path: PathBuf },
    /// A Lister disconnected.
    ListerGone(ListerId),
    /// Current roles and every Lister's path.
    Roles {
        source: Option<ListerId>,
        dests: Vec<ListerId>,
        paths: Vec<(ListerId, PathBuf)>,
    },
    /// Progress for a running job.
    JobUpdate {
        job_id: u64,
        current_file: PathBuf,
        files_done: u64,
        files_total: u64,
        file_bytes_done: u64,
        file_bytes_total: u64,
    },
    /// A job finished.
    JobFinished { job_id: u64, ok: bool, errors: usize, cancelled: bool },
    /// No Listers remain open.
    AllListersGone,
    /// A redundant hub invocation asked us to raise the toolbar.
    RaiseToolbar,
    /// The saved Functions changed (a Lister saved them): the full new list,
    /// for the GTK side's bank buttons and bank editor.
    FunctionsChanged(Vec<opusdm_ipc::function::Function>),
    /// Desktop settings changed (from `opus-desktop-config`). Carries the
    /// sanitised value; the GTK side repaints and re-lays the desktop.
    DesktopSettingsChanged(DesktopSettings),
    /// Theme changed (from `opusdm-settings`). Carries the new theme; the GTK
    /// side repaints the desktop background and re-renders desktop icons.
    ThemeChanged(Box<opusdm_ipc::Theme>),
    /// Appearance changed (from `opusdm-settings`). Carries the new value;
    /// the GTK side applies `appearance.theme_mode` live via
    /// `opusdm_ui::apply_theme_mode`.
    AppearanceChanged(opusdm_ipc::Appearance),
    /// Paths to leave out on the desktop (already checked to exist). The GTK
    /// side adds them to `leftout.json` and refreshes the icon field.
    LeftOutAdd(Vec<PathBuf>),
    /// Paths to put away: remove them from `leftout.json` and refresh.
    LeftOutRemove(Vec<PathBuf>),
    /// The desktop was hidden (`true`) or revealed (`false`): the GTK side
    /// hides or shows the panel, the dock and the group windows.
    HiddenChanged(bool),
    /// A Function step asked to apply `action` to the bank called `bank`.
    Bank { action: opusdm_ipc::buttonbank::BankAction, bank: String },
    /// Open the button bank manager window.
    OpenBankManager,
}

/// Commands the GTK toolbar (or the SIGCHLD handler) sends to the server.
#[derive(Debug)]
pub enum HubCmd {
    /// Open another Lister.
    SpawnRequested { start_path: Option<PathBuf> },
    /// Run a file operation from the hub GUI itself (a drop on the desktop
    /// window). The destination is always concrete.
    RunJob { op: Op, srcs: Vec<PathBuf>, dest_dir: PathBuf },
    /// Cancel a running or queued job.
    CancelJob(u64),
    /// Ask a Lister to raise its window.
    PresentLister(ListerId),
    /// Hide (`true`) or reveal (`false`) the whole desktop environment.
    SetHidden(bool),
    /// Flip the hidden state (desktop menu).
    ToggleHidden,
    /// Run a saved Function on the hub's own initiative (global hotkey,
    /// script event): SOURCE is the current SOURCE Lister, or an empty
    /// selection when there is none.
    RunFunctionHeadless(opusdm_ipc::function::FunctionId),
    /// Run a saved Function or inline command on the given files, with no
    /// Lister (a double click on a desktop icon that has a file type).
    RunFunctionOnPaths { id: opusdm_ipc::function::FunctionId, paths: Vec<std::path::PathBuf> },
    /// Emit a script event: run every function bound to it (from `main` for
    /// `Startup`, from the desktop for the double clicks).
    EmitScriptEvent(ScriptEvent),
    /// Tear everything down, then send `()` on the channel once the
    /// `Shutdown` hooks have finished. The GTK quit path waits on it so the
    /// process does not exit (and kill a running hook) before they are done.
    ShutdownAndWait(Sender<()>),
    /// Tear everything down.
    Shutdown,
    /// A child process exited (from the SIGCHLD reaper).
    ChildExited(u32),
}

/// Channels the GTK side keeps to talk to the server.
pub struct ServerHandle {
    /// Send commands to the server.
    pub cmd_tx: Sender<HubCmd>,
    /// Receive events from the server.
    pub evt_rx: Receiver<HubEvent>,
}

/// Knobs the tests leave off and `main` turns on.
pub struct ServerOptions {
    /// Grab the global hotkeys over X11. Off in tests, which must never touch
    /// a real X server.
    pub global_keys: bool,
    /// Bindings to start with. `None` loads them from disk (production). The
    /// default is empty bindings, so a test never reads, or acts on, a
    /// developer's real hotkeys, menu and script events.
    pub initial_bindings: Option<Bindings>,
    /// Formats to start with. `None` loads `formats.json` from disk
    /// (production). The default is an empty store so a test never reads a
    /// developer's real formats.
    pub initial_formats: Option<FormatStore>,
    /// Functions to start with. `None` loads `functions.json` from disk.
    pub initial_functions: Option<Vec<opusdm_ipc::function::Function>>,
}

impl Default for ServerOptions {
    /// Options for tests: no disk read.
    fn default() -> Self {
        Self {
            global_keys: false,
            initial_bindings: Some(Bindings::default()),
            initial_formats: Some(FormatStore::default()),
            initial_functions: None,
        }
    }
}

impl ServerOptions {
    /// Options for the real hub: load the bindings from disk.
    pub fn production() -> Self {
        Self { global_keys: true, initial_bindings: None, initial_formats: None, initial_functions: None }
    }
}

/// Start the socket server on `listener` with test-friendly options. Returns
/// channels to/from the GTK side.
pub fn spawn_server(listener: UnixListener, on_spawn: SpawnFn) -> ServerHandle {
    spawn_server_with(listener, on_spawn, ServerOptions::default())
}

/// Start the socket server on `listener`. Returns channels to/from the GTK side.
pub fn spawn_server_with(listener: UnixListener, on_spawn: SpawnFn, opts: ServerOptions) -> ServerHandle {
    let (owner_tx, owner_rx) = channel::<OwnerMsg>();
    let (evt_tx, evt_rx) = channel::<HubEvent>();
    let (cmd_tx, cmd_rx) = channel::<HubCmd>();

    // Accept loop.
    {
        let owner_tx = owner_tx.clone();
        std::thread::Builder::new()
            .name("opusdm-accept".into())
            .spawn(move || accept_loop(|| listener.accept().map(|(s, _)| s), &owner_tx, ACCEPT_BACKOFF))
            .expect("spawn accept thread");
    }
    // GTK command forwarder.
    {
        let owner_tx = owner_tx.clone();
        std::thread::Builder::new()
            .name("opusdm-gtk-cmd".into())
            .spawn(move || {
                for cmd in cmd_rx {
                    if owner_tx.send(OwnerMsg::FromGtk(cmd)).is_err() {
                        break;
                    }
                }
                opusdm_ipc::debug_log!("hub", "gtk command forwarder exiting");
            })
            .expect("spawn gtk command forwarder thread");
    }
    // Owner loop.
    std::thread::Builder::new()
        .name("opusdm-owner".into())
        .spawn(move || owner_loop(owner_rx, owner_tx, evt_tx, on_spawn, opts))
        .expect("spawn owner thread");

    ServerHandle { cmd_tx, evt_rx }
}

/// How long the accept loop sleeps after running out of file descriptors or
/// memory, so it does not spin while the condition lasts.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// What the accept loop does after an `accept()` error.
#[derive(Debug, PartialEq, Eq)]
enum AcceptErrorAction {
    /// A per-connection error (aborted handshake, signal): try again at once.
    Retry,
    /// A resource limit (EMFILE, ENFILE, ENOBUFS, ENOMEM): wait, then retry.
    Backoff,
    /// The listening socket itself is unusable: stop accepting.
    Stop,
}

/// Sort an `accept()` error into retry, back off or stop. Unknown errors
/// are treated as temporary: a hub that stops accepting is unusable, while a
/// retry costs one log line per [`ACCEPT_BACKOFF`].
fn classify_accept_error(e: &std::io::Error) -> AcceptErrorAction {
    match e.raw_os_error() {
        Some(libc::EBADF | libc::EINVAL | libc::ENOTSOCK | libc::EOPNOTSUPP | libc::EFAULT) => AcceptErrorAction::Stop,
        Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM) => AcceptErrorAction::Backoff,
        Some(_) => AcceptErrorAction::Retry,
        None => AcceptErrorAction::Backoff,
    }
}

/// Accept connections with `accept` and hand each one to the owner loop.
/// Temporary errors are logged and retried (after `backoff` for a resource
/// limit); the loop ends on a fatal error or when the owner loop is gone.
fn accept_loop(mut accept: impl FnMut() -> std::io::Result<UnixStream>, owner_tx: &Sender<OwnerMsg>, backoff: Duration) {
    loop {
        let e = match accept() {
            Ok(s) => {
                if owner_tx.send(OwnerMsg::Accepted(s)).is_err() {
                    return;
                }
                continue;
            }
            Err(e) => e,
        };
        match classify_accept_error(&e) {
            AcceptErrorAction::Retry => opusdm_ipc::err_log!("hub", "accept failed, retrying: {e}"),
            AcceptErrorAction::Backoff => {
                opusdm_ipc::err_log!("hub", "accept failed, retrying in {backoff:?}: {e}");
                std::thread::sleep(backoff);
            }
            AcceptErrorAction::Stop => {
                opusdm_ipc::err_log!("hub", "accept failed, no more clients can connect: {e}");
                return;
            }
        }
    }
}

/// Everything the owner loop merges into one stream.
enum OwnerMsg {
    Accepted(UnixStream),
    FromLister(u64, ToHub),
    ListerEof(u64),
    JobProgress(u64, Progress),
    FromGtk(HubCmd),
    FunctionEvent(u64, crate::functions::FnEvent),
    /// The fit-check worker answered connection `cid`.
    FitChecked(u64, ToLister),
}

/// How many messages may wait for one connection's writer. A Lister reads
/// its socket on a thread of its own, so a full queue means it stopped
/// reading altogether: it is disconnected rather than left to grow the
/// hub's memory without bound.
const WRITER_QUEUE: usize = 4096;

/// One connected peer. `registered` flips true after `Register`.
///
/// Writes go through `tx` to a dedicated writer thread rather than a
/// blocking `send()` on the owner thread, so a stalled or non-reading
/// client cannot stall connection lifecycle, role changes, or job
/// progress for every other Lister. `shutdown` is a separate socket
/// clone kept to tear the connection down from `drop_conn` or when the
/// queue overflows.
struct Conn {
    tx: SyncSender<OutMsg>,
    shutdown: UnixStream,
    registered: bool,
    pid: u32,
    path: PathBuf,
    /// The directories this Lister reported as cached.
    cache_paths: Vec<PathBuf>,
}

impl Conn {
    /// Queue `msg` for this connection's writer; `cid` names it in the log.
    /// A full queue means the peer stopped reading: the socket is shut down,
    /// so its reader reports the end and the hub drops the connection then,
    /// and the message is lost. Returns whether the message was queued.
    fn queue(&self, cid: u64, msg: impl Into<OutMsg>) -> bool {
        match self.tx.try_send(msg.into()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                opusdm_ipc::err_log!("hub", "connection {cid} stopped reading ({WRITER_QUEUE} messages queued): disconnecting it");
                let _ = self.shutdown.shutdown(std::net::Shutdown::Both);
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                opusdm_ipc::debug_log!("hub", "connection {cid} is closing: message dropped");
                false
            }
        }
    }
}

/// What the hub writes to a Lister socket. `Welcome` is sent exactly once,
/// right after `Register`; everything else after that is a `ToLister`, or
/// one already encoded once for every Lister of a broadcast.
enum OutMsg {
    Welcome(Box<Welcome>),
    ToLister(ToLister),
    /// The JSON body of a `ToLister`, shared by every receiver.
    Encoded(Arc<[u8]>),
}

impl From<ToLister> for OutMsg {
    fn from(msg: ToLister) -> Self {
        OutMsg::ToLister(msg)
    }
}

impl From<Welcome> for OutMsg {
    fn from(msg: Welcome) -> Self {
        OutMsg::Welcome(Box::new(msg))
    }
}

/// Owns one Lister's write half; runs on its own thread so a blocking
/// `send()` (a non-reading peer, a full socket buffer) never stalls the
/// owner loop.
fn writer_loop(mut stream: UnixStream, rx: Receiver<OutMsg>) {
    for msg in rx {
        let result = match msg {
            OutMsg::Welcome(w) => send(&mut stream, &*w),
            OutMsg::ToLister(m) => send(&mut stream, &m),
            OutMsg::Encoded(bytes) => opusdm_ipc::frame::write_frame(&mut stream, &bytes),
        };
        if result.is_err() {
            break;
        }
    }
}

/// Per-job bookkeeping the owner keeps until `Done`.
struct JobMeta {
    /// The Lister that asked for the job; `None` for a hub-originated one.
    requester: Option<u64>,
    op: Op,
    totals: ProgressTotals,
    dest_dir: PathBuf,
    src_parents: Vec<PathBuf>,
    /// A Function step waiting for this job: told the outcome on `Done`.
    waiter: Option<Sender<crate::functions::JobResult>>,
}

/// All mutable hub state, owned by the owner loop. Splitting the loop's big
/// match into methods on this struct keeps each concern - connection
/// lifecycle, authentication/registration, role transitions, settings
/// persistence, job submission, conflict replies, GTK commands - in its own
/// handler, where its invariants are easier to follow and test.
struct Hub {
    conns: HashMap<u64, Conn>,
    /// Connection ids only ever grow: a recycled id would let the stale
    /// `ListerEof` / `FromLister` messages of a dead reader thread hit a
    /// new Lister.
    next_conn: u64,
    roles: RoleState,
    /// Loaded once at startup; kept in memory and updated in place by
    /// `ToHub::SetSettings`. Never re-read from disk after this.
    current_settings: opusdm_ipc::Settings,
    /// Same lifecycle for desktop settings; updated in place by
    /// `ToHub::SetDesktopSettings` and pushed to the GTK side.
    current_desktop: DesktopSettings,
    /// Whether the desktop is hidden (Lister windows minimized, panel and dock
    /// hidden). Not persisted: the hub always starts revealed.
    hidden: bool,
    /// Lister spawns someone asked for (desktop menu, "New Lister") that have
    /// not registered yet. A Lister registering while hidden reveals the
    /// desktop only when it was asked for; the initial Lister of a `Startup`
    /// script that hides the desktop was not, and just hides with the rest.
    visible_spawns_pending: u32,
    /// Loaded once at startup; updated in place by `ToHub::SetBindings`.
    current_bindings: Bindings,
    /// Loaded once at startup; updated in place by `ToHub::SetFormats`.
    current_formats: FormatStore,
    /// The saved Functions. Loaded once at startup; updated in place by
    /// `ToHub::SetFunctions`, which is the only writer of `functions.json`.
    current_functions: Vec<opusdm_ipc::function::Function>,
    /// Loop protection for script events.
    event_guard: EventGuard,
    /// Whether global hotkeys may be grabbed (off in tests).
    global_keys_enabled: bool,
    /// Sender back into the owner loop, used by the global-key thread.
    owner_tx: Sender<OwnerMsg>,
    /// The X11 grabs, created lazily on the first non-empty global set.
    global_keys: Option<crate::global_keys::GlobalKeys>,
    /// The Wayland global hotkeys (labwc keybinds or the portal), created
    /// lazily like the X11 grabs.
    wayland_keys: Option<crate::global_keys_wl::GlobalKeysWl>,
    /// The X11 connection (or, on Wayland, every global hotkey backend)
    /// failed once; do not retry or log again.
    x11_failed: bool,
    persist_tx: Sender<PersistMsg>,
    engine: JobEngine,
    jobs: HashMap<u64, JobMeta>,
    replies: HashMap<u64, Sender<Resolution>>,
    /// Connection that must answer each conflict prompt in `replies` (by
    /// request id), so a closed Lister releases the job waiting on it.
    conflict_owners: HashMap<u64, u64>,
    /// Pending `Confirm` questions: request id to (answer channel, asked Lister).
    confirms: HashMap<u64, (Sender<bool>, u64)>,
    /// Which Lister shows each `Run` output window, by `(run id, step)`. It
    /// outlives the function run that opened it.
    output_windows: HashMap<(u64, usize), u64>,
    next_req: u64,
    evt_tx: Sender<HubEvent>,
    on_spawn: SpawnFn,
    /// Function runs: pending selections, requesters and cancel tokens.
    runs: FunctionRuns,
    /// Feeds the fit-check worker.
    fit_tx: Sender<FitRequest>,
}

impl Hub {
    /// Send `msg` to every registered Lister.
    fn broadcast(&mut self, msg: ToLister) {
        broadcast(&self.conns, &msg);
    }

    /// A new peer connected: allocate a connection id, spawn its writer and
    /// reader threads. A spawn failure drops just this connection.
    fn accepted(&mut self, stream: UnixStream, owner_tx: &Sender<OwnerMsg>) {
        let cid = self.next_conn;
        self.next_conn += 1;
        let (Ok(write_stream), Ok(shutdown)) = (stream.try_clone(), stream.try_clone()) else {
            return;
        };
        let (tx, writer_rx) = sync_channel::<OutMsg>(WRITER_QUEUE);
        let writer_spawned = std::thread::Builder::new()
            .name(format!("opusdm-writer-{cid}"))
            .spawn(move || writer_loop(write_stream, writer_rx));
        if let Err(e) = writer_spawned {
            opusdm_ipc::err_log!("hub", "cannot spawn writer thread for conn {cid}: {e}");
            return;
        }
        self.conns.insert(
            cid,
            Conn { tx, shutdown, registered: false, pid: 0, path: PathBuf::new(), cache_paths: Vec::new() },
        );
        let owner_tx = owner_tx.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("opusdm-reader-{cid}"))
            .spawn(move || reader_loop(cid, stream, owner_tx));
        // A thread-spawn failure (e.g. the process thread limit) must
        // not take down every other Lister connected to this hub;
        // drop just this one and let the caller retry.
        if let Err(e) = spawned {
            opusdm_ipc::err_log!("hub", "cannot spawn reader thread for conn {cid}: {e}");
            self.conns.remove(&cid);
        }
    }

    /// A message arrived from Lister connection `cid`.
    fn lister_msg(&mut self, cid: u64, msg: ToHub) {
        match msg {
            ToHub::Ping => {
                self.send_to(cid, ToLister::Pong);
            }

            ToHub::RaiseToolbar => {
                send_or_log(&self.evt_tx, HubEvent::RaiseToolbar, "GTK event");
            }

            ToHub::SetDesktopSettings(ds) => self.set_desktop_settings(ds),

            ToHub::SetSettings(new_settings) => self.apply_settings(*new_settings),

            ToHub::SetBindings(new_bindings) => self.set_bindings(cid, new_bindings),

            ToHub::SetFormats(new_formats) => self.set_formats(new_formats),

            ToHub::SetFunctions(new_functions) => self.set_functions(new_functions),

            ToHub::QueryConfig => self.query_config(cid),

            ToHub::CacheReport(paths) => {
                if let Some(c) = self.conns.get_mut(&cid) {
                    c.cache_paths = paths;
                }
            }

            ToHub::QueryCaches => self.query_caches(cid),

            ToHub::OpenBankManager => {
                send_or_log(&self.evt_tx, HubEvent::OpenBankManager, "GTK event");
            }

            ToHub::FreeAllCaches => {
                self.broadcast(ToLister::FreeCaches);
            }

            ToHub::Invalidate(path) => {
                self.broadcast(ToLister::InvalidatePath(path));
            }

            ToHub::JumpTo { lister, path } => {
                self.send_to_lister(lister.0, ToLister::GoTo { path });
            }

            ToHub::Register { pid, initial_path, version } => {
                if let Some(warning) = opusdm_ipc::protocol_mismatch(version) {
                    opusdm_ipc::err_log!("hub", "Lister pid {pid}: {warning}");
                }
                self.register_lister(cid, pid, initial_path)
            }

            ToHub::FocusGained => {
                apply_role(&mut self.roles, RoleEvent::FocusGained(ListerId(cid)), &mut self.conns, &self.evt_tx);
            }

            ToHub::SetRole(role) => {
                apply_role(&mut self.roles, RoleEvent::SetRole(ListerId(cid), role), &mut self.conns, &self.evt_tx);
            }

            ToHub::LockRole => {
                apply_role(&mut self.roles, RoleEvent::Lock(ListerId(cid)), &mut self.conns, &self.evt_tx);
            }

            ToHub::UnlockRole => {
                apply_role(&mut self.roles, RoleEvent::Unlock(ListerId(cid)), &mut self.conns, &self.evt_tx);
            }

            ToHub::PathChanged(p) => {
                if let Some(c) = self.conns.get_mut(&cid) {
                    c.path = p;
                }
                emit_roles(&self.roles, &self.conns, &self.evt_tx);
            }

            ToHub::SpawnLister { start_path } => self.spawn_lister(start_path),

            ToHub::RunJob { op, srcs, dest_dir, options } => {
                self.lister_run_job(cid, op, srcs, dest_dir, options);
            }

            ToHub::RunFunction { steps } => match self.run_source_for(cid) {
                Some(source) => {
                    self.start_function_run(Some(cid), Some(source), steps);
                }
                // A script can work without a Lister (it starts in $HOME).
                None if only_scripts(&steps) => {
                    self.start_function_run(Some(cid), None, steps);
                }
                None => self.function_failed(Some(cid), NO_SOURCE_ERROR.to_string()),
            },

            ToHub::RunFunctionById { id } => {
                self.run_function_by_id(Some(cid), Some(cid), &id);
            }

            ToHub::RunFunctionHeadless { id } => {
                self.run_headless(&id);
            }

            ToHub::LeaveOut { paths } => self.leave_out(paths),

            ToHub::SelectionReply { paths, cwd } => {
                self.selection_reply(cid, paths, cwd);
            }

            ToHub::CheckFit { srcs, dest_dir } => {
                self.check_fit(cid, srcs, dest_dir);
            }

            // Tab in a Lister: raise the pane holding the opposite role.
            ToHub::PresentLister(target) => {
                self.send_to_lister(target.0, ToLister::Present);
            }

            ToHub::ConfirmReply { req_id, ok } => {
                if let Some((tx, _)) = self.confirms.remove(&req_id) {
                    let _ = tx.send(ok);
                }
            }

            ToHub::ConflictReply { req_id, resolution } => {
                self.conflict_owners.remove(&req_id);
                if let Some(tx) = self.replies.remove(&req_id) {
                    let _ = tx.send(resolution);
                }
            }

            ToHub::Goodbye => {
                self.drop_lister(cid);
            }
        }
    }

    /// A Lister asked to change the desktop settings: clamp, save and tell the GTK side.
    fn set_desktop_settings(&mut self, ds: DesktopSettings) {
        let ds = ds.sanitized();
        if ds == self.current_desktop {
            return;
        }
        self.current_desktop = ds;
        send_or_log(&self.persist_tx, PersistMsg::Desktop(self.current_desktop.clone()), "desktop.json save");
        send_or_log(&self.evt_tx, HubEvent::DesktopSettingsChanged(self.current_desktop.clone()), "GTK event");
    }

    /// A Lister sent new hotkeys and script bindings: sanitize, save, tell every Lister and re-grab the global keys.
    fn set_bindings(&mut self, cid: u64, new_bindings: Bindings) {
        opusdm_ipc::debug_log!("hub", "SetBindings from conn {cid}: {} hotkeys", new_bindings.hotkeys.len());
        self.current_bindings = bindings::sanitize(new_bindings);
        send_or_log(&self.persist_tx, PersistMsg::Bindings(self.current_bindings.clone()), "bindings save");
        self.broadcast(ToLister::BindingsUpdate(self.current_bindings.clone()));
        self.apply_global_keys();
    }

    /// A Lister sent new formats: sanitize, save and tell every Lister.
    fn set_formats(&mut self, new_formats: FormatStore) {
        self.current_formats = new_formats.sanitized();
        send_or_log(&self.persist_tx, PersistMsg::Formats(self.current_formats.clone()), "formats.json save");
        self.broadcast(ToLister::FormatsUpdate(self.current_formats.clone()));
    }

    /// A Lister sent new functions: save, tell every Lister and the GTK side.
    fn set_functions(&mut self, new_functions: Vec<opusdm_ipc::function::Function>) {
        self.current_functions = new_functions;
        send_or_log(&self.persist_tx, PersistMsg::Functions(self.current_functions.clone()), "functions.json save");
        self.broadcast(ToLister::FunctionsUpdate(self.current_functions.clone()));
        send_or_log(&self.evt_tx, HubEvent::FunctionsChanged(self.current_functions.clone()), "GTK event");
    }

    /// `ToHub::QueryConfig`: send `cid` the bindings and Functions in force.
    fn query_config(&mut self, cid: u64) {
        self.send_to(cid, ToLister::BindingsUpdate(self.current_bindings.clone()));
        self.send_to(cid, ToLister::FunctionsUpdate(self.current_functions.clone()));
    }

    /// Send `msg` to connection `cid`. A connection that is gone or closing
    /// is skipped: its reader reports the end and the hub cleans up then.
    fn send_to(&self, cid: u64, msg: ToLister) {
        let Some(c) = self.conns.get(&cid) else { return };
        c.queue(cid, msg);
    }

    /// [`Self::send_to`], only when `cid` is a registered Lister.
    fn send_to_lister(&self, cid: u64, msg: ToLister) {
        if self.conns.get(&cid).is_some_and(|c| c.registered) {
            self.send_to(cid, msg);
        }
    }

    /// Answer `cid` with the cache folders every registered Lister reported.
    fn query_caches(&mut self, cid: u64) {
        let mut ids: Vec<u64> = self.conns.iter().filter(|(_, c)| c.registered).map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let entries: Vec<opusdm_ipc::CacheEntry> = ids
            .iter()
            .flat_map(|id| {
                self.conns[id]
                    .cache_paths
                    .iter()
                    .map(move |p| opusdm_ipc::CacheEntry { lister: ListerId(*id), path: p.clone() })
            })
            .collect();
        self.send_to(cid, ToLister::CacheList(entries));
    }

    /// `cid` registered: send its `Welcome`, announce it, share the roles and,
    /// while everything is hidden, decide whether it brings the desktop back.
    fn register_lister(&mut self, cid: u64, pid: u32, initial_path: PathBuf) {
        if let Some(c) = self.conns.get_mut(&cid) {
            c.registered = true;
            c.pid = pid;
            c.path = initial_path.clone();
            c.queue(
                cid,
                Welcome {
                    lister_id: ListerId(cid),
                    source: self.roles.source(),
                    dests: self.roles.dests(),
                    source_locked: self.roles.source_locked(),
                    settings: self.current_settings.clone(),
                    bindings: self.current_bindings.clone(),
                    formats: self.current_formats.clone(),
                    functions: self.current_functions.clone(),
                    version: opusdm_ipc::PROTOCOL_VERSION,
                },
            );
            send_or_log(&self.evt_tx, HubEvent::ListerRegistered { id: ListerId(cid), pid, path: initial_path }, "GTK event");
            emit_roles(&self.roles, &self.conns, &self.evt_tx);
        }
        if self.conns.get(&cid).is_some_and(|c| c.registered) {
            // While everything is hidden, a Lister somebody asked for
            // means the user wants to see it: bring the desktop back
            // (without raising the old SOURCE over the new window). One
            // nobody asked for (the initial Lister) hides with the rest.
            let requested = self.visible_spawns_pending > 0;
            self.visible_spawns_pending = self.visible_spawns_pending.saturating_sub(1);
            if self.hidden {
                if requested {
                    self.set_hidden_with(false, false);
                } else {
                    self.send_to(cid, ToLister::SetHidden(true));
                }
            }
            self.emit_script_event(ScriptEvent::ListerOpened);
        }
    }

    /// Drop Lister connection `cid`: forget its Function-run state, tear the
    /// connection down and, when a registered Lister went away, emit
    /// `ListerClosed`.
    fn drop_lister(&mut self, cid: u64) {
        self.drop_function_state(cid);
        let was_registered = drop_conn(cid, &mut self.conns, &mut self.roles, &self.evt_tx);
        if was_registered {
            self.emit_script_event(ScriptEvent::ListerClosed);
        }
    }

    /// The functions bound to `event`, in binding order.
    fn functions_for(&self, event: ScriptEvent) -> Vec<FunctionId> {
        self.current_bindings
            .scripts
            .iter()
            .filter(|s| s.event == event)
            .map(|s| s.function.clone())
            .collect()
    }

    /// Run every function bound to `event` headless. The functions start in
    /// binding order and are not awaited one after another. The emission is
    /// dropped (and logged) by [`EventGuard`] when the same event fired
    /// under `EVENT_COOLDOWN` ago or its previous runs are still going.
    fn emit_script_event(&mut self, event: ScriptEvent) {
        let functions = self.functions_for(event);
        if functions.is_empty() {
            return;
        }
        let now = Instant::now();
        if !self.event_guard.try_begin(event, now) {
            opusdm_ipc::err_log!("hub", "script event {event:?} dropped: it fired again too soon or its previous run is still going");
            return;
        }
        for id in functions {
            if let Some(run_id) = self.run_headless(&id) {
                self.event_guard.add_run(event, run_id, now);
            }
        }
    }

    /// Run the `Shutdown` hooks and wait for them, up to
    /// `SHUTDOWN_HOOK_TIMEOUT`. The Listers are closing, so there is no
    /// `QuerySelection`: each hook runs with an empty selection in `$HOME`.
    fn run_shutdown_hooks(&mut self) {
        let ids = self.functions_for(ScriptEvent::Shutdown);
        if ids.is_empty() {
            return;
        }
        let deadline = Instant::now() + SHUTDOWN_HOOK_TIMEOUT;
        let mut receivers = Vec::new();
        for id in ids {
            // An inline command is waited for: the hub must not quit under it.
            let steps = match function_run::resolve_steps(&self.current_functions, &id, true) {
                Ok(steps) => steps,
                Err(e) => {
                    opusdm_ipc::err_log!("hub", "shutdown hook '{id}' cannot run: {e}");
                    continue;
                }
            };
            let run_id = self.runs.next_id();
            let (tx, rx) = channel();
            let empty = SelectionCtx { paths: Vec::new(), cwd: home_dir() };
            crate::functions::spawn_executor(run_id, steps, empty, None, tx, CancelToken::new());
            receivers.push(rx);
        }
        for rx in receivers {
            loop {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    opusdm_ipc::err_log!("hub", "shutdown hooks did not finish within {SHUTDOWN_HOOK_TIMEOUT:?}");
                    return;
                };
                match rx.recv_timeout(left) {
                    Ok((_, crate::functions::FnEvent::AllDone)) => break,
                    Ok((_, crate::functions::FnEvent::Failed { step_idx, error })) => {
                        opusdm_ipc::err_log!("hub", "shutdown hook failed at step {step_idx}: {error}");
                        break;
                    }
                    // No job engine here: run the job in place, skipping conflicts.
                    Ok((_, crate::functions::FnEvent::Job { spec, done, .. })) => {
                        let result = crate::functions::run_job_inline(spec, &CancelToken::new(), |_, _, _, reply| {
                            let _ = reply.send(Resolution::Skip);
                        });
                        let _ = done.send(result);
                    }
                    Ok(_) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        opusdm_ipc::err_log!("hub", "shutdown hooks did not finish within {SHUTDOWN_HOOK_TIMEOUT:?}");
                        return;
                    }
                }
            }
        }
    }

    /// Bring the X11 grabs in line with the global hotkeys of
    /// `current_bindings`. The X connection is created on the first non-empty
    /// set; if it cannot be opened the hub gives up on global hotkeys for the
    /// rest of the session.
    fn apply_global_keys(&mut self) {
        if !self.global_keys_enabled {
            return;
        }
        let globals: Vec<bindings::Hotkey> =
            self.current_bindings.hotkeys.iter().filter(|h| h.global).cloned().collect();
        if opusdm_ipc::session::is_wayland() {
            self.apply_wayland_global_keys(&globals);
            return;
        }
        if !opusdm_ipc::session::is_x11() {
            return;
        }
        if self.global_keys.is_none() {
            if globals.is_empty() || self.x11_failed {
                return;
            }
            let tx = self.owner_tx.clone();
            let started = crate::global_keys::GlobalKeys::start(Box::new(move |id| {
                let _ = tx.send(OwnerMsg::FromGtk(HubCmd::RunFunctionHeadless(id)));
            }));
            match started {
                Some(keys) => self.global_keys = Some(keys),
                None => {
                    self.x11_failed = true;
                    return;
                }
            }
        }
        if let Some(keys) = &self.global_keys {
            keys.set(&globals);
        }
    }

    /// [`Self::apply_global_keys`] on Wayland: compositor keybinds or the
    /// GlobalShortcuts portal (see `global_keys_wl`), started on first use.
    fn apply_wayland_global_keys(&mut self, globals: &[bindings::Hotkey]) {
        if self.wayland_keys.is_none() && !self.x11_failed {
            let tx = self.owner_tx.clone();
            let started = crate::global_keys_wl::GlobalKeysWl::start(Box::new(move |id| {
                let _ = tx.send(OwnerMsg::FromGtk(HubCmd::RunFunctionHeadless(id)));
            }));
            match started {
                Some(keys) => self.wayland_keys = Some(keys),
                None => {
                    self.x11_failed = true;
                    return;
                }
            }
        }
        if let Some(keys) = &self.wayland_keys {
            keys.set(globals);
        }
    }

    /// `ToHub::CheckFit` from `cid`: hand the selection to the fit-check
    /// worker (sizing a big tree can take a while), which answers through
    /// `OwnerMsg::FitChecked`.
    fn check_fit(&mut self, cid: u64, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf>) {
        if !self.conns.contains_key(&cid) {
            return;
        }
        let dest_dir = dest_dir.or_else(|| {
            self.roles
                .dests()
                .iter()
                .find_map(|d| self.conns.get(&d.0).map(|c| c.path.clone()))
        });
        if self.fit_tx.send(FitRequest { cid, srcs, dest_dir }).is_err() {
            self.send_to(cid, ToLister::CheckFitFailed { message: "the fit-check worker is gone".into() });
        }
    }

    /// Adopt `new_settings`: sanitize the background, persist to
    /// `settings.json`, push the change to every registered Lister and to
    /// the GTK side. Shared by `ToHub::SetSettings` and the `SetBackground`
    /// function step.
    fn apply_settings(&mut self, mut new_settings: opusdm_ipc::Settings) {
        new_settings.theme.background = new_settings.theme.background.sanitized();
        // Nothing changed: no write, no repaint, no message to the Listers.
        if new_settings == self.current_settings {
            return;
        }
        self.current_settings = new_settings;
        send_or_log(&self.persist_tx, PersistMsg::Settings(Box::new(self.current_settings.clone())), "settings.json save");
        self.broadcast(ToLister::SettingsUpdate(Box::new(self.current_settings.clone())));
        send_or_log(&self.evt_tx, HubEvent::ThemeChanged(Box::new(self.current_settings.theme.clone())), "GTK event");
        send_or_log(&self.evt_tx, HubEvent::AppearanceChanged(self.current_settings.appearance.clone()), "GTK event");
    }

    /// Hide or reveal the desktop environment. Does nothing when the state
    /// already matches; otherwise tells every registered Lister (which
    /// minimize or restore themselves) and the GTK side (panel, dock, group
    /// windows).
    fn set_hidden(&mut self, hidden: bool) {
        self.set_hidden_with(hidden, true);
    }

    /// [`Hub::set_hidden`], with control over the focus on reveal: the Listers
    /// restore themselves without raising (raising them in map order would
    /// hand the focus, and with it the SOURCE role, to a random one), so when
    /// `present_source` is set the hub afterwards raises only the SOURCE
    /// Lister, which keeps the roles as they were.
    fn set_hidden_with(&mut self, hidden: bool, present_source: bool) {
        if self.hidden == hidden {
            return;
        }
        self.hidden = hidden;
        self.broadcast(ToLister::SetHidden(hidden));
        send_or_log(&self.evt_tx, HubEvent::HiddenChanged(hidden), "GTK event");
        if hidden || !present_source {
            return;
        }
        if let Some(source) = self.roles.source() {
            self.send_to(source.0, ToLister::Present);
        }
    }

    /// Start a Lister on request and remember that one is on its way.
    fn spawn_lister(&mut self, start_path: Option<PathBuf>) {
        self.visible_spawns_pending = self.visible_spawns_pending.saturating_add(1);
        (self.on_spawn)(start_path);
    }

    /// Leave `paths` out on the desktop: refuse the whole request (log only)
    /// when any path is not absolute or does not exist, otherwise ask the GTK
    /// side to add them. Shared by `ToHub::LeaveOut` and the `LeaveOut` step.
    fn leave_out(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if let Err(e) = opusdm_ipc::leftout::check_leavable(&paths) {
            opusdm_ipc::err_log!("hub", "{e}");
            return;
        }
        send_or_log(&self.evt_tx, HubEvent::LeftOutAdd(paths), "GTK event");
    }

    /// Make `path` (a picture or a video) the one desktop background,
    /// replacing the whole list. `fit` overrides the default fit when `Some`.
    /// The interval, fade and shuffle settings are kept.
    fn apply_background_image(&mut self, path: PathBuf, fit: Option<opusdm_ipc::ImageFit>) {
        let mut settings = self.current_settings.clone();
        settings.theme.background.set_single(path, fit);
        self.apply_settings(settings);
    }

    /// `ToHub::RunJob` from a Lister: resolve the destinations (a `None`
    /// destination fans out from the SOURCE to every DEST Lister's current
    /// directory), validate, and submit each job to the engine.
    fn lister_run_job(
        &mut self,
        cid: u64,
        op: Op,
        srcs: Vec<PathBuf>,
        dest_dir: Option<PathBuf>,
        options: opusdm_ipc::JobOptions,
    ) {
        let fail = |conns: &mut HashMap<u64, Conn>, msg: String| {
            if let Some(c) = conns.get(&cid) {
                c.queue(cid, ToLister::JobDone { job_id: 0, outcome: Outcome::Error { message: msg }, errors: Vec::new() });
            }
        };
        // A `None` destination means a button Copy/Move: the requester
        // must be the SOURCE and the job fans out to every DEST
        // Lister's current directory. A `Some` destination is explicit
        // (drag-drop, Delete, MakeDir, NewFile) and is a single target.
        let dest_dirs: Vec<PathBuf> = match dest_dir {
            Some(p) => vec![p],
            None => {
                if self.roles.source() != Some(ListerId(cid)) {
                    fail(&mut self.conns, "this Lister is not the SOURCE".into());
                    return;
                }
                let dirs: Vec<PathBuf> = self
                    .roles
                    .dests()
                    .iter()
                    .filter_map(|d| self.conns.get(&d.0).map(|c| c.path.clone()))
                    .collect();
                if dirs.is_empty() {
                    fail(&mut self.conns, "no DEST Lister set".into());
                    return;
                }
                dirs
            }
        };
        for dest_dir in dest_dirs {
            // Remote locations are the Lister's business: hand the job back
            // with the destination resolved.
            if std::iter::once(dest_dir.as_path()).chain(srcs.iter().map(PathBuf::as_path)).any(crate::jobs::is_remote_path) {
                self.send_to(cid, ToLister::RemoteTransfer { op: op.clone(), srcs: srcs.clone(), dest_dir });
                continue;
            }
            if let Err(msg) = validate_op(&op, &srcs, &dest_dir) {
                fail(&mut self.conns, msg);
                continue;
            }
            let spec = opusdm_fsops::JobSpec {
                op: op.clone(),
                srcs: srcs.clone(),
                dest_dir,
                options: options.clone(),
            };
            self.submit_job(Some(cid), spec, None);
        }
    }

    /// Hand `spec` to the job engine on behalf of `requester` (a connection
    /// id, or `None` for a hub-originated job) and start tracking it.
    /// `waiter`, when set, is told the outcome on `Done` (a Function step).
    fn submit_job(&mut self, requester: Option<u64>, spec: opusdm_fsops::JobSpec, waiter: Option<Sender<crate::functions::JobResult>>) {
        let src_parents: Vec<PathBuf> = spec.srcs.iter().filter_map(|s| s.parent().map(PathBuf::from)).collect();
        let (op, dest_dir) = (spec.op.clone(), spec.dest_dir.clone());
        let out = self.engine.submit(requester.map(ListerId), spec);
        self.jobs.insert(
            out.job_id,
            JobMeta { requester, op, totals: ProgressTotals::default(), dest_dir, src_parents, waiter },
        );
    }

    /// A command arrived from the GTK toolbar (or the SIGCHLD reaper).
    /// Returns `false` when the owner loop should shut down.
    fn gtk_cmd(&mut self, cmd: HubCmd) -> bool {
        match cmd {
            HubCmd::SpawnRequested { start_path } => self.spawn_lister(start_path),

            HubCmd::RunJob { op, srcs, dest_dir } => {
                // A hub-originated job has no requesting Lister, so progress and
                // completion just do not route back to one; the hub's own job
                // box still tracks it.
                if let Err(msg) = validate_op(&op, &srcs, &dest_dir) {
                    opusdm_ipc::err_log!("hub", "desktop job rejected: {msg}");
                    return true;
                }
                let spec = opusdm_fsops::JobSpec { op, srcs, dest_dir, options: Default::default() };
                self.submit_job(None, spec, None);
            }

            HubCmd::CancelJob(id) => self.engine.cancel(id),

            HubCmd::RunFunctionHeadless(id) => {
                self.run_headless(&id);
            }

            HubCmd::RunFunctionOnPaths { id, paths } => {
                self.run_on_paths(&id, paths);
            }

            HubCmd::SetHidden(hidden) => self.set_hidden(hidden),

            HubCmd::ToggleHidden => self.set_hidden(!self.hidden),

            HubCmd::PresentLister(id) => {
                self.send_to(id.0, ToLister::Present);
            }

            HubCmd::EmitScriptEvent(event) => self.emit_script_event(event),

            HubCmd::ShutdownAndWait(done) => {
                let keep_running = self.gtk_cmd(HubCmd::Shutdown);
                let _ = done.send(());
                return keep_running;
            }

            HubCmd::Shutdown => {
                self.run_shutdown_hooks();
                self.broadcast(ToLister::Shutdown);
                // The process exits soon after: a Set* save still queued
                // would be lost, or leave a `.tmp` behind.
                persist::flush(&self.persist_tx, persist::FLUSH_TIMEOUT);
                return false;
            }

            HubCmd::ChildExited(pid) => {
                let hit = self.conns.iter().find(|(_, c)| c.pid == pid).map(|(k, _)| *k);
                if let Some(cid) = hit {
                    self.drop_lister(cid);
                }
            }
        }
        true
    }

    /// A job engine progress event: conflict prompts, completion routing
    /// and lister invalidation, or a plain progress update.
    fn job_progress(&mut self, job_id: u64, prog: Progress) {
        let Some(meta) = self.jobs.get_mut(&job_id) else { return };
        let requester = meta.requester;

        match prog {
            Progress::Conflict { path, src, dst, reply } => {
                // Nobody to ask (hub-originated job, or the Lister is gone): skip the entry.
                let Some((cid, c)) = requester.and_then(|r| self.conns.get_mut(&r).map(|c| (r, c))) else {
                    opusdm_ipc::err_log!("hub", "job {job_id}: {} already exists, skipped (nobody to ask)", path.display());
                    let _ = reply.send(Resolution::Skip);
                    return;
                };
                let req_id = self.next_req;
                self.next_req += 1;
                self.replies.insert(req_id, reply);
                self.conflict_owners.insert(req_id, cid);
                c.queue(cid, ToLister::ConflictPrompt { req_id, job_id, path, src, dst });
            }
            Progress::Done { copied, skipped, cancelled, errors } => {
                let outcome = if cancelled {
                    Outcome::Cancelled { copied, skipped }
                } else {
                    Outcome::Ok { copied, skipped }
                };
                let err_count = errors.len();
                if let Some(waiter) = meta.waiter.take() {
                    let result = if let Some((path, msg)) = errors.first() {
                        Err(format!("{} ({msg})", path.display()))
                    } else if cancelled {
                        Err("cancelled".to_string())
                    } else {
                        Ok((copied, skipped))
                    };
                    let _ = waiter.send(result);
                }
                // `meta` still borrows `self.jobs`: send through the field.
                if let Some((r, c)) = requester.and_then(|r| self.conns.get(&r).map(|c| (r, c))) {
                    c.queue(r, ToLister::JobDone { job_id, outcome, errors });
                }
                send_or_log(&self.evt_tx, HubEvent::JobFinished { job_id, ok: !cancelled && err_count == 0, errors: err_count, cancelled }, "GTK event");
                // Refresh every Lister showing an affected directory.
                // Sources are refreshed too: Move/Delete remove them, Duplicate
                // adds to them, and Copy may have run as a Move.
                let mut affected = vec![meta.dest_dir.clone()];
                affected.extend(meta.src_parents.iter().cloned());
                for dir in affected {
                    self.broadcast(ToLister::InvalidatePath(dir));
                }
                self.engine.on_done(job_id);
                self.jobs.remove(&job_id);
            }
            other => {
                let op = meta.op.clone();
                if let Some(msg) = map_progress(job_id, &op, &other, &mut meta.totals) {
                    if let ToLister::JobProgress {
                        current_file,
                        files_done,
                        files_total,
                        file_bytes_done,
                        file_bytes_total,
                        ..
                    } = &msg
                    {
                        let update = HubEvent::JobUpdate {
                            job_id,
                            current_file: current_file.clone(),
                            files_done: *files_done,
                            files_total: *files_total,
                            file_bytes_done: *file_bytes_done,
                            file_bytes_total: *file_bytes_total,
                        };
                        send_or_log(&self.evt_tx, update, "GTK event");
                    }
                    if let Some(r) = requester {
                        self.send_to(r, msg);
                    }
                }
            }
        }
    }
}

fn owner_loop(
    rx: Receiver<OwnerMsg>,
    owner_tx: Sender<OwnerMsg>,
    evt_tx: Sender<HubEvent>,
    on_spawn: SpawnFn,
    opts: ServerOptions,
) {

    let persist_tx = persist::spawn_persist_thread();

    let (jp_tx, jp_rx) = channel::<(u64, Progress)>();
    spawn_forwarder("opusdm-job-progress", jp_rx, owner_tx.clone(), |(id, ev)| OwnerMsg::JobProgress(id, ev));
    let (fe_tx, fe_rx) = channel::<(u64, crate::functions::FnEvent)>();
    spawn_forwarder("opusdm-function-events", fe_rx, owner_tx.clone(), |(id, ev)| OwnerMsg::FunctionEvent(id, ev));

    let current_desktop = desktop_settings::load();
    opusdm_ipc::debug_log!(
        "hub",
        "desktop settings loaded (icon size {})",
        current_desktop.icon_size
    );
    let mut hub = Hub {
        conns: HashMap::new(),
        next_conn: 1,
        roles: RoleState::default(),
        current_settings: settings::load_settings(),
        current_desktop,
        hidden: false,
        visible_spawns_pending: 0,
        event_guard: EventGuard::new(),
        global_keys_enabled: opts.global_keys,
        owner_tx: owner_tx.clone(),
        global_keys: None,
        wayland_keys: None,
        x11_failed: false,
        current_bindings: match opts.initial_bindings {
            Some(b) => b,
            None => bindings::sanitize(bindings::load_bindings()),
        },
        current_formats: match opts.initial_formats {
            Some(f) => f,
            None => format::load_formats(),
        },
        current_functions: match opts.initial_functions {
            Some(f) => f,
            None => opusdm_ipc::function::load_functions(),
        },
        persist_tx,
        engine: JobEngine::new(default_runner(), jp_tx),
        jobs: HashMap::new(),
        replies: HashMap::new(),
        conflict_owners: HashMap::new(),
        confirms: HashMap::new(),
        output_windows: HashMap::new(),
        next_req: 1,
        evt_tx,
        on_spawn,
        runs: FunctionRuns::new(fe_tx),
        fit_tx: spawn_fit_worker(owner_tx.clone()),
    };

    hub.apply_global_keys();

    for msg in rx {
        let keep_running = match msg {
            OwnerMsg::Accepted(stream) => {
                hub.accepted(stream, &owner_tx);
                true
            }
            OwnerMsg::FromLister(cid, msg) => {
                hub.lister_msg(cid, msg);
                true
            }
            OwnerMsg::ListerEof(cid) => {
                hub.drop_lister(cid);
                true
            }
            OwnerMsg::JobProgress(job_id, prog) => {
                hub.job_progress(job_id, prog);
                true
            }
            OwnerMsg::FromGtk(cmd) => hub.gtk_cmd(cmd),
            OwnerMsg::FunctionEvent(run_id, ev) => {
                hub.function_event(run_id, ev);
                true
            }
            OwnerMsg::FitChecked(cid, answer) => {
                hub.send_to(cid, answer);
                true
            }
        };
        if !keep_running {
            return;
        }
    }
}

/// One `ToHub::CheckFit` waiting for the fit-check worker.
struct FitRequest {
    cid: u64,
    srcs: Vec<PathBuf>,
    dest_dir: Option<PathBuf>,
}

/// Start the thread that answers `ToHub::CheckFit` requests one after the
/// other, each answer going back to the owner loop as
/// `OwnerMsg::FitChecked`. One worker instead of a thread per request.
fn spawn_fit_worker(owner_tx: Sender<OwnerMsg>) -> Sender<FitRequest> {
    let (tx, rx) = channel::<FitRequest>();
    std::thread::Builder::new()
        .name("opusdm-check-fit".into())
        .spawn(move || {
            for req in rx {
                let answer = fit_answer(&req.srcs, req.dest_dir);
                if owner_tx.send(OwnerMsg::FitChecked(req.cid, answer)).is_err() {
                    return;
                }
            }
        })
        .expect("spawn fit-check thread");
    tx
}

/// Size `srcs`, read the free space of `dest_dir` and say whether they fit.
fn fit_answer(srcs: &[PathBuf], dest_dir: Option<PathBuf>) -> ToLister {
    let Some(dir) = dest_dir else { return ToLister::CheckFitFailed { message: "no DEST Lister set".into() } };
    match opusdm_fsops::checkfit::check_fit(srcs, &dir) {
        Ok(r) => ToLister::CheckFitResult { dest_dir: dir, needed: r.needed, free: r.free, fits: r.fits, percent: r.percent },
        Err(e) => ToLister::CheckFitFailed { message: format!("{}: {e}", dir.display()) },
    }
}

/// Start thread `name`, which wraps every message of `rx` with `wrap` and
/// passes it to the owner loop, until either side closes.
fn spawn_forwarder<T: Send + 'static>(name: &str, rx: Receiver<T>, owner_tx: Sender<OwnerMsg>, wrap: fn(T) -> OwnerMsg) {
    let thread_name = name.to_string();
    std::thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            for msg in rx {
                if owner_tx.send(wrap(msg)).is_err() {
                    break;
                }
            }
            opusdm_ipc::debug_log!("hub", "{thread_name} forwarder exiting");
        })
        .unwrap_or_else(|e| panic!("spawn {name} forwarder thread: {e}"));
}

/// Whether a `recv` error means the peer sent something this hub cannot
/// decode (an unknown message, a bad or oversized frame), as opposed to the
/// connection itself failing.
fn is_decode_error(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::Other | std::io::ErrorKind::InvalidData)
}

/// Read connection `cid` until it ends, passing every message to the owner
/// loop. A message the hub cannot decode ends the connection with a log line,
/// so a client built from another OpusDM version is not dropped silently.
fn reader_loop(cid: u64, mut stream: UnixStream, owner_tx: Sender<OwnerMsg>) {
    loop {
        match recv::<_, ToHub>(&mut stream) {
            Ok(Some(msg)) => {
                if owner_tx.send(OwnerMsg::FromLister(cid, msg)).is_err() {
                    return;
                }
                continue;
            }
            Ok(None) => {}
            Err(e) if is_decode_error(&e) => {
                opusdm_ipc::err_log!("hub", "connection {cid} sent a message this hub cannot read (another OpusDM version?), disconnecting: {e}");
            }
            Err(e) => opusdm_ipc::debug_log!("hub", "connection {cid} failed: {e}"),
        }
        let _ = owner_tx.send(OwnerMsg::ListerEof(cid));
        return;
    }
}

fn drop_conn(
    cid: u64,
    conns: &mut HashMap<u64, Conn>,
    roles: &mut RoleState,
    evt_tx: &Sender<HubEvent>,
) -> bool {
    let Some(c) = conns.remove(&cid) else { return false };
    let _ = c.shutdown.shutdown(std::net::Shutdown::Both);
    if !c.registered {
        return false;
    }
    apply_role(roles, RoleEvent::Dropped(ListerId(cid)), conns, evt_tx);
    send_or_log(evt_tx, HubEvent::ListerGone(ListerId(cid)), "GTK event");
    if !conns.values().any(|c| c.registered) {
        send_or_log(evt_tx, HubEvent::AllListersGone, "GTK event");
    }
    true
}

fn apply_role(
    roles: &mut RoleState,
    ev: RoleEvent,
    conns: &mut HashMap<u64, Conn>,
    evt_tx: &Sender<HubEvent>,
) {
    let (next, effects) = apply_role_event(roles.clone(), ev);
    *roles = next;
    for effect in effects {
        match effect {
            RoleEffect::Broadcast => {
                broadcast(conns, &roles_msg(roles));
                emit_roles(roles, conns, evt_tx);
            }
            RoleEffect::Reject(id) => {
                if let Some(c) = conns.get(&id.0) {
                    c.queue(id.0, roles_msg(roles));
                }
            }
        }
    }
}

/// The `RoleChanged` message describing `roles`.
fn roles_msg(roles: &RoleState) -> ToLister {
    ToLister::RoleChanged { source: roles.source(), dests: roles.dests(), source_locked: roles.source_locked() }
}

/// Send `msg` to every registered Lister.
/// The message is encoded to JSON once and the bytes are shared by every
/// writer, instead of cloning and encoding it again for each Lister.
fn broadcast(conns: &HashMap<u64, Conn>, msg: &ToLister) {
    if !conns.values().any(|c| c.registered) {
        return;
    }
    let bytes: Arc<[u8]> = match serde_json::to_vec(msg) {
        Ok(bytes) => bytes.into(),
        Err(e) => {
            opusdm_ipc::err_log!("hub", "cannot encode a broadcast: {e}");
            return;
        }
    };
    for (cid, c) in conns.iter().filter(|(_, c)| c.registered) {
        c.queue(*cid, OutMsg::Encoded(bytes.clone()));
    }
}

fn emit_roles(roles: &RoleState, conns: &HashMap<u64, Conn>, evt_tx: &Sender<HubEvent>) {
    let paths = conns
        .iter()
        .filter(|(_, c)| c.registered)
        .map(|(k, c)| (ListerId(*k), c.path.clone()))
        .collect();
    send_or_log(evt_tx, HubEvent::Roles { source: roles.source(), dests: roles.dests(), paths }, "GTK event");
}

#[cfg(test)]
mod tests;
