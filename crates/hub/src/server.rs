//! The socket server: an accept loop, one reader thread per Lister, and a
//! single owner loop that holds all mutable hub state. Only channels cross
//! thread boundaries.

use crate::jobs::{default_runner, map_progress, validate_op, JobEngine, ProgressTotals};
use crate::script_events::{EventGuard, ScriptEvent, SHUTDOWN_HOOK_TIMEOUT};
use crate::roles::{apply_role_event, RoleEffect, RoleEvent, RoleState};
use crate::settings;
use crate::desktop_settings;
use opusdm_fsops::Progress;
use opusdm_ipc::bindings::{self, Bindings};
use opusdm_ipc::format::{self, FormatStore};
use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::function::{FnStep, FunctionId, SelectionCtx};
use opusdm_ipc::{DesktopSettings, ListerId, Op, Outcome, Resolution, ToHub, ToLister, Welcome};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

mod function_run;
mod persist;

use function_run::PendingFunctionRun;
use persist::PersistMsg;

/// Called when a Lister or the toolbar asks for a new Lister window.
pub type SpawnFn = Box<dyn Fn(Option<PathBuf>) + Send>;

/// Requester id of a Function run the hub started itself (no Lister asked
/// for it). Same sentinel value as the desktop-originated job requester.
const HUB_REQUESTER: u64 = u64::MAX;

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
    /// Desktop settings changed (from `opus-desktop-config`). Carries the
    /// sanitised value; the GTK side repaints and re-lays the desktop.
    DesktopSettingsChanged(DesktopSettings),
    /// Theme changed (from `opusdm-settings`). Carries the new theme; the GTK
    /// side repaints the desktop background and re-renders desktop icons.
    ThemeChanged(opusdm_ipc::Theme),
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
}

impl Default for ServerOptions {
    /// Options for tests: no disk read.
    fn default() -> Self {
        Self {
            global_keys: false,
            initial_bindings: Some(Bindings::default()),
            initial_formats: Some(FormatStore::default()),
        }
    }
}

impl ServerOptions {
    /// Options for the real hub: load the bindings from disk.
    pub fn production() -> Self {
        Self { global_keys: true, initial_bindings: None, initial_formats: None }
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
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(s) => {
                            if owner_tx.send(OwnerMsg::Accepted(s)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
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

/// Everything the owner loop merges into one stream.
enum OwnerMsg {
    Accepted(UnixStream),
    FromLister(u64, ToHub),
    ListerEof(u64),
    JobProgress(u64, Progress),
    FromGtk(HubCmd),
    FunctionEvent(u64, crate::functions::FnEvent),
}

/// One connected peer. `registered` flips true after `Register`.
///
/// Writes go through `tx` to a dedicated writer thread rather than a
/// blocking `send()` on the owner thread, so a stalled or non-reading
/// client cannot stall connection lifecycle, role changes, or job
/// progress for every other Lister. `shutdown` is a separate socket
/// clone kept only to tear the connection down from `drop_conn`.
struct Conn {
    tx: Sender<OutMsg>,
    shutdown: UnixStream,
    registered: bool,
    pid: u32,
    path: PathBuf,
    /// The directories this Lister reported as cached.
    cache_paths: Vec<PathBuf>,
}

/// Either of the two message types the hub writes to a Lister socket.
/// `Welcome` is sent exactly once, right after `Register`; everything
/// else after that is a `ToLister`.
enum OutMsg {
    Welcome(Welcome),
    ToLister(ToLister),
}

impl From<ToLister> for OutMsg {
    fn from(msg: ToLister) -> Self {
        OutMsg::ToLister(msg)
    }
}

impl From<Welcome> for OutMsg {
    fn from(msg: Welcome) -> Self {
        OutMsg::Welcome(msg)
    }
}

/// Owns one Lister's write half; runs on its own thread so a blocking
/// `send()` (a non-reading peer, a full socket buffer) never stalls the
/// owner loop.
fn writer_loop(mut stream: UnixStream, rx: Receiver<OutMsg>) {
    for msg in rx {
        let result = match msg {
            OutMsg::Welcome(w) => send(&mut stream, &w),
            OutMsg::ToLister(m) => send(&mut stream, &m),
        };
        if result.is_err() {
            break;
        }
    }
}

/// Per-job bookkeeping the owner keeps until `Done`.
struct JobMeta {
    requester: u64,
    op: Op,
    totals: ProgressTotals,
    dest_dir: PathBuf,
    src_parents: Vec<PathBuf>,
}

/// All mutable hub state, owned by the owner loop. Splitting the loop's big
/// match into methods on this struct keeps each concern - connection
/// lifecycle, authentication/registration, role transitions, settings
/// persistence, job submission, conflict replies, GTK commands - in its own
/// handler, where its invariants are easier to follow and test.
struct Hub {
    conns: HashMap<u64, Conn>,
    free_conns: BTreeSet<u64>,
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
    /// Loop protection for script events.
    event_guard: EventGuard,
    /// Whether global hotkeys may be grabbed (off in tests).
    global_keys_enabled: bool,
    /// Sender back into the owner loop, used by the global-key thread.
    owner_tx: Sender<OwnerMsg>,
    /// The X11 grabs, created lazily on the first non-empty global set.
    global_keys: Option<crate::global_keys::GlobalKeys>,
    /// The X11 connection failed once; do not retry or log again.
    x11_failed: bool,
    persist_tx: Sender<PersistMsg>,
    engine: JobEngine,
    jobs: HashMap<u64, JobMeta>,
    replies: HashMap<u64, Sender<Resolution>>,
    /// Connection that must answer each Function conflict prompt in `replies`
    /// (by request id), so a closed Lister releases the job waiting on it.
    fn_conflict_owners: HashMap<u64, u64>,
    /// Pending `Confirm` questions: request id to (answer channel, asked Lister).
    confirms: HashMap<u64, (Sender<bool>, u64)>,
    /// Which Lister shows each `Run` output window, by `(run id, step)`. It
    /// outlives the function run that opened it.
    output_windows: HashMap<(u64, usize), u64>,
    next_req: u64,
    evt_tx: Sender<HubEvent>,
    on_spawn: SpawnFn,
    /// Function runs waiting on `SelectionReply` from SOURCE and/or DEST.
    pending_functions: HashMap<u64, PendingFunctionRun>,
    /// Which pending runs a Lister connection's next `SelectionReply`s are
    /// for, oldest first: a Lister answers `QuerySelection`s in order, so two
    /// runs waiting on it at once are matched by a FIFO, not a single slot.
    selection_waiters: HashMap<u64, VecDeque<u64>>,
    /// Which connection to route a running Function's events back to, kept
    /// alive from the moment execution starts until `FunctionDone`/`Failed`.
    function_requesters: HashMap<u64, u64>,
    next_run_id: u64,
    function_events_tx: Sender<(u64, crate::functions::FnEvent)>,
}

impl Hub {
    /// A new peer connected: allocate a connection id, spawn its writer and
    /// reader threads. A spawn failure drops just this connection.
    fn accepted(&mut self, stream: UnixStream, owner_tx: &Sender<OwnerMsg>) {
        let cid = if let Some(&first) = self.free_conns.iter().next() {
            self.free_conns.remove(&first);
            first
        } else {
            let id = self.next_conn;
            self.next_conn += 1;
            id
        };
        let (Ok(write_stream), Ok(shutdown)) = (stream.try_clone(), stream.try_clone()) else {
            self.free_conns.insert(cid);
            return;
        };
        let (tx, writer_rx) = channel::<OutMsg>();
        let writer_spawned = std::thread::Builder::new()
            .name(format!("opusdm-writer-{cid}"))
            .spawn(move || writer_loop(write_stream, writer_rx));
        if let Err(e) = writer_spawned {
            opusdm_ipc::err_log!("hub", "cannot spawn writer thread for conn {cid}: {e}");
            self.free_conns.insert(cid);
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
            self.free_conns.insert(cid);
        }
    }

    /// A message arrived from Lister connection `cid`.
    fn lister_msg(&mut self, cid: u64, msg: ToHub) {
        match msg {
            ToHub::Ping => {
                if let Some(c) = self.conns.get_mut(&cid) {
                    let _ = c.tx.send(ToLister::Pong.into());
                }
            }

            ToHub::RaiseToolbar => {
                let _ = self.evt_tx.send(HubEvent::RaiseToolbar);
            }

            ToHub::SetDesktopSettings(ds) => {
                self.current_desktop = ds.sanitized();
                let _ = self.persist_tx.send(PersistMsg::Desktop(self.current_desktop.clone()));
                let _ = self
                    .evt_tx
                    .send(HubEvent::DesktopSettingsChanged(self.current_desktop.clone()));
            }

            ToHub::SetSettings(new_settings) => self.apply_settings(new_settings),

            ToHub::SetBindings(new_bindings) => {
                self.current_bindings = bindings::sanitize(new_bindings);
                let _ = self.persist_tx.send(PersistMsg::Bindings(self.current_bindings.clone()));
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::BindingsUpdate(self.current_bindings.clone()).into());
                }
                self.apply_global_keys();
            }

            ToHub::SetFormats(new_formats) => {
                self.current_formats = new_formats.sanitized();
                let _ = self.persist_tx.send(PersistMsg::Formats(self.current_formats.clone()));
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::FormatsUpdate(self.current_formats.clone()).into());
                }
            }

            ToHub::CacheReport(paths) => {
                if let Some(c) = self.conns.get_mut(&cid) {
                    c.cache_paths = paths;
                }
            }

            ToHub::QueryCaches => {
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
                if let Some(c) = self.conns.get(&cid) {
                    let _ = c.tx.send(ToLister::CacheList(entries).into());
                }
            }

            ToHub::OpenBankManager => {
                let _ = self.evt_tx.send(HubEvent::OpenBankManager);
            }

            ToHub::FreeAllCaches => {
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::FreeCaches.into());
                }
            }

            ToHub::Invalidate(path) => {
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::InvalidatePath(path.clone()).into());
                }
            }

            ToHub::JumpTo { lister, path } => {
                if let Some(c) = self.conns.get(&lister.0).filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::GoTo { path }.into());
                }
            }

            ToHub::Register { pid, initial_path } => {
                if let Some(c) = self.conns.get_mut(&cid) {
                    c.registered = true;
                    c.pid = pid;
                    c.path = initial_path.clone();
                    let _ = c.tx.send(
                        Welcome {
                            lister_id: ListerId(cid),
                            source: self.roles.source(),
                            dests: self.roles.dests(),
                            source_locked: self.roles.source_locked(),
                            settings: self.current_settings.clone(),
                            bindings: self.current_bindings.clone(),
                            formats: self.current_formats.clone(),
                        }
                        .into(),
                    );
                    let _ = self.evt_tx.send(HubEvent::ListerRegistered {
                        id: ListerId(cid),
                        pid,
                        path: initial_path,
                    });
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
                        } else if let Some(c) = self.conns.get_mut(&cid) {
                            let _ = c.tx.send(ToLister::SetHidden(true).into());
                        }
                    }
                    self.emit_script_event(ScriptEvent::ListerOpened);
                }
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

            ToHub::RunFunction { steps } => {
                self.start_function_run(cid, Some(cid), steps);
            }

            ToHub::RunFunctionById { id } => {
                self.run_function_by_id(cid, Some(cid), &id);
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
                if let Some(c) = self.conns.get_mut(&target.0).filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::Present.into());
                }
            }

            ToHub::ConfirmReply { req_id, ok } => {
                if let Some((tx, _)) = self.confirms.remove(&req_id) {
                    let _ = tx.send(ok);
                }
            }

            ToHub::ConflictReply { req_id, resolution } => {
                self.fn_conflict_owners.remove(&req_id);
                if let Some(tx) = self.replies.remove(&req_id) {
                    let _ = tx.send(resolution);
                }
            }

            ToHub::Goodbye => {
                self.drop_lister(cid);
            }
        }
    }

    /// Drop Lister connection `cid`: forget its Function-run state, tear the
    /// connection down and, when a registered Lister went away, emit
    /// `ListerClosed`.
    fn drop_lister(&mut self, cid: u64) {
        self.drop_function_state(cid);
        let was_registered = drop_conn(cid, &mut self.conns, &mut self.free_conns, &mut self.roles, &self.evt_tx);
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
        let saved = opusdm_ipc::function::load_functions();
        let deadline = Instant::now() + SHUTDOWN_HOOK_TIMEOUT;
        let mut receivers = Vec::new();
        for id in ids {
            let Some(function) = saved.iter().find(|f| f.id == id) else {
                opusdm_ipc::err_log!("hub", "shutdown hook: function '{id}' not found");
                continue;
            };
            if !opusdm_ipc::function::validate(function).is_empty() {
                opusdm_ipc::err_log!("hub", "shutdown hook: function '{}' is invalid", function.name);
                continue;
            }
            let run_id = self.next_run_id;
            self.next_run_id += 1;
            let (tx, rx) = channel();
            let empty = SelectionCtx { paths: Vec::new(), cwd: home_dir() };
            crate::functions::spawn_executor(run_id, function.steps.clone(), empty, None, tx);
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
                    Ok((_, crate::functions::FnEvent::Conflict { reply, .. })) => {
                        let _ = reply.send(Resolution::Skip);
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

    /// `ToHub::CheckFit` from `cid`: size the selection and read the free
    /// space on its own thread (a big tree can take a while), then answer
    /// the requester.
    fn check_fit(&mut self, cid: u64, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf>) {
        let Some(reply) = self.conns.get(&cid).map(|c| c.tx.clone()) else { return };
        let dest_dir = dest_dir.or_else(|| {
            self.roles
                .dests()
                .iter()
                .find_map(|d| self.conns.get(&d.0).map(|c| c.path.clone()))
        });
        std::thread::spawn(move || {
            let msg = match dest_dir {
                None => ToLister::CheckFitFailed { message: "no DEST Lister set".into() },
                Some(dir) => match opusdm_fsops::checkfit::check_fit(&srcs, &dir) {
                    Ok(r) => ToLister::CheckFitResult {
                        dest_dir: dir,
                        needed: r.needed,
                        free: r.free,
                        fits: r.fits,
                        percent: r.percent,
                    },
                    Err(e) => ToLister::CheckFitFailed { message: format!("{}: {e}", dir.display()) },
                },
            };
            let _ = reply.send(msg.into());
        });
    }

    /// Adopt `new_settings`: sanitize the background, persist to
    /// `settings.json`, push the change to every registered Lister and to
    /// the GTK side. Shared by `ToHub::SetSettings` and the `SetBackground`
    /// function step.
    fn apply_settings(&mut self, mut new_settings: opusdm_ipc::Settings) {
        new_settings.theme.background = new_settings.theme.background.sanitized();
        self.current_settings = new_settings;
        let _ = self.persist_tx.send(PersistMsg::Settings(self.current_settings.clone()));
        for c in self.conns.values_mut().filter(|c| c.registered) {
            let _ = c.tx.send(ToLister::SettingsUpdate(self.current_settings.clone()).into());
        }
        let _ = self.evt_tx.send(HubEvent::ThemeChanged(self.current_settings.theme.clone()));
        let _ = self.evt_tx.send(HubEvent::AppearanceChanged(self.current_settings.appearance.clone()));
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
        for c in self.conns.values_mut().filter(|c| c.registered) {
            let _ = c.tx.send(ToLister::SetHidden(hidden).into());
        }
        let _ = self.evt_tx.send(HubEvent::HiddenChanged(hidden));
        if hidden || !present_source {
            return;
        }
        if let Some(source) = self.roles.source() {
            if let Some(c) = self.conns.get_mut(&source.0) {
                let _ = c.tx.send(ToLister::Present.into());
            }
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
        let _ = self.evt_tx.send(HubEvent::LeftOutAdd(paths));
    }

    /// Install `path` as the desktop background image. `fit` overrides the
    /// current fit when `Some`; `None` keeps it.
    fn apply_background_image(&mut self, path: PathBuf, fit: Option<opusdm_ipc::ImageFit>) {
        let mut settings = self.current_settings.clone();
        let background = &mut settings.theme.background;
        background.mode = opusdm_ipc::BackgroundMode::Image;
        background.image_path = Some(path);
        if let Some(fit) = fit {
            background.image_fit = fit;
        }
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
            if let Some(c) = conns.get_mut(&cid) {
                let _ = c.tx.send(
                    ToLister::JobDone {
                        job_id: 0,
                        outcome: Outcome::Error { message: msg },
                        errors: Vec::new(),
                    }
                    .into(),
                );
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
        let src_parents: Vec<PathBuf> = srcs
            .iter()
            .filter_map(|s| s.parent().map(PathBuf::from))
            .collect();
        for dest_dir in dest_dirs {
            // Remote locations are the Lister's business: hand the job back
            // with the destination resolved.
            if std::iter::once(dest_dir.as_path()).chain(srcs.iter().map(PathBuf::as_path)).any(crate::jobs::is_remote_path) {
                if let Some(c) = self.conns.get_mut(&cid) {
                    let _ = c.tx.send(ToLister::RemoteTransfer { op: op.clone(), srcs: srcs.clone(), dest_dir }.into());
                }
                continue;
            }
            if let Err(msg) = validate_op(&op, &srcs, &dest_dir) {
                fail(&mut self.conns, msg);
                continue;
            }
            let spec = opusdm_fsops::JobSpec {
                op: op.clone(),
                srcs: srcs.clone(),
                dest_dir: dest_dir.clone(),
                options: options.clone(),
            };
            let out = self.engine.submit(ListerId(cid), spec);
            self.jobs.insert(
                out.job_id,
                JobMeta {
                    requester: cid,
                    op: op.clone(),
                    totals: ProgressTotals::default(),
                    dest_dir,
                    src_parents: src_parents.clone(),
                },
            );
        }
    }

    /// A command arrived from the GTK toolbar (or the SIGCHLD reaper).
    /// Returns `false` when the owner loop should shut down.
    fn gtk_cmd(&mut self, cmd: HubCmd) -> bool {
        match cmd {
            HubCmd::SpawnRequested { start_path } => self.spawn_lister(start_path),

            HubCmd::RunJob { op, srcs, dest_dir } => {
                // A hub-originated job has no requesting Lister, so progress and
                // completion just do not route back to one; the hub's own job
                // box still tracks it. `u64::MAX` is the sentinel requester.
                if let Err(msg) = validate_op(&op, &srcs, &dest_dir) {
                    opusdm_ipc::err_log!("hub", "desktop job rejected: {msg}");
                    return true;
                }
                let src_parents: Vec<PathBuf> =
                    srcs.iter().filter_map(|s| s.parent().map(PathBuf::from)).collect();
                let spec = opusdm_fsops::JobSpec {
                    op: op.clone(),
                    srcs: srcs.clone(),
                    dest_dir: dest_dir.clone(),
                    options: Default::default(),
                };
                let out = self.engine.submit(ListerId(HUB_REQUESTER), spec);
                self.jobs.insert(
                    out.job_id,
                    JobMeta {
                        requester: HUB_REQUESTER,
                        op,
                        totals: ProgressTotals::default(),
                        dest_dir,
                        src_parents,
                    },
                );
            }

            HubCmd::CancelJob(id) => self.engine.cancel(id),

            HubCmd::RunFunctionHeadless(id) => {
                self.run_headless(&id);
            }

            HubCmd::SetHidden(hidden) => self.set_hidden(hidden),

            HubCmd::ToggleHidden => self.set_hidden(!self.hidden),

            HubCmd::PresentLister(id) => {
                if let Some(c) = self.conns.get_mut(&id.0) {
                    let _ = c.tx.send(ToLister::Present.into());
                }
            }

            HubCmd::EmitScriptEvent(event) => self.emit_script_event(event),

            HubCmd::ShutdownAndWait(done) => {
                let keep_running = self.gtk_cmd(HubCmd::Shutdown);
                let _ = done.send(());
                return keep_running;
            }

            HubCmd::Shutdown => {
                self.run_shutdown_hooks();
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::Shutdown.into());
                }
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
                let req_id = self.next_req;
                self.next_req += 1;
                self.replies.insert(req_id, reply);
                if let Some(c) = self.conns.get_mut(&requester) {
                    let _ = c.tx.send(
                        ToLister::ConflictPrompt { req_id, job_id, path, src, dst }.into(),
                    );
                }
            }
            Progress::Done { copied, skipped, cancelled, errors } => {
                let outcome = if cancelled {
                    Outcome::Cancelled { copied, skipped }
                } else {
                    Outcome::Ok { copied, skipped }
                };
                let err_count = errors.len();
                if let Some(c) = self.conns.get_mut(&requester) {
                    let _ = c.tx.send(ToLister::JobDone { job_id, outcome, errors }.into());
                }
                let _ = self.evt_tx.send(HubEvent::JobFinished {
                    job_id,
                    ok: !cancelled && err_count == 0,
                    errors: err_count,
                    cancelled,
                });
                // Refresh every Lister showing an affected directory.
                // Sources are refreshed too: Move/Delete remove them, Duplicate
                // adds to them, and Copy may have run as a Move.
                let mut affected = vec![meta.dest_dir.clone()];
                affected.extend(meta.src_parents.iter().cloned());
                for dir in affected {
                    for c in self.conns.values_mut().filter(|c| c.registered) {
                        let _ = c.tx.send(ToLister::InvalidatePath(dir.clone()).into());
                    }
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
                        let _ = self.evt_tx.send(HubEvent::JobUpdate {
                            job_id,
                            current_file: current_file.clone(),
                            files_done: *files_done,
                            files_total: *files_total,
                            file_bytes_done: *file_bytes_done,
                            file_bytes_total: *file_bytes_total,
                        });
                    }
                    if let Some(c) = self.conns.get_mut(&requester) {
                        let _ = c.tx.send(msg.into());
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
    {
        let owner_tx = owner_tx.clone();
        std::thread::Builder::new()
            .name("opusdm-job-progress".into())
            .spawn(move || {
                for (id, ev) in jp_rx {
                    if owner_tx.send(OwnerMsg::JobProgress(id, ev)).is_err() {
                        break;
                    }
                }
                opusdm_ipc::debug_log!("hub", "job progress forwarder exiting");
            })
            .expect("spawn job progress forwarder thread");
    }

    let (fe_tx, fe_rx) = channel::<(u64, crate::functions::FnEvent)>();
    {
        let owner_tx = owner_tx.clone();
        std::thread::Builder::new()
            .name("opusdm-function-events".into())
            .spawn(move || {
                for (id, ev) in fe_rx {
                    if owner_tx.send(OwnerMsg::FunctionEvent(id, ev)).is_err() {
                        break;
                    }
                }
                opusdm_ipc::debug_log!("hub", "function event forwarder exiting");
            })
            .expect("spawn function event forwarder thread");
    }

    let current_desktop = desktop_settings::load();
    opusdm_ipc::debug_log!(
        "hub",
        "desktop settings loaded (icon size {})",
        current_desktop.icon_size
    );
    let mut hub = Hub {
        conns: HashMap::new(),
        free_conns: BTreeSet::new(),
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
        x11_failed: false,
        current_bindings: match opts.initial_bindings {
            Some(b) => b,
            None => bindings::sanitize(bindings::load_bindings()),
        },
        current_formats: match opts.initial_formats {
            Some(f) => f,
            None => format::load_formats(),
        },
        persist_tx,
        engine: JobEngine::new(default_runner(), jp_tx),
        jobs: HashMap::new(),
        replies: HashMap::new(),
        fn_conflict_owners: HashMap::new(),
        confirms: HashMap::new(),
        output_windows: HashMap::new(),
        next_req: 1,
        evt_tx,
        on_spawn,
        pending_functions: HashMap::new(),
        selection_waiters: HashMap::new(),
        function_requesters: HashMap::new(),
        next_run_id: 1,
        function_events_tx: fe_tx,
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
        };
        if !keep_running {
            return;
        }
    }
}

fn reader_loop(cid: u64, mut stream: UnixStream, owner_tx: Sender<OwnerMsg>) {
    loop {
        match recv::<_, ToHub>(&mut stream) {
            Ok(Some(msg)) => {
                if owner_tx.send(OwnerMsg::FromLister(cid, msg)).is_err() {
                    return;
                }
            }
            Ok(None) | Err(_) => {
                let _ = owner_tx.send(OwnerMsg::ListerEof(cid));
                return;
            }
        }
    }
}

fn drop_conn(
    cid: u64,
    conns: &mut HashMap<u64, Conn>,
    free_conns: &mut BTreeSet<u64>,
    roles: &mut RoleState,
    evt_tx: &Sender<HubEvent>,
) -> bool {
    let Some(c) = conns.remove(&cid) else { return false };
    free_conns.insert(cid);
    let _ = c.shutdown.shutdown(std::net::Shutdown::Both);
    if !c.registered {
        return false;
    }
    apply_role(roles, RoleEvent::Dropped(ListerId(cid)), conns, evt_tx);
    let _ = evt_tx.send(HubEvent::ListerGone(ListerId(cid)));
    if !conns.values().any(|c| c.registered) {
        let _ = evt_tx.send(HubEvent::AllListersGone);
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
                for c in conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(
                        ToLister::RoleChanged {
                            source: roles.source(),
                            dests: roles.dests(),
                            source_locked: roles.source_locked(),
                        }
                        .into(),
                    );
                }
                emit_roles(roles, conns, evt_tx);
            }
            RoleEffect::Reject(id) => {
                if let Some(c) = conns.get_mut(&id.0) {
                    let _ = c.tx.send(
                        ToLister::RoleChanged {
                            source: roles.source(),
                            dests: roles.dests(),
                            source_locked: roles.source_locked(),
                        }
                        .into(),
                    );
                }
            }
        }
    }
}

fn emit_roles(roles: &RoleState, conns: &HashMap<u64, Conn>, evt_tx: &Sender<HubEvent>) {
    let paths = conns
        .iter()
        .filter(|(_, c)| c.registered)
        .map(|(k, c)| (ListerId(*k), c.path.clone()))
        .collect();
    let _ = evt_tx.send(HubEvent::Roles { source: roles.source(), dests: roles.dests(), paths });
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::frame::{recv, send};
    use std::sync::Mutex;
    use std::time::Duration;

    /// Guards tests that mutate the process-wide `XDG_CONFIG_HOME` env var,
    /// which libtest would otherwise race across threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn bind_temp() -> (tempfile::TempDir, UnixListener, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let l = UnixListener::bind(&path).unwrap();
        (td, l, path)
    }

    /// Settings/desktop-settings persistence now happens on its own thread
    /// (see `PersistMsg`), so the file may not exist yet the instant the
    /// owner loop emits its "changed" event; poll instead of a bare check.
    fn wait_for_file(path: &std::path::Path) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            // `fs::write` truncates before it writes, so an empty file means
            // the writer is between the two steps: keep waiting.
            if let Ok(content) = std::fs::read_to_string(path) {
                if !content.is_empty() {
                    return content;
                }
            }
            if std::time::Instant::now() >= deadline {
                panic!("{path:?} was not written in time");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn connect(path: &std::path::Path) -> UnixStream {
        let s = UnixStream::connect(path).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s
    }

    #[test]
    fn register_gets_welcome_and_focus_makes_source() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1000, initial_path: "/home/a".into() }).unwrap();
        let welcome: Welcome = recv(&mut a).unwrap().unwrap();
        assert_eq!(welcome.source, None);
        assert!(welcome.dests.is_empty());
        let id_a = welcome.lister_id;

        send(&mut a, &ToHub::FocusGained).unwrap();
        let rc: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(rc, ToLister::RoleChanged { source, .. } if source == Some(id_a)));

        let mut saw_reg = false;
        while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(200)) {
            if matches!(ev, HubEvent::ListerRegistered { .. }) {
                saw_reg = true;
            }
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
        let _wa: Welcome = recv(&mut a).unwrap().unwrap();
        send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into() }).unwrap();
        let _wb: Welcome = recv(&mut b).unwrap().unwrap();

        send(&mut a, &ToHub::FocusGained).unwrap();
        let _ra: ToLister = recv(&mut a).unwrap().unwrap();
        let _rb: ToLister = recv(&mut b).unwrap().unwrap();
        send(&mut b, &ToHub::FocusGained).unwrap();
        let rb2: ToLister = recv(&mut b).unwrap().unwrap();
        match rb2 {
            ToLister::RoleChanged { source, dests, .. } => {
                assert!(source.is_some());
                assert_eq!(dests.len(), 1);
                assert_ne!(Some(dests[0]), source);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn set_desktop_settings_emits_a_changed_event() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Keep desktop.json out of the developer's real config dir.
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());

        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));

        let mut c = connect(&path);
        let mut ds = DesktopSettings::default();
        ds.icon_size = 5; // below ICON_SIZE_RANGE.0, hub must clamp
        send(&mut c, &ToHub::SetDesktopSettings(ds)).unwrap();

        let mut got = None;
        while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(500)) {
            if let HubEvent::DesktopSettingsChanged(d) = ev {
                got = Some(d);
                break;
            }
        }
        let got = got.expect("DesktopSettingsChanged not emitted");
        assert_eq!(got.icon_size, opusdm_ipc::ICON_SIZE_RANGE.0);

        // desktop.json landed under the temp config dir.
        wait_for_file(&cfg.path().join("opusdm").join("desktop.json"));
    }

    #[test]
    fn set_background_step_applies_and_persists_the_image() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let img_dir = tempfile::tempdir().unwrap();
        let img = img_dir.path().join("wall.png");
        std::fs::write(&img, b"not really a png").unwrap();

        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut c = connect(&path);
        send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut c).unwrap().unwrap();

        let mut args = std::collections::BTreeMap::new();
        args.insert("path".to_string(), img.to_string_lossy().into_owned());
        args.insert("fit".to_string(), "tile".to_string());
        let steps = vec![opusdm_ipc::function::FnStep::Internal {
            cmd: opusdm_ipc::function::InternalCmd::SetBackground,
            args: opusdm_ipc::function::InternalArgs(args),
        }];
        send(&mut c, &ToHub::RunFunction { steps }).unwrap();
        let q: ToLister = recv(&mut c).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection));
        send(&mut c, &ToHub::SelectionReply { paths: vec![], cwd: img_dir.path().to_path_buf() }).unwrap();

        let mut got = None;
        while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(1000)) {
            if let HubEvent::ThemeChanged(t) = ev {
                got = Some(t);
                break;
            }
        }
        let theme = got.expect("ThemeChanged not emitted");
        assert_eq!(theme.background.mode, opusdm_ipc::BackgroundMode::Image);
        assert_eq!(theme.background.image_path, Some(std::fs::canonicalize(&img).unwrap()));
        assert_eq!(theme.background.image_fit, opusdm_ipc::ImageFit::Tile);
        wait_for_file(&cfg.path().join("opusdm").join("settings.json"));
    }

    /// Read events until `pick` returns `Some`, or `wait` passes without one.
    fn next_matching<T>(handle: &ServerHandle, wait: Duration, pick: impl Fn(HubEvent) -> Option<T>) -> Option<T> {
        while let Ok(ev) = handle.evt_rx.recv_timeout(wait) {
            if let Some(v) = pick(ev) {
                return Some(v);
            }
        }
        None
    }

    #[test]
    fn leave_out_from_a_lister_emits_left_out_add_for_existing_paths_only() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("real.txt");
        std::fs::write(&file, b"x").unwrap();
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut c = connect(&path);

        // A path that does not exist (e.g. inside an archive) is refused.
        send(&mut c, &ToHub::LeaveOut { paths: vec![td.path().join("archive.zip#inner/x")] }).unwrap();
        let none = next_matching(&handle, Duration::from_millis(300), |ev| match ev {
            HubEvent::LeftOutAdd(p) => Some(p),
            _ => None,
        });
        assert!(none.is_none(), "a missing path must not be left out");

        send(&mut c, &ToHub::LeaveOut { paths: vec![file.clone()] }).unwrap();
        let got = next_matching(&handle, Duration::from_millis(1000), |ev| match ev {
            HubEvent::LeftOutAdd(p) => Some(p),
            _ => None,
        });
        assert_eq!(got, Some(vec![file]));
    }

    #[test]
    fn leave_out_and_put_away_steps_emit_hub_events() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("real.txt");
        std::fs::write(&file, b"x").unwrap();
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut c = connect(&path);
        send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut c).unwrap().unwrap();

        for cmd in [opusdm_ipc::function::InternalCmd::LeaveOut, opusdm_ipc::function::InternalCmd::PutAway] {
            let steps = vec![opusdm_ipc::function::FnStep::Internal {
                cmd,
                args: opusdm_ipc::function::InternalArgs::default(),
            }];
            send(&mut c, &ToHub::RunFunction { steps }).unwrap();
            let q: ToLister = recv(&mut c).unwrap().unwrap();
            assert!(matches!(q, ToLister::QuerySelection));
            send(&mut c, &ToHub::SelectionReply { paths: vec![file.clone()], cwd: td.path().to_path_buf() }).unwrap();
            let expect_add = cmd == opusdm_ipc::function::InternalCmd::LeaveOut;
            let got = next_matching(&handle, Duration::from_millis(1000), |ev| match ev {
                HubEvent::LeftOutAdd(p) if expect_add => Some(p),
                HubEvent::LeftOutRemove(p) if !expect_add => Some(p),
                _ => None,
            });
            assert_eq!(got, Some(vec![file.clone()]), "{cmd:?}");
            // Drain the function's own replies before the next run.
            loop {
                match recv::<_, ToLister>(&mut c) {
                    Ok(Some(ToLister::FunctionDone)) => break,
                    Ok(Some(_)) => continue,
                    _ => panic!("no FunctionDone"),
                }
            }
        }
    }

    #[test]
    fn set_settings_persists_and_emits_a_changed_event() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Keep settings.json out of the developer's real config dir.
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());

        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));

        let mut c = connect(&path);
        let mut settings = opusdm_ipc::Settings::default();
        settings.theme.icon_set = opusdm_ipc::IconSetId::Custom("breeze-dark".to_string());
        settings.theme.background.gradient_angle = 900; // above GRADIENT_ANGLE_MAX, hub must clamp
        settings.appearance.monospace_list = true;
        send(&mut c, &ToHub::SetSettings(settings.clone())).unwrap();

        let mut got = None;
        while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(500)) {
            if let HubEvent::ThemeChanged(t) = ev {
                got = Some(t);
                break;
            }
        }
        let got = got.expect("ThemeChanged not emitted");
        assert_eq!(got.icon_set, opusdm_ipc::IconSetId::Custom("breeze-dark".to_string()));
        assert_eq!(got.background.gradient_angle, opusdm_ipc::GRADIENT_ANGLE_MAX);

        // settings.json landed under the temp config dir and carries appearance too.
        let written = wait_for_file(&cfg.path().join("opusdm").join("settings.json"));
        let back: opusdm_ipc::Settings = serde_json::from_str(&written).unwrap();
        assert!(back.appearance.monospace_list);
    }

    #[test]
    fn goodbye_drops_lister_and_notifies_gtk() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 9, initial_path: "/a".into() }).unwrap();
        let _wa: Welcome = recv(&mut a).unwrap().unwrap();
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

    #[test]
    fn run_function_queries_source_and_dest_then_executes() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));

        // SOURCE lister.
        let mut src = connect(&path);
        send(&mut src, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut src).unwrap().unwrap();

        // DEST lister.
        let mut dst = connect(&path);
        send(&mut dst, &ToHub::Register { pid: 2, initial_path: "/dst".into() }).unwrap();
        let _w: Welcome = recv(&mut dst).unwrap().unwrap();

        // `start_function_run` treats the RunFunction sender as SOURCE
        // directly (it never consults `roles.source()`), so `src` needs no
        // role at all here; it only reads `roles.dests()` for the DEST
        // connection, so `dst` needs an explicit DEST role. `SetRole`
        // (unlike `FocusGained`, which always makes the focused Lister
        // SOURCE and demotes the previous SOURCE to DEST - see
        // `two_listers_focus_toggles_source_dest` above) sets exactly the
        // role named, with no source required first.
        send(&mut dst, &ToHub::SetRole(opusdm_ipc::Role::Dest)).unwrap();
        // Drain the RoleChanged broadcast this triggers on both connections.
        let _r: ToLister = recv(&mut src).unwrap().unwrap();
        let _r: ToLister = recv(&mut dst).unwrap().unwrap();

        let steps = vec![opusdm_ipc::function::FnStep::External {
            argv: vec!["true".into()],
            cwd_source: true,
        }];
        send(&mut src, &ToHub::RunFunction { steps }).unwrap();

        // Hub must ask both listers for their selection.
        let q1: ToLister = recv(&mut src).unwrap().unwrap();
        assert!(matches!(q1, ToLister::QuerySelection));
        let q2: ToLister = recv(&mut dst).unwrap().unwrap();
        assert!(matches!(q2, ToLister::QuerySelection));

        // `cwd_source: true` on the step makes the executor's subprocess
        // really `current_dir()` into this path, so it must exist on disk
        // (unlike `initial_path` above, which the hub never uses as a
        // process cwd) - a real temp dir stands in for the Lister's
        // actual open directory.
        let real_cwd = std::env::temp_dir();
        send(&mut src, &ToHub::SelectionReply { paths: vec!["/src/a".into()], cwd: real_cwd.clone() }).unwrap();
        send(&mut dst, &ToHub::SelectionReply { paths: vec![], cwd: real_cwd }).unwrap();

        // Execution happens on a worker thread; poll src for the result.
        let mut done = false;
        for _ in 0..50 {
            match recv::<_, ToLister>(&mut src) {
                Ok(Some(ToLister::FunctionDone)) => {
                    done = true;
                    break;
                }
                Ok(Some(_)) => continue, // FunctionOutput / FunctionStepDone lines
                _ => break,
            }
        }
        assert!(done, "expected FunctionDone on the requesting (SOURCE) connection");
        let _ = handle;
    }

    /// SOURCE and DEST listers, DEST role set, ready for a function run.
    fn source_and_dest(path: &std::path::Path) -> (UnixStream, UnixStream) {
        let mut src = connect(path);
        send(&mut src, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut src).unwrap().unwrap();
        let mut dst = connect(path);
        send(&mut dst, &ToHub::Register { pid: 2, initial_path: "/dst".into() }).unwrap();
        let _w: Welcome = recv(&mut dst).unwrap().unwrap();
        send(&mut dst, &ToHub::SetRole(opusdm_ipc::Role::Dest)).unwrap();
        let _r: ToLister = recv(&mut src).unwrap().unwrap();
        let _r: ToLister = recv(&mut dst).unwrap().unwrap();
        (src, dst)
    }

    #[test]
    fn remote_jobs_are_handed_back_to_the_requester_with_the_destination_resolved() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut src, mut dst) = source_and_dest(&path);
        send(&mut src, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
        recv_until(&mut src, |m| matches!(m, ToLister::RoleChanged { source: Some(_), .. }).then_some(()));
        send(&mut dst, &ToHub::PathChanged("sftp://me@host/in".into())).unwrap();
        // The hub reads each connection in order, so once it answers this the
        // path is recorded (the two Listers' streams race otherwise).
        send(&mut dst, &ToHub::QueryCaches).unwrap();
        recv_until(&mut dst, |m| matches!(m, ToLister::CacheList(_)).then_some(()));

        // Remote destination, local source: bounced with the DEST path.
        send(&mut src, &ToHub::RunJob { op: Op::Copy, srcs: vec!["/tmp/a".into()], dest_dir: None, options: Default::default() }).unwrap();
        let (op, srcs, dest) = recv_until(&mut src, |m| match m {
            ToLister::RemoteTransfer { op, srcs, dest_dir } => Some((op, srcs, dest_dir)),
            _ => None,
        });
        assert_eq!((op, srcs, dest), (Op::Copy, vec!["/tmp/a".into()], "sftp://me@host/in".into()));

        // Remote source with an explicit destination (Delete): bounced too.
        send(&mut src, &ToHub::RunJob { op: Op::Delete, srcs: vec!["sftp://me@host/x".into()], dest_dir: Some("sftp://me@host/".into()), options: Default::default() }).unwrap();
        let op = recv_until(&mut src, |m| match m {
            ToLister::RemoteTransfer { op, .. } => Some(op),
            _ => None,
        });
        assert_eq!(op, Op::Delete);
    }

    /// Run a Confirm step from one Lister and answer the question with `ok`.
    /// Returns whether the function finished (`true`) or failed (`false`).
    fn confirm_round_trip(ok: bool) -> bool {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        let mut args = std::collections::BTreeMap::new();
        args.insert("message".to_string(), "Go on?".to_string());
        let steps = vec![FnStep::Internal { cmd: opusdm_ipc::function::InternalCmd::Confirm, args: opusdm_ipc::function::InternalArgs(args) }];
        send(&mut a, &ToHub::RunFunction { steps }).unwrap();
        recv_until(&mut a, |m| matches!(m, ToLister::QuerySelection).then_some(()));
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();
        let (req_id, message) = recv_until(&mut a, |m| match m {
            ToLister::ConfirmPrompt { req_id, message } => Some((req_id, message)),
            _ => None,
        });
        assert_eq!(message, "Go on?");
        send(&mut a, &ToHub::ConfirmReply { req_id, ok }).unwrap();
        recv_until(&mut a, |m| match m {
            ToLister::FunctionDone => Some(true),
            ToLister::FunctionFailed { .. } => Some(false),
            _ => None,
        })
    }

    #[test]
    fn a_confirm_step_waits_for_the_users_answer() {
        assert!(confirm_round_trip(true), "a yes lets the function finish");
        assert!(!confirm_round_trip(false), "a no stops it");
    }

    #[test]
    fn a_run_window_keeps_receiving_output_after_the_function_finished() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        let mut args = std::collections::BTreeMap::new();
        args.insert("command".to_string(), "sh -c 'sleep 0.3; echo late'".to_string());
        args.insert("window".to_string(), "true".to_string());
        let steps = vec![FnStep::Internal { cmd: opusdm_ipc::function::InternalCmd::Run, args: opusdm_ipc::function::InternalArgs(args) }];
        send(&mut a, &ToHub::RunFunction { steps }).unwrap();
        recv_until(&mut a, |m| matches!(m, ToLister::QuerySelection).then_some(()));
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

        // Collect everything up to the end of the output, in arrival order.
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..40 {
            let m: ToLister = recv(&mut a).unwrap().unwrap();
            match m {
                ToLister::OutputOpen { .. } => seen.push("open".into()),
                ToLister::OutputLine { line, .. } => seen.push(format!("line:{line}")),
                ToLister::OutputEnd { exit, .. } => {
                    seen.push(format!("end:{exit:?}"));
                    break;
                }
                ToLister::FunctionDone => seen.push("done".into()),
                _ => {}
            }
        }
        let pos = |s: &str| seen.iter().position(|x| x == s).unwrap_or_else(|| panic!("{s} missing in {seen:?}"));
        assert!(pos("open") < pos("done"), "{seen:?}");
        assert!(pos("done") < pos("line:late"), "the output outlives the function: {seen:?}");
        assert!(pos("line:late") < pos("end:Some(0)"), "{seen:?}");
    }

    #[test]
    fn invalidate_reaches_every_lister() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut a, mut b) = source_and_dest(&path);
        send(&mut a, &ToHub::Invalidate("sftp://h/a".into())).unwrap();
        for lister in [&mut a, &mut b] {
            let p = recv_until(lister, |m| match m {
                ToLister::InvalidatePath(p) => Some(p),
                _ => None,
            });
            assert_eq!(p, std::path::PathBuf::from("sftp://h/a"));
        }
    }

    #[test]
    fn function_copy_conflict_is_prompted_and_the_answer_is_applied() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut src, mut dst) = source_and_dest(&path);

        let src_dir = tempfile::tempdir().unwrap();
        let dst_dir = tempfile::tempdir().unwrap();
        let src_file = src_dir.path().join("a.txt");
        std::fs::write(&src_file, b"new").unwrap();
        std::fs::write(dst_dir.path().join("a.txt"), b"existing").unwrap();

        let steps = vec![FnStep::Internal { cmd: opusdm_ipc::function::InternalCmd::Copy, args: Default::default() }];
        send(&mut src, &ToHub::RunFunction { steps }).unwrap();
        let _q: ToLister = recv(&mut src).unwrap().unwrap();
        let _q: ToLister = recv(&mut dst).unwrap().unwrap();
        send(&mut src, &ToHub::SelectionReply { paths: vec![src_file], cwd: src_dir.path().to_path_buf() }).unwrap();
        send(&mut dst, &ToHub::SelectionReply { paths: vec![], cwd: dst_dir.path().to_path_buf() }).unwrap();

        // The prompt goes to the Lister that started the run.
        let (mut req_id, mut done) = (None, false);
        for _ in 0..50 {
            match recv::<_, ToLister>(&mut src) {
                Ok(Some(ToLister::ConflictPrompt { req_id: id, path, .. })) => {
                    assert!(path.ends_with("a.txt"));
                    req_id = Some(id);
                    send(&mut src, &ToHub::ConflictReply { req_id: id, resolution: opusdm_ipc::Resolution::Overwrite }).unwrap();
                }
                Ok(Some(ToLister::FunctionDone)) => {
                    done = true;
                    break;
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(req_id.is_some(), "expected a ConflictPrompt on the requesting connection");
        assert!(done, "expected FunctionDone after the answer");
        assert_eq!(std::fs::read(dst_dir.path().join("a.txt")).unwrap(), b"new");
    }

    #[test]
    fn headless_function_copy_as_conflict_is_skipped_without_a_prompt() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"new").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"existing").unwrap();
        let marker = dir.path().join("marker");

        // Copy a.txt over the existing b.txt (it works in place, so no DEST
        // is needed), then a marker step proving the run went on.
        let mut args = std::collections::BTreeMap::new();
        args.insert("new_name".to_string(), "b.txt".to_string());
        let mut f = opusdm_ipc::Function::new("copy-as-then-mark");
        f.steps = vec![
            FnStep::Internal {
                cmd: opusdm_ipc::function::InternalCmd::CopyAs,
                args: opusdm_ipc::function::InternalArgs(args),
            },
            FnStep::External { argv: vec!["sh".into(), "-c".into(), format!("echo x >> {}", marker.display())], cwd_source: true },
        ];
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
        let _r: ToLister = recv(&mut a).unwrap().unwrap(); // RoleChanged

        handle.cmd_tx.send(HubCmd::RunFunctionHeadless(f.id)).unwrap();
        let q: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection), "got {q:?}");
        send(&mut a, &ToHub::SelectionReply { paths: vec![dir.path().join("a.txt")], cwd: dir.path().to_path_buf() }).unwrap();

        wait_for_lines(&marker, 1);
        assert_eq!(std::fs::read(dir.path().join("b.txt")).unwrap(), b"existing");
    }

    #[test]
    fn run_function_from_the_dest_lister_itself_does_not_wait_on_itself_twice() {
        // A Lister holding the DEST role can still be the one requesting a
        // run (e.g. it just pressed Test in its own Function Editor).
        // `start_function_run` must not pick it as its own `dest_cid` -
        // that would make `selection_reply`'s `if cid == source_cid` /
        // `else if Some(cid) == dest_cid` only ever take the first branch,
        // so `dest_sel` (and the whole run) could never complete.
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Dest)).unwrap();
        let _r: ToLister = recv(&mut a).unwrap().unwrap(); // RoleChanged

        let steps = vec![opusdm_ipc::function::FnStep::External { argv: vec!["true".into()], cwd_source: true }];
        send(&mut a, &ToHub::RunFunction { steps }).unwrap();

        // Exactly one QuerySelection - not one that will never be answered
        // because it was sent to `a` for a `dest_cid` that is also `a`.
        let q: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection));

        let real_cwd = std::env::temp_dir();
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: real_cwd }).unwrap();

        let mut done = false;
        for _ in 0..50 {
            match recv::<_, ToLister>(&mut a) {
                Ok(Some(ToLister::FunctionDone)) => {
                    done = true;
                    break;
                }
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
        assert!(done, "expected FunctionDone - the run must not wait forever on its own DEST selection");
        let _ = handle;
    }

    #[test]
    fn set_bindings_sanitizes_persists_and_broadcasts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let welcome: Welcome = recv(&mut a).unwrap().unwrap();
        assert_eq!(welcome.bindings, opusdm_ipc::Bindings::default());

        let dirty = opusdm_ipc::Bindings {
            hotkeys: vec![
                opusdm_ipc::bindings::Hotkey { key: "ctrl+U".into(), function: "f1".into(), global: false },
                opusdm_ipc::bindings::Hotkey { key: "Ctrl+u".into(), function: "f2".into(), global: false },
            ],
            ..opusdm_ipc::Bindings::default()
        };
        send(&mut a, &ToHub::SetBindings(dirty)).unwrap();

        let got: ToLister = recv(&mut a).unwrap().unwrap();
        let ToLister::BindingsUpdate(b) = got else { panic!("expected BindingsUpdate, got {got:?}") };
        assert_eq!(b.hotkeys.len(), 1, "the duplicate must be dropped, first wins");
        assert_eq!(b.hotkeys[0].key, "Ctrl+u");
        assert_eq!(b.hotkeys[0].function, "f1");

        let written = wait_for_file(&cfg.path().join("opusdm").join("hotkeys.json"));
        let back: Vec<opusdm_ipc::bindings::Hotkey> = serde_json::from_str(&written).unwrap();
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn set_formats_sanitizes_persists_and_broadcasts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let welcome: Welcome = recv(&mut a).unwrap().unwrap();
        assert_eq!(welcome.formats, opusdm_ipc::FormatStore::default());

        let mut dirty = opusdm_ipc::FormatStore::default();
        dirty.upsert(
            "/tmp".into(),
            false,
            opusdm_ipc::ListerFormat {
                columns: vec![opusdm_ipc::FormatColumn::Size, opusdm_ipc::FormatColumn::Size],
                ..opusdm_ipc::ListerFormat::builtin()
            },
        );
        send(&mut a, &ToHub::SetFormats(dirty)).unwrap();

        let got: ToLister = recv(&mut a).unwrap().unwrap();
        let ToLister::FormatsUpdate(store) = got else { panic!("expected FormatsUpdate, got {got:?}") };
        assert_eq!(
            store.entries[0].format.columns,
            vec![opusdm_ipc::FormatColumn::Size, opusdm_ipc::FormatColumn::Name],
            "duplicates dropped and Name restored"
        );

        let written = wait_for_file(&cfg.path().join("opusdm").join("formats.json"));
        let back: opusdm_ipc::FormatStore = serde_json::from_str(&written).unwrap();
        assert_eq!(back, store);
    }

    #[test]
    fn welcome_carries_the_formats_loaded_from_disk() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let mut on_disk = opusdm_ipc::FormatStore::default();
        on_disk.upsert(
            "/tmp".into(),
            true,
            opusdm_ipc::ListerFormat { hide_glob: "*.o".into(), ..opusdm_ipc::ListerFormat::builtin() },
        );
        opusdm_ipc::format::save_formats(&on_disk).unwrap();

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server_with(listener, Box::new(|_p| {}), ServerOptions::production());

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let welcome: Welcome = recv(&mut a).unwrap().unwrap();
        assert_eq!(welcome.formats, on_disk);
    }

    /// Read messages from `s` until `pick` returns a value, panicking after 30
    /// messages (role broadcasts may interleave with the reply we want).
    fn recv_until<T>(s: &mut UnixStream, pick: impl Fn(ToLister) -> Option<T>) -> T {
        for _ in 0..30 {
            let msg: ToLister = recv(s).unwrap().unwrap();
            if let Some(v) = pick(msg) {
                return v;
            }
        }
        panic!("the expected message never arrived");
    }

    /// Register a Lister and return its stream and id.
    fn register(path: &std::path::Path, pid: u32, start: &str) -> (UnixStream, ListerId) {
        let mut s = connect(path);
        send(&mut s, &ToHub::Register { pid, initial_path: start.into() }).unwrap();
        let welcome: Welcome = recv(&mut s).unwrap().unwrap();
        (s, welcome.lister_id)
    }

    #[test]
    fn query_caches_lists_every_listers_reported_paths() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut a, id_a) = register(&path, 1, "/a");
        let (mut b, id_b) = register(&path, 2, "/b");

        send(&mut a, &ToHub::CacheReport(vec!["/x".into()])).unwrap();
        send(&mut b, &ToHub::CacheReport(vec!["/y".into(), "/z".into()])).unwrap();
        // Connections are read by separate threads: b's own query is only
        // answered after its report was handled, so wait for that reply
        // before a asks.
        send(&mut b, &ToHub::QueryCaches).unwrap();
        recv_until(&mut b, |m| matches!(m, ToLister::CacheList(_)).then_some(()));
        send(&mut a, &ToHub::QueryCaches).unwrap();

        let entries = recv_until(&mut a, |m| match m {
            ToLister::CacheList(e) => Some(e),
            _ => None,
        });
        let got: Vec<(ListerId, String)> =
            entries.iter().map(|e| (e.lister, e.path.to_string_lossy().into_owned())).collect();
        assert_eq!(got, vec![(id_a, "/x".into()), (id_b, "/y".into()), (id_b, "/z".into())]);
    }

    #[test]
    fn a_disconnected_listers_cached_paths_disappear() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut a, _id_a) = register(&path, 1, "/a");
        let (mut b, _id_b) = register(&path, 2, "/b");
        send(&mut b, &ToHub::CacheReport(vec!["/y".into()])).unwrap();
        // Make sure the hub processed b's report before b goes away: b's own
        // query is answered only after its report was handled.
        send(&mut b, &ToHub::QueryCaches).unwrap();
        recv_until(&mut b, |m| matches!(m, ToLister::CacheList(_)).then_some(()));
        send(&mut a, &ToHub::QueryCaches).unwrap();
        let first = recv_until(&mut a, |m| match m {
            ToLister::CacheList(e) => Some(e),
            _ => None,
        });
        assert_eq!(first.len(), 1);
        drop(b);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            send(&mut a, &ToHub::QueryCaches).unwrap();
            let now = recv_until(&mut a, |m| match m {
                ToLister::CacheList(e) => Some(e),
                _ => None,
            });
            if now.is_empty() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "b's paths never disappeared");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn free_all_caches_reaches_every_lister() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut a, _) = register(&path, 1, "/a");
        let (mut b, _) = register(&path, 2, "/b");
        send(&mut a, &ToHub::FreeAllCaches).unwrap();
        recv_until(&mut a, |m| matches!(m, ToLister::FreeCaches).then_some(()));
        recv_until(&mut b, |m| matches!(m, ToLister::FreeCaches).then_some(()));
    }

    #[test]
    fn jump_to_reaches_only_the_target_and_ignores_unknown_ids() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut a, _id_a) = register(&path, 1, "/a");
        let (mut b, id_b) = register(&path, 2, "/b");

        send(&mut a, &ToHub::JumpTo { lister: ListerId(9999), path: "/nowhere".into() }).unwrap();
        send(&mut a, &ToHub::JumpTo { lister: id_b, path: "/target".into() }).unwrap();
        let goto = recv_until(&mut b, |m| match m {
            ToLister::GoTo { path } => Some(path),
            _ => None,
        });
        assert_eq!(goto, std::path::PathBuf::from("/target"));

        // a must not have been sent a GoTo: the next reply it reads is the
        // CacheList, with no GoTo before it.
        send(&mut a, &ToHub::QueryCaches).unwrap();
        recv_until(&mut a, |m| match m {
            ToLister::GoTo { .. } => panic!("a GoTo reached the wrong Lister"),
            ToLister::CacheList(_) => Some(()),
            _ => None,
        });
    }

    #[test]
    fn welcome_carries_the_bindings_loaded_from_disk() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let on_disk = opusdm_ipc::Bindings {
            hotkeys: vec![opusdm_ipc::bindings::Hotkey { key: "Ctrl+j".into(), function: "f".into(), global: false }],
            ..opusdm_ipc::Bindings::default()
        };
        opusdm_ipc::bindings::save_bindings(&on_disk).unwrap();

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server_with(listener, Box::new(|_p| {}), ServerOptions::production());

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let welcome: Welcome = recv(&mut a).unwrap().unwrap();
        assert_eq!(welcome.bindings, on_disk);
    }

    /// A saved function whose only step appends a line to `marker`.
    fn marker_function(name: &str, marker: &std::path::Path) -> opusdm_ipc::Function {
        let mut f = opusdm_ipc::Function::new(name);
        f.steps = vec![FnStep::External {
            argv: vec!["sh".into(), "-c".into(), format!("echo x >> {}", marker.display())],
            cwd_source: true,
        }];
        f
    }

    /// Poll until `marker` has at least `lines` lines, or panic.
    fn wait_for_lines(marker: &std::path::Path, lines: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let count = std::fs::read_to_string(marker).map(|s| s.lines().count()).unwrap_or(0);
            if count >= lines {
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!("{marker:?} has {count} lines, wanted {lines}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn run_function_by_id_with_an_unknown_id_reports_failure() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        send(&mut a, &ToHub::RunFunctionById { id: "nope".into() }).unwrap();

        let got: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(got, ToLister::FunctionFailed { step_idx: 0, .. }), "got {got:?}");
    }

    #[test]
    fn run_function_by_id_runs_a_saved_function() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("touch", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();

        send(&mut a, &ToHub::RunFunctionById { id: f.id.clone() }).unwrap();
        let q: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection));
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn headless_run_without_listers_executes_immediately() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("touch", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, _path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        handle.cmd_tx.send(HubCmd::RunFunctionHeadless(f.id)).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn headless_run_queries_the_source_lister() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("touch", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
        let _r: ToLister = recv(&mut a).unwrap().unwrap(); // RoleChanged

        handle.cmd_tx.send(HubCmd::RunFunctionHeadless(f.id)).unwrap();
        let q: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection), "got {q:?}");
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn two_runs_waiting_on_one_lister_both_complete_in_order() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker_a = out.path().join("a");
        let marker_b = out.path().join("b");
        let fa = marker_function("a", &marker_a);
        let fb = marker_function("b", &marker_b);
        opusdm_ipc::function::save_functions(&[fa.clone(), fb.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();

        // Both requests reach the hub before the lister answers either.
        send(&mut a, &ToHub::RunFunctionById { id: fa.id.clone() }).unwrap();
        send(&mut a, &ToHub::RunFunctionById { id: fb.id.clone() }).unwrap();
        for _ in 0..2 {
            let q: ToLister = recv(&mut a).unwrap().unwrap();
            assert!(matches!(q, ToLister::QuerySelection), "got {q:?}");
        }
        for _ in 0..2 {
            send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();
        }

        wait_for_lines(&marker_a, 1);
        wait_for_lines(&marker_b, 1);
    }

    /// Server options for an event test: these bindings, no disk read.
    fn with_bindings(bindings: opusdm_ipc::Bindings) -> ServerOptions {
        ServerOptions { initial_bindings: Some(bindings), ..ServerOptions::default() }
    }

    fn script(event: opusdm_ipc::bindings::ScriptEvent, function: &str) -> opusdm_ipc::bindings::ScriptBinding {
        opusdm_ipc::bindings::ScriptBinding { event, function: function.into() }
    }

    #[test]
    fn startup_event_runs_the_bound_function() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, _path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::Startup, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));
        handle.cmd_tx.send(HubCmd::EmitScriptEvent(opusdm_ipc::bindings::ScriptEvent::Startup)).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn an_event_runs_only_the_functions_bound_to_it() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, _path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::ListerClosed, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));
        handle.cmd_tx.send(HubCmd::EmitScriptEvent(opusdm_ipc::bindings::ScriptEvent::Startup)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists(), "a Startup event must not run a ListerClosed hook");
    }

    #[test]
    fn lister_opened_runs_once_and_is_throttled_inside_the_cooldown() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::ListerOpened, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let _handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        wait_for_lines(&marker, 1);

        // A second lister right away is the loop a self-spawning hook would
        // create: it must be throttled, not run again.
        let mut b = connect(&path);
        send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into() }).unwrap();
        let _w: Welcome = recv(&mut b).unwrap().unwrap();
        std::thread::sleep(Duration::from_millis(400));
        let lines = std::fs::read_to_string(&marker).unwrap().lines().count();
        assert_eq!(lines, 1, "the second ListerOpened inside the cooldown must be dropped");
    }

    #[test]
    fn lister_closed_runs_the_bound_function() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::ListerClosed, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let _handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        send(&mut a, &ToHub::Goodbye).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn shutdown_hooks_finish_before_the_listers_are_told_to_quit() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::Shutdown, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();

        handle.cmd_tx.send(HubCmd::Shutdown).unwrap();
        let got: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(got, ToLister::Shutdown), "got {got:?}");
        let lines = std::fs::read_to_string(&marker).map(|s| s.lines().count()).unwrap_or(0);
        assert_eq!(lines, 1, "the Shutdown hook must have finished before the Shutdown message");
    }

    #[test]
    fn shutdown_and_wait_acknowledges_only_after_the_hooks_finished() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        // A hook that takes a moment: the ack must not arrive before it ends.
        let mut f = opusdm_ipc::Function::new("slow hook");
        f.steps = vec![FnStep::External {
            argv: vec!["sh".into(), "-c".into(), format!("sleep 0.3; echo x >> {}", marker.display())],
            cwd_source: true,
        }];
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, _path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::Shutdown, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

        let (ack_tx, ack_rx) = channel();
        handle.cmd_tx.send(HubCmd::ShutdownAndWait(ack_tx)).unwrap();
        ack_rx.recv_timeout(Duration::from_secs(5)).expect("no acknowledgement");
        let lines = std::fs::read_to_string(&marker).map(|s| s.lines().count()).unwrap_or(0);
        assert_eq!(lines, 1, "the ack came before the Shutdown hook finished");
    }

    #[test]
    fn set_hidden_tells_listers_and_the_gtk_side_once_and_toggle_flips_it() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut c = connect(&path);
        send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut c).unwrap().unwrap();
        let hidden_event = |ev: HubEvent| match ev {
            HubEvent::HiddenChanged(h) => Some(h),
            _ => None,
        };

        handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
        assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(true));
        loop {
            match recv::<_, ToLister>(&mut c).unwrap() {
                Some(ToLister::SetHidden(true)) => break,
                Some(_) => continue,
                None => panic!("connection closed before SetHidden(true)"),
            }
        }
        // Already hidden: no second event.
        handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
        assert_eq!(next_matching(&handle, Duration::from_millis(300), hidden_event), None);
        // Toggle reveals.
        handle.cmd_tx.send(HubCmd::ToggleHidden).unwrap();
        assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(false));
        loop {
            match recv::<_, ToLister>(&mut c).unwrap() {
                Some(ToLister::SetHidden(false)) => break,
                Some(_) => continue,
                None => panic!("connection closed before SetHidden(false)"),
            }
        }
    }

    #[test]
    fn a_requested_lister_registering_while_hidden_reveals_everything() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
        // The user asked for a new Lister (desktop menu, "New Lister" button).
        handle.cmd_tx.send(HubCmd::SpawnRequested { start_path: None }).unwrap();
        let hidden_event = |ev: HubEvent| match ev {
            HubEvent::HiddenChanged(h) => Some(h),
            _ => None,
        };
        assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(true));
        let mut c = connect(&path);
        send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut c).unwrap().unwrap();
        assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(false));
    }

    #[test]
    fn hide_reveal_and_toggle_steps_drive_the_hidden_state() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut c = connect(&path);
        send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut c).unwrap().unwrap();
        let hidden_event = |ev: HubEvent| match ev {
            HubEvent::HiddenChanged(h) => Some(h),
            _ => None,
        };
        for (cmd, expected) in [
            (opusdm_ipc::function::InternalCmd::Hide, true),
            (opusdm_ipc::function::InternalCmd::ToggleHide, false),
            (opusdm_ipc::function::InternalCmd::ToggleHide, true),
            (opusdm_ipc::function::InternalCmd::Reveal, false),
        ] {
            let steps = vec![opusdm_ipc::function::FnStep::Internal {
                cmd,
                args: opusdm_ipc::function::InternalArgs::default(),
            }];
            send(&mut c, &ToHub::RunFunction { steps }).unwrap();
            let q = loop {
                match recv::<_, ToLister>(&mut c).unwrap() {
                    Some(ToLister::QuerySelection) => break true,
                    Some(_) => continue,
                    None => break false,
                }
            };
            assert!(q, "no QuerySelection for {cmd:?}");
            send(&mut c, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();
            assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(expected), "{cmd:?}");
            loop {
                match recv::<_, ToLister>(&mut c).unwrap() {
                    Some(ToLister::FunctionDone) => break,
                    Some(_) => continue,
                    None => panic!("no FunctionDone"),
                }
            }
        }
    }

    #[test]
    fn a_lister_nobody_asked_for_registering_while_hidden_is_told_to_hide_too() {
        // The initial Lister of a Startup script that hides the desktop.
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
        let hidden_event = |ev: HubEvent| match ev {
            HubEvent::HiddenChanged(h) => Some(h),
            _ => None,
        };
        assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(true));
        let mut c = connect(&path);
        send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut c).unwrap().unwrap();
        loop {
            match recv::<_, ToLister>(&mut c).unwrap() {
                Some(ToLister::SetHidden(true)) => break,
                Some(_) => continue,
                None => panic!("the new Lister was not told to hide"),
            }
        }
        assert_eq!(
            next_matching(&handle, Duration::from_millis(300), hidden_event),
            None,
            "an unrequested Lister must not reveal the desktop"
        );
    }

    #[test]
    fn reveal_presents_only_the_source_lister_after_telling_everyone() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        let mut b = connect(&path);
        send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into() }).unwrap();
        let _w: Welcome = recv(&mut b).unwrap().unwrap();
        send(&mut a, &ToHub::FocusGained).unwrap();
        loop {
            match recv::<_, ToLister>(&mut a).unwrap() {
                Some(ToLister::RoleChanged { .. }) => break,
                Some(_) => continue,
                None => panic!("no RoleChanged"),
            }
        }
        handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
        handle.cmd_tx.send(HubCmd::SetHidden(false)).unwrap();
        // SOURCE Lister a: SetHidden(true), SetHidden(false), then Present.
        let mut seen_reveal = false;
        loop {
            match recv::<_, ToLister>(&mut a).unwrap() {
                Some(ToLister::SetHidden(false)) => seen_reveal = true,
                Some(ToLister::Present) => {
                    assert!(seen_reveal, "Present must come after SetHidden(false)");
                    break;
                }
                Some(_) => continue,
                None => panic!("the SOURCE Lister never got Present"),
            }
        }
        // Lister b (not SOURCE) is told to reveal but never asked to present.
        let mut got_reveal = false;
        b.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        while let Ok(Some(msg)) = recv::<_, ToLister>(&mut b) {
            match msg {
                ToLister::SetHidden(false) => got_reveal = true,
                ToLister::Present => panic!("a non-SOURCE Lister must not be presented"),
                _ => {}
            }
        }
        assert!(got_reveal);
    }
}
