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
use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::function::{FnStep, FunctionId, SelectionCtx};
use opusdm_ipc::{DesktopSettings, ListerId, Op, Outcome, Resolution, ToHub, ToLister, Welcome};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

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
}

impl Default for ServerOptions {
    /// Options for tests: no disk read.
    fn default() -> Self {
        Self { global_keys: false, initial_bindings: Some(Bindings::default()) }
    }
}

impl ServerOptions {
    /// Options for the real hub: load the bindings from disk.
    pub fn production() -> Self {
        Self { global_keys: true, initial_bindings: None }
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

/// Messages persisted to disk off the owner thread.
enum PersistMsg {
    Settings(opusdm_ipc::Settings),
    Desktop(DesktopSettings),
    Bindings(Bindings),
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

/// A Function run waiting on `SelectionReply` from SOURCE and/or DEST,
/// tracked between `ToHub::RunFunction` and the point every slot it needs
/// has been filled.
struct PendingFunctionRun {
    requester_cid: u64,
    steps: Vec<FnStep>,
    source_cid: u64,
    dest_cid: Option<u64>,
    source_sel: Option<SelectionCtx>,
    dest_sel: Option<SelectionCtx>,
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
    /// Loaded once at startup; updated in place by `ToHub::SetBindings`.
    current_bindings: Bindings,
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
            Conn { tx, shutdown, registered: false, pid: 0, path: PathBuf::new() },
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

            ToHub::SetSettings(mut new_settings) => {
                new_settings.theme.background = new_settings.theme.background.sanitized();
                self.current_settings = new_settings;
                let _ = self.persist_tx.send(PersistMsg::Settings(self.current_settings.clone()));
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::SettingsUpdate(self.current_settings.clone()).into());
                }
                let _ = self.evt_tx.send(HubEvent::ThemeChanged(self.current_settings.theme.clone()));
                let _ = self.evt_tx.send(HubEvent::AppearanceChanged(self.current_settings.appearance.clone()));
            }

            ToHub::SetBindings(new_bindings) => {
                self.current_bindings = bindings::sanitize(new_bindings);
                let _ = self.persist_tx.send(PersistMsg::Bindings(self.current_bindings.clone()));
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::BindingsUpdate(self.current_bindings.clone()).into());
                }
                self.apply_global_keys();
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

            ToHub::SpawnLister { start_path } => {
                (self.on_spawn)(start_path);
            }

            ToHub::RunJob { op, srcs, dest_dir, options } => {
                self.lister_run_job(cid, op, srcs, dest_dir, options);
            }

            ToHub::RunFunction { steps } => {
                self.start_function_run(cid, Some(cid), steps);
            }

            ToHub::RunFunctionById { id } => {
                self.run_function_by_id(cid, Some(cid), &id);
            }

            ToHub::SelectionReply { paths, cwd } => {
                self.selection_reply(cid, paths, cwd);
            }

            // Tab in a Lister: raise the pane holding the opposite role.
            ToHub::PresentLister(target) => {
                if let Some(c) = self.conns.get_mut(&target.0).filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::Present.into());
                }
            }

            ToHub::ConflictReply { req_id, resolution } => {
                if let Some(tx) = self.replies.remove(&req_id) {
                    let _ = tx.send(resolution);
                }
            }

            ToHub::Goodbye => {
                self.drop_lister(cid);
            }
        }
    }

    /// Start a Function run for `requester`. With `source == Some(cid)` the
    /// hub asks `cid` (SOURCE) and, if one exists, the first DEST Lister other
    /// than `cid` for their selection first. With `None` there is no Lister to
    /// ask: the run starts at once with an empty selection in `$HOME`.
    /// Returns the run id.
    fn start_function_run(&mut self, requester: u64, source: Option<u64>, steps: Vec<FnStep>) -> u64 {
        let run_id = self.next_run_id;
        self.next_run_id += 1;
        let Some(source_cid) = source else {
            self.function_requesters.insert(run_id, requester);
            let empty = SelectionCtx { paths: Vec::new(), cwd: home_dir() };
            crate::functions::spawn_executor(run_id, steps, empty, None, self.function_events_tx.clone());
            return run_id;
        };
        // Exclude the source itself: a Lister that currently holds a DEST role
        // can still be the one requesting the run. Picking it as its own
        // `dest_cid` would make `selection_reply` only ever fill `source_sel`
        // (the two branches are `if`/`else if` on the same `cid`), so
        // `dest_sel` - and the run - could never complete.
        let dest_cid = self.roles.dests().iter().map(|d| d.0).find(|&d| d != source_cid);
        self.pending_functions.insert(
            run_id,
            PendingFunctionRun {
                requester_cid: requester,
                steps,
                source_cid,
                dest_cid,
                source_sel: None,
                dest_sel: None,
            },
        );
        self.query_selection(source_cid, run_id);
        if let Some(d) = dest_cid {
            self.query_selection(d, run_id);
        }
        run_id
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

    /// Queue `run_id` on `cid`'s FIFO of pending selection requests and ask
    /// the Lister for its selection.
    fn query_selection(&mut self, cid: u64, run_id: u64) {
        self.selection_waiters.entry(cid).or_default().push_back(run_id);
        if let Some(c) = self.conns.get_mut(&cid) {
            let _ = c.tx.send(ToLister::QuerySelection.into());
        }
    }

    /// Load the saved function `id`, validate it and start it for `requester`
    /// (see [`Self::start_function_run`]). A missing or invalid function is
    /// reported through [`Self::function_failed`]. Returns the run id.
    fn run_function_by_id(&mut self, requester: u64, source: Option<u64>, id: &str) -> Option<u64> {
        let Some(function) = opusdm_ipc::function::load_functions().into_iter().find(|f| f.id == id) else {
            self.function_failed(requester, format!("function '{id}' not found (it may have been deleted)"));
            return None;
        };
        let problems = opusdm_ipc::function::validate(&function);
        if let Some(first) = problems.first() {
            self.function_failed(requester, format!("function '{}' is invalid: {}", function.name, first.message));
            return None;
        }
        Some(self.start_function_run(requester, source, function.steps))
    }

    /// Run the saved function `id` on the hub's own initiative: SOURCE is the
    /// Lister holding the SOURCE role (if it is registered), else nothing.
    fn run_headless(&mut self, id: &str) -> Option<u64> {
        let source = self
            .roles
            .source()
            .map(|l| l.0)
            .filter(|cid| self.conns.get(cid).is_some_and(|c| c.registered));
        self.run_function_by_id(HUB_REQUESTER, source, id)
    }

    /// Tell `requester` a run could not start. A hub-originated run has no
    /// Lister to tell, so the failure goes to the log.
    fn function_failed(&mut self, requester: u64, error: String) {
        if requester == HUB_REQUESTER {
            opusdm_ipc::err_log!("hub", "function run failed: {error}");
            return;
        }
        if let Some(c) = self.conns.get_mut(&requester) {
            let _ = c.tx.send(ToLister::FunctionFailed { step_idx: 0, error }.into());
        }
    }

    /// `ToHub::SelectionReply` from `cid`: fill in whichever slot (source or
    /// dest) of its pending run this answers, and start execution once every
    /// slot the run needs is filled.
    fn selection_reply(&mut self, cid: u64, paths: Vec<PathBuf>, cwd: PathBuf) {
        let Some(queue) = self.selection_waiters.get_mut(&cid) else { return };
        let Some(run_id) = queue.pop_front() else { return };
        if queue.is_empty() {
            self.selection_waiters.remove(&cid);
        }
        let Some(run) = self.pending_functions.get_mut(&run_id) else { return };
        let sel = SelectionCtx { paths, cwd };
        if cid == run.source_cid {
            run.source_sel = Some(sel);
        } else if Some(cid) == run.dest_cid {
            run.dest_sel = Some(sel);
        }
        let ready = run.source_sel.is_some() && (run.dest_cid.is_none() || run.dest_sel.is_some());
        if !ready {
            return;
        }
        let run = self.pending_functions.remove(&run_id).unwrap();
        self.function_requesters.insert(run_id, run.requester_cid);
        crate::functions::spawn_executor(
            run_id,
            run.steps,
            run.source_sel.unwrap(),
            run.dest_sel,
            self.function_events_tx.clone(),
        );
    }

    /// A `FnEvent` arrived from a running Function's executor thread; turn
    /// it into the matching `ToLister` and send it to whichever connection
    /// asked for this run. A hub-originated run has no such connection: its
    /// failures are logged instead.
    fn function_event(&mut self, run_id: u64, ev: crate::functions::FnEvent) {
        let Some(&requester) = self.function_requesters.get(&run_id) else { return };
        let terminal = matches!(ev, crate::functions::FnEvent::Failed { .. } | crate::functions::FnEvent::AllDone);
        if terminal {
            self.function_requesters.remove(&run_id);
            self.event_guard.finish(run_id);
        }
        if requester == HUB_REQUESTER {
            match ev {
                crate::functions::FnEvent::Failed { step_idx, error } => {
                    opusdm_ipc::err_log!("hub", "function run {run_id} failed at step {step_idx}: {error}");
                }
                crate::functions::FnEvent::Output { step_idx, line } => {
                    opusdm_ipc::debug_log!("hub", "function run {run_id} step {step_idx}: {line}");
                }
                _ => {}
            }
            return;
        }
        let msg = match ev {
            crate::functions::FnEvent::Output { step_idx, line } => ToLister::FunctionOutput { step_idx, line },
            crate::functions::FnEvent::StepDone { step_idx, exit_code } => {
                ToLister::FunctionStepDone { step_idx, exit_code }
            }
            crate::functions::FnEvent::Failed { step_idx, error } => ToLister::FunctionFailed { step_idx, error },
            crate::functions::FnEvent::AllDone => ToLister::FunctionDone,
        };
        if let Some(c) = self.conns.get_mut(&requester) {
            let _ = c.tx.send(msg.into());
        }
    }

    /// Clean up any Function-run bookkeeping referencing `cid`, called when
    /// its connection drops. A pending run waiting on `cid` for one of its
    /// two selections can now never complete (its other side, if any, would
    /// otherwise wait forever); a running Function's events for a
    /// since-dropped requester are also dropped instead of accumulating.
    fn drop_function_state(&mut self, cid: u64) {
        let doomed: Vec<u64> = self
            .pending_functions
            .iter()
            .filter(|(_, run)| run.source_cid == cid || run.dest_cid == Some(cid))
            .map(|(run_id, _)| *run_id)
            .collect();
        for run_id in doomed {
            self.pending_functions.remove(&run_id);
            self.event_guard.finish(run_id);
            for queue in self.selection_waiters.values_mut() {
                queue.retain(|&r| r != run_id);
            }
        }
        self.selection_waiters.remove(&cid);
        self.selection_waiters.retain(|_, queue| !queue.is_empty());
        self.function_requesters.retain(|_, requester| *requester != cid);
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
            HubCmd::SpawnRequested { start_path } => (self.on_spawn)(start_path),

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
    // Settings/desktop-settings writes go to disk on their own thread so a
    // slow filesystem cannot stall the owner loop; the owner keeps the
    // authoritative in-memory copy and only hands off the bytes to persist.
    let (persist_tx, persist_rx) = channel::<PersistMsg>();
    std::thread::Builder::new()
        .name("opusdm-persist".into())
        .spawn(move || {
            for msg in persist_rx {
                match msg {
                    PersistMsg::Settings(s) => {
                        if let Err(e) = settings::save_settings(&s) {
                            opusdm_ipc::err_log!("hub", "cannot save settings.json: {e}");
                        }
                    }
                    PersistMsg::Desktop(d) => {
                        if let Err(e) = desktop_settings::save(&d) {
                            opusdm_ipc::err_log!("hub", "cannot save desktop.json: {e}");
                        }
                    }
                    PersistMsg::Bindings(b) => {
                        if let Err(e) = bindings::save_bindings(&b) {
                            opusdm_ipc::err_log!("hub", "cannot save bindings: {e}");
                        }
                    }
                }
            }
            opusdm_ipc::debug_log!("hub", "persist thread exiting");
        })
        .expect("spawn persist thread");

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
        event_guard: EventGuard::new(),
        global_keys_enabled: opts.global_keys,
        owner_tx: owner_tx.clone(),
        global_keys: None,
        x11_failed: false,
        current_bindings: match opts.initial_bindings {
            Some(b) => b,
            None => bindings::sanitize(bindings::load_bindings()),
        },
        persist_tx,
        engine: JobEngine::new(default_runner(), jp_tx),
        jobs: HashMap::new(),
        replies: HashMap::new(),
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
            if let Ok(content) = std::fs::read_to_string(path) {
                return content;
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
}
