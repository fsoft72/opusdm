//! The socket server: an accept loop, one reader thread per Lister, and a
//! single owner loop that holds all mutable hub state. Only channels cross
//! thread boundaries.

use crate::jobs::{default_runner, map_progress, validate_job, JobEngine, ProgressTotals};
use crate::roles::{apply_role_event, RoleEffect, RoleEvent, RoleState};
use crate::settings;
use crate::desktop_settings;
use opusdm_fsops::Progress;
use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::{DesktopSettings, ListerId, Op, Outcome, Resolution, ToHub, ToLister, Welcome};
use std::collections::{BTreeSet, HashMap};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Called when a Lister or the toolbar asks for a new Lister window.
pub type SpawnFn = Box<dyn Fn(Option<PathBuf>) + Send>;

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
    /// Theme changed (from `opusdm-theme`). Carries the new theme; the GTK
    /// side repaints the desktop background and re-renders desktop icons.
    ThemeChanged(opusdm_ipc::Theme),
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

/// Start the socket server on `listener`. Returns channels to/from the GTK side.
pub fn spawn_server(listener: UnixListener, on_spawn: SpawnFn) -> ServerHandle {
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
        .spawn(move || owner_loop(owner_rx, owner_tx, evt_tx, on_spawn))
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
    persist_tx: Sender<PersistMsg>,
    engine: JobEngine,
    jobs: HashMap<u64, JobMeta>,
    replies: HashMap<u64, Sender<Resolution>>,
    next_req: u64,
    evt_tx: Sender<HubEvent>,
    on_spawn: SpawnFn,
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
            eprintln!("opusdm-hub: cannot spawn writer thread for conn {cid}: {e}");
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
            eprintln!("opusdm-hub: cannot spawn reader thread for conn {cid}: {e}");
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

            ToHub::RunJob { op, srcs, dest_dir } => {
                self.lister_run_job(cid, op, srcs, dest_dir);
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
                drop_conn(cid, &mut self.conns, &mut self.free_conns, &mut self.roles, &self.evt_tx);
            }
        }
    }

    /// `ToHub::RunJob` from a Lister: resolve the destinations (a `None`
    /// destination fans out from the SOURCE to every DEST Lister's current
    /// directory), validate, and submit each job to the engine.
    fn lister_run_job(&mut self, cid: u64, op: Op, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf>) {
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
            if matches!(op, Op::Copy | Op::Move) {
                if let Err(msg) = validate_job(&srcs, &dest_dir) {
                    fail(&mut self.conns, msg);
                    continue;
                }
            }
            let spec = opusdm_fsops::JobSpec {
                op: op.clone(),
                srcs: srcs.clone(),
                dest_dir: dest_dir.clone(),
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
                const GTK_REQUESTER: u64 = u64::MAX;
                if matches!(op, Op::Copy | Op::Move) {
                    if let Err(msg) = validate_job(&srcs, &dest_dir) {
                        eprintln!("opusdm-hub: desktop job rejected: {msg}");
                        return true;
                    }
                }
                let src_parents: Vec<PathBuf> =
                    srcs.iter().filter_map(|s| s.parent().map(PathBuf::from)).collect();
                let spec = opusdm_fsops::JobSpec {
                    op: op.clone(),
                    srcs: srcs.clone(),
                    dest_dir: dest_dir.clone(),
                };
                let out = self.engine.submit(ListerId(GTK_REQUESTER), spec);
                self.jobs.insert(
                    out.job_id,
                    JobMeta {
                        requester: GTK_REQUESTER,
                        op,
                        totals: ProgressTotals::default(),
                        dest_dir,
                        src_parents,
                    },
                );
            }

            HubCmd::CancelJob(id) => self.engine.cancel(id),

            HubCmd::PresentLister(id) => {
                if let Some(c) = self.conns.get_mut(&id.0) {
                    let _ = c.tx.send(ToLister::Present.into());
                }
            }

            HubCmd::Shutdown => {
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::Shutdown.into());
                }
                return false;
            }

            HubCmd::ChildExited(pid) => {
                let hit = self.conns.iter().find(|(_, c)| c.pid == pid).map(|(k, _)| *k);
                if let Some(cid) = hit {
                    drop_conn(cid, &mut self.conns, &mut self.free_conns, &mut self.roles, &self.evt_tx);
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
                let mut affected = vec![meta.dest_dir.clone()];
                if matches!(meta.op, Op::Move | Op::Delete) {
                    affected.extend(meta.src_parents.iter().cloned());
                }
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
                            eprintln!("opusdm-hub: cannot save settings.json: {e}");
                        }
                    }
                    PersistMsg::Desktop(d) => {
                        if let Err(e) = desktop_settings::save(&d) {
                            eprintln!("opusdm-hub: cannot save desktop.json: {e}");
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
        persist_tx,
        engine: JobEngine::new(default_runner(), jp_tx),
        jobs: HashMap::new(),
        replies: HashMap::new(),
        next_req: 1,
        evt_tx,
        on_spawn,
    };

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
                drop_conn(cid, &mut hub.conns, &mut hub.free_conns, &mut hub.roles, &hub.evt_tx);
                true
            }
            OwnerMsg::JobProgress(job_id, prog) => {
                hub.job_progress(job_id, prog);
                true
            }
            OwnerMsg::FromGtk(cmd) => hub.gtk_cmd(cmd),
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
) {
    let Some(c) = conns.remove(&cid) else { return };
    free_conns.insert(cid);
    let _ = c.shutdown.shutdown(std::net::Shutdown::Both);
    if !c.registered {
        return;
    }
    apply_role(roles, RoleEvent::Dropped(ListerId(cid)), conns, evt_tx);
    let _ = evt_tx.send(HubEvent::ListerGone(ListerId(cid)));
    if !conns.values().any(|c| c.registered) {
        let _ = evt_tx.send(HubEvent::AllListersGone);
    }
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
}
