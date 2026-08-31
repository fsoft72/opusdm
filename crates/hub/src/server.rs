//! The socket server: an accept loop, one reader thread per Lister, and a
//! single owner loop that holds all mutable hub state. Only channels cross
//! thread boundaries.

use crate::jobs::{default_runner, map_progress, validate_job, JobEngine, ProgressTotals};
use crate::roles::{apply_role_event, RoleEffect, RoleEvent, RoleState};
use opusdm_fsops::Progress;
use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::{ListerId, Op, Outcome, Resolution, ToHub, ToLister};
use std::collections::HashMap;
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
        dest: Option<ListerId>,
        paths: Vec<(ListerId, PathBuf)>,
    },
    /// Progress for a running job.
    JobUpdate { job_id: u64, label: String, done: u64, total: u64 },
    /// A job finished.
    JobFinished { job_id: u64, ok: bool, errors: usize },
    /// No Listers remain open.
    AllListersGone,
    /// A redundant hub invocation asked us to raise the toolbar.
    RaiseToolbar,
}

/// Commands the GTK toolbar (or the SIGCHLD handler) sends to the server.
#[derive(Debug)]
pub enum HubCmd {
    /// Open another Lister.
    SpawnRequested { start_path: Option<PathBuf> },
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
        std::thread::spawn(move || {
            for cmd in cmd_rx {
                if owner_tx.send(OwnerMsg::FromGtk(cmd)).is_err() {
                    break;
                }
            }
        });
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
struct Conn {
    write: UnixStream,
    registered: bool,
    pid: u32,
    path: PathBuf,
}

/// Per-job bookkeeping the owner keeps until `Done`.
struct JobMeta {
    requester: u64,
    op: Op,
    totals: ProgressTotals,
    dest_dir: PathBuf,
    src_parents: Vec<PathBuf>,
}

fn owner_loop(
    rx: Receiver<OwnerMsg>,
    owner_tx: Sender<OwnerMsg>,
    evt_tx: Sender<HubEvent>,
    on_spawn: SpawnFn,
) {
    let mut conns: HashMap<u64, Conn> = HashMap::new();
    let mut next_conn: u64 = 1;
    let mut roles = RoleState::default();

    let (jp_tx, jp_rx) = channel::<(u64, Progress)>();
    {
        let owner_tx = owner_tx.clone();
        std::thread::spawn(move || {
            for (id, ev) in jp_rx {
                if owner_tx.send(OwnerMsg::JobProgress(id, ev)).is_err() {
                    break;
                }
            }
        });
    }
    let mut engine = JobEngine::new(default_runner(), jp_tx);
    let mut jobs: HashMap<u64, JobMeta> = HashMap::new();
    let mut replies: HashMap<u64, Sender<Resolution>> = HashMap::new();
    let mut next_req: u64 = 1;

    for msg in rx {
        match msg {
            OwnerMsg::Accepted(stream) => {
                let cid = next_conn;
                next_conn += 1;
                let Ok(write) = stream.try_clone() else { continue };
                conns.insert(
                    cid,
                    Conn { write, registered: false, pid: 0, path: PathBuf::new() },
                );
                let owner_tx = owner_tx.clone();
                std::thread::Builder::new()
                    .name(format!("opusdm-reader-{cid}"))
                    .spawn(move || reader_loop(cid, stream, owner_tx))
                    .expect("spawn reader thread");
            }

            OwnerMsg::FromLister(cid, ToHub::Ping) => {
                if let Some(c) = conns.get_mut(&cid) {
                    let _ = send(&mut c.write, &ToLister::Pong);
                }
            }

            OwnerMsg::FromLister(_cid, ToHub::RaiseToolbar) => {
                let _ = evt_tx.send(HubEvent::RaiseToolbar);
            }

            OwnerMsg::FromLister(cid, ToHub::Register { pid, initial_path }) => {
                if let Some(c) = conns.get_mut(&cid) {
                    c.registered = true;
                    c.pid = pid;
                    c.path = initial_path.clone();
                    let _ = send(
                        &mut c.write,
                        &ToLister::Welcome {
                            lister_id: ListerId(cid),
                            source: roles.source,
                            dest: roles.dest,
                        },
                    );
                    let _ = evt_tx.send(HubEvent::ListerRegistered {
                        id: ListerId(cid),
                        pid,
                        path: initial_path,
                    });
                    emit_roles(&roles, &conns, &evt_tx);
                }
            }

            OwnerMsg::FromLister(cid, ToHub::Goodbye) | OwnerMsg::ListerEof(cid) => {
                drop_conn(cid, &mut conns, &mut roles, &evt_tx);
            }

            OwnerMsg::FromLister(cid, ToHub::FocusGained) => {
                apply_role(&mut roles, RoleEvent::FocusGained(ListerId(cid)), &mut conns, &evt_tx);
            }

            OwnerMsg::FromLister(cid, ToHub::SetRole(role)) => {
                apply_role(&mut roles, RoleEvent::SetRole(ListerId(cid), role), &mut conns, &evt_tx);
            }

            OwnerMsg::FromLister(cid, ToHub::LockRole) => {
                apply_role(&mut roles, RoleEvent::Lock(ListerId(cid)), &mut conns, &evt_tx);
            }

            OwnerMsg::FromLister(cid, ToHub::UnlockRole) => {
                apply_role(&mut roles, RoleEvent::Unlock(ListerId(cid)), &mut conns, &evt_tx);
            }

            OwnerMsg::FromLister(cid, ToHub::PathChanged(p)) => {
                if let Some(c) = conns.get_mut(&cid) {
                    c.path = p;
                }
                emit_roles(&roles, &conns, &evt_tx);
            }

            OwnerMsg::FromLister(cid, ToHub::SpawnLister { start_path }) => {
                let _ = cid;
                on_spawn(start_path);
            }

            OwnerMsg::FromLister(cid, ToHub::RunJob { op, srcs, dest_dir }) => {
                let needs_validation = matches!(op, Op::Copy | Op::Move);
                if needs_validation {
                    if let Err(msg) = validate_job(&srcs, &dest_dir) {
                        if let Some(c) = conns.get_mut(&cid) {
                            let _ = send(
                                &mut c.write,
                                &ToLister::JobDone {
                                    job_id: 0,
                                    outcome: Outcome::Error { message: msg },
                                    errors: Vec::new(),
                                },
                            );
                        }
                        continue;
                    }
                }
                let src_parents: Vec<PathBuf> = srcs
                    .iter()
                    .filter_map(|s| s.parent().map(PathBuf::from))
                    .collect();
                let spec = opusdm_fsops::JobSpec { op: op.clone(), srcs, dest_dir: dest_dir.clone() };
                let out = engine.submit(ListerId(cid), spec);
                jobs.insert(
                    out.job_id,
                    JobMeta {
                        requester: cid,
                        op,
                        totals: ProgressTotals::default(),
                        dest_dir,
                        src_parents,
                    },
                );
            }

            OwnerMsg::FromLister(_cid, ToHub::ConflictReply { req_id, resolution }) => {
                if let Some(tx) = replies.remove(&req_id) {
                    let _ = tx.send(resolution);
                }
            }

            OwnerMsg::JobProgress(job_id, prog) => {
                handle_job_progress(
                    job_id,
                    prog,
                    &mut jobs,
                    &mut conns,
                    &mut engine,
                    &mut replies,
                    &mut next_req,
                    &evt_tx,
                );
            }

            OwnerMsg::FromGtk(HubCmd::SpawnRequested { start_path }) => on_spawn(start_path),

            OwnerMsg::FromGtk(HubCmd::CancelJob(id)) => engine.cancel(id),

            OwnerMsg::FromGtk(HubCmd::PresentLister(id)) => {
                if let Some(c) = conns.get_mut(&id.0) {
                    let _ = send(&mut c.write, &ToLister::Present);
                }
            }

            OwnerMsg::FromGtk(HubCmd::Shutdown) => {
                for c in conns.values_mut().filter(|c| c.registered) {
                    let _ = send(&mut c.write, &ToLister::Shutdown);
                }
                return;
            }

            OwnerMsg::FromGtk(HubCmd::ChildExited(pid)) => {
                let hit = conns.iter().find(|(_, c)| c.pid == pid).map(|(k, _)| *k);
                if let Some(cid) = hit {
                    drop_conn(cid, &mut conns, &mut roles, &evt_tx);
                }
            }
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
    roles: &mut RoleState,
    evt_tx: &Sender<HubEvent>,
) {
    let Some(c) = conns.remove(&cid) else { return };
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
                    let _ = send(
                        &mut c.write,
                        &ToLister::RoleChanged { source: roles.source, dest: roles.dest },
                    );
                }
                emit_roles(roles, conns, evt_tx);
            }
            RoleEffect::Reject(id) => {
                if let Some(c) = conns.get_mut(&id.0) {
                    let _ = send(
                        &mut c.write,
                        &ToLister::RoleChanged { source: roles.source, dest: roles.dest },
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
    let _ = evt_tx.send(HubEvent::Roles { source: roles.source, dest: roles.dest, paths });
}

#[allow(clippy::too_many_arguments)]
fn handle_job_progress(
    job_id: u64,
    prog: Progress,
    jobs: &mut HashMap<u64, JobMeta>,
    conns: &mut HashMap<u64, Conn>,
    engine: &mut JobEngine,
    replies: &mut HashMap<u64, Sender<Resolution>>,
    next_req: &mut u64,
    evt_tx: &Sender<HubEvent>,
) {
    let Some(meta) = jobs.get_mut(&job_id) else { return };
    let requester = meta.requester;

    match prog {
        Progress::Conflict { path, src, dst, reply } => {
            let req_id = *next_req;
            *next_req += 1;
            replies.insert(req_id, reply);
            if let Some(c) = conns.get_mut(&requester) {
                let _ = send(
                    &mut c.write,
                    &ToLister::ConflictPrompt { req_id, job_id, path, src, dst },
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
            if let Some(c) = conns.get_mut(&requester) {
                let _ = send(
                    &mut c.write,
                    &ToLister::JobDone { job_id, outcome, errors },
                );
            }
            let _ = evt_tx.send(HubEvent::JobFinished {
                job_id,
                ok: !cancelled && err_count == 0,
                errors: err_count,
            });
            // Refresh every Lister showing an affected directory.
            let mut affected = vec![meta.dest_dir.clone()];
            if matches!(meta.op, Op::Move | Op::Delete) {
                affected.extend(meta.src_parents.iter().cloned());
            }
            for dir in affected {
                for c in conns.values_mut().filter(|c| c.registered) {
                    let _ = send(&mut c.write, &ToLister::InvalidatePath(dir.clone()));
                }
            }
            engine.on_done(job_id);
            jobs.remove(&job_id);
        }
        other => {
            let op = meta.op.clone();
            if let Some(msg) = map_progress(job_id, &op, &other, &mut meta.totals) {
                if let ToLister::JobProgress { bytes_done, bytes_total, .. } = &msg {
                    let _ = evt_tx.send(HubEvent::JobUpdate {
                        job_id,
                        label: format!("{op:?}"),
                        done: *bytes_done,
                        total: *bytes_total,
                    });
                }
                if let Some(c) = conns.get_mut(&requester) {
                    let _ = send(&mut c.write, &msg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::frame::{recv, send};
    use std::time::Duration;

    fn bind_temp() -> (tempfile::TempDir, UnixListener, PathBuf) {
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
        let handle = spawn_server(listener, Box::new(|_p| {}));

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
        let _wa: ToLister = recv(&mut a).unwrap().unwrap();
        send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into() }).unwrap();
        let _wb: ToLister = recv(&mut b).unwrap().unwrap();

        send(&mut a, &ToHub::FocusGained).unwrap();
        let _ra: ToLister = recv(&mut a).unwrap().unwrap();
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
