//! The process-wide Wayland connection that watches the compositor's
//! windows (`zwlr_foreign_toplevel_manager_v1`) and workspaces
//! (`ext_workspace_manager_v1`), and sends the requests about them.
//!
//! It is a connection of its own, separate from GTK's, served by a thread:
//! every change ends with a `done` event, after which the subscribers get a
//! fresh [`Snapshot`]. Requests are sent from any thread on the same
//! connection (Wayland proxies are thread safe) and flushed at once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use wayland_client::backend::ObjectId;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use crate::model::{apply_states, pager_order, parse_coordinates, stepped_index, Snapshot, Toplevel, ToplevelId, Workspace};

/// Highest `wl_output` version used: 4 brings the output name.
const OUTPUT_VERSION: u32 = 4;

/// A callback that receives every new snapshot, on the Wayland thread.
type Subscriber = Box<dyn Fn(&Snapshot) + Send>;

/// The live view of the compositor and the way to act on it.
pub struct Session {
    conn: Connection,
    inner: Mutex<Inner>,
    subscribers: Mutex<Vec<Subscriber>>,
}

/// The Wayland objects and what they last reported.
#[derive(Default)]
struct Inner {
    seat: Option<wl_seat::WlSeat>,
    has_toplevels: bool,
    toplevels: Vec<(ZwlrForeignToplevelHandleV1, Toplevel)>,
    next_id: ToplevelId,
    workspace_manager: Option<ExtWorkspaceManagerV1>,
    groups: Vec<ExtWorkspaceGroupHandleV1>,
    workspaces: Vec<(ExtWorkspaceHandleV1, Workspace)>,
    /// Output names by `wl_output` object.
    outputs: HashMap<ObjectId, String>,
}

/// The dispatch state of the Wayland thread.
struct State {
    session: Arc<Session>,
}

/// The session of this process, connected and started on first use. `None`
/// outside a Wayland session or when the compositor cannot be reached (the
/// cause is logged once).
pub fn shared() -> Option<Arc<Session>> {
    static SHARED: OnceLock<Option<Arc<Session>>> = OnceLock::new();
    SHARED
        .get_or_init(|| {
            if !opusdm_ipc::session::is_wayland() { return None; }
            match start() {
                Ok(session) => Some(session),
                Err(e) => {
                    opusdm_ipc::err_log!("opusdm-wayland", "cannot watch the compositor's windows: {e}");
                    None
                }
            }
        })
        .clone()
}

/// Connect, bind the globals, read the initial state and start the thread.
fn start() -> Result<Arc<Session>, Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let session = Arc::new(Session { conn: conn.clone(), inner: Mutex::new(Inner::default()), subscribers: Mutex::new(Vec::new()) });
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    {
        let mut inner = session.lock();
        inner.seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=1, ()).ok();
        inner.has_toplevels = globals.bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, ()).is_ok();
        inner.workspace_manager = globals.bind::<ExtWorkspaceManagerV1, _, _>(&qh, 1..=1, ()).ok();
        let registry = globals.registry();
        globals.contents().with_list(|list| {
            for global in list.iter().filter(|g| g.interface == wl_output::WlOutput::interface().name) {
                registry.bind::<wl_output::WlOutput, _, _>(global.name, global.version.min(OUTPUT_VERSION), &qh, ());
            }
        });
    }
    let mut state = State { session: session.clone() };
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    std::thread::Builder::new().name("opusdm-wayland".into()).spawn(move || loop {
        if let Err(e) = queue.blocking_dispatch(&mut state) {
            opusdm_ipc::err_log!("opusdm-wayland", "compositor connection lost: {e}");
            return;
        }
    })?;
    Ok(session)
}

impl Session {
    /// The inner state, even after a panic on another thread.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether the compositor lists the windows of other clients.
    pub fn has_toplevels(&self) -> bool {
        self.lock().has_toplevels
    }

    /// Whether the compositor reports its workspaces.
    pub fn has_workspaces(&self) -> bool {
        self.lock().workspace_manager.is_some()
    }

    /// What is known now.
    pub fn snapshot(&self) -> Snapshot {
        let inner = self.lock();
        Snapshot {
            toplevels: inner.toplevels.iter().map(|(_, t)| t.clone()).collect(),
            workspaces: pager_order(&inner.workspaces.iter().map(|(_, w)| w.clone()).collect::<Vec<_>>()),
        }
    }

    /// Call `f` with the current snapshot now and with every new one after,
    /// on the Wayland thread.
    pub fn subscribe(&self, f: impl Fn(&Snapshot) + Send + 'static) {
        f(&self.snapshot());
        self.subscribers.lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(f));
    }

    /// Hand the current snapshot to every subscriber.
    fn publish(&self) {
        let snapshot = self.snapshot();
        for f in self.subscribers.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            f(&snapshot);
        }
    }

    /// Run `f` on the handle of toplevel `id` and flush; unknown ids are
    /// ignored (the window just closed).
    fn with_toplevel(&self, id: ToplevelId, f: impl FnOnce(&ZwlrForeignToplevelHandleV1, Option<&wl_seat::WlSeat>)) {
        {
            let inner = self.lock();
            let Some((handle, _)) = inner.toplevels.iter().find(|(_, t)| t.id == id) else { return };
            f(handle, inner.seat.as_ref());
        }
        self.flush();
    }

    /// Send what is queued, logging a failure.
    fn flush(&self) {
        if let Err(e) = self.conn.flush() {
            opusdm_ipc::err_log!("opusdm-wayland", "cannot send a request to the compositor: {e}");
        }
    }

    /// Raise and focus window `id` (also restores it when minimized).
    pub fn activate(&self, id: ToplevelId) {
        self.with_toplevel(id, |handle, seat| match seat {
            Some(seat) => handle.activate(seat),
            None => opusdm_ipc::err_log!("opusdm-wayland", "no seat: cannot activate a window"),
        });
    }

    /// Minimize window `id`.
    pub fn minimize(&self, id: ToplevelId) {
        self.with_toplevel(id, |handle, _| handle.set_minimized());
    }

    /// Ask window `id` to close, like its close button.
    pub fn close(&self, id: ToplevelId) {
        self.with_toplevel(id, |handle, _| handle.close());
    }

    /// Switch to the workspace at `index` of [`Snapshot::workspaces`].
    pub fn activate_workspace(&self, index: usize) {
        {
            let inner = self.lock();
            let Some(manager) = &inner.workspace_manager else { return };
            let shown = pager_order(&inner.workspaces.iter().map(|(_, w)| w.clone()).collect::<Vec<_>>());
            let Some(target) = shown.get(index) else { return };
            let Some((handle, _)) = inner.workspaces.iter().find(|(_, w)| w == target) else { return };
            handle.activate();
            manager.commit();
        }
        self.flush();
    }

    /// Switch to the workspace `delta` steps from the current one. With
    /// `wrap` it goes around at both ends, without it stops at the first and
    /// the last workspace.
    pub fn switch_workspace(&self, delta: i32, wrap: bool) {
        let snapshot = self.snapshot();
        let current = snapshot.current_workspace().unwrap_or(0);
        if let Some(target) = stepped_index(current, snapshot.workspaces.len(), delta, wrap) {
            self.activate_workspace(target);
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    /// Bind outputs plugged in later.
    fn event(_: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface == wl_output::WlOutput::interface().name {
                registry.bind::<wl_output::WlOutput, _, _>(name, version.min(OUTPUT_VERSION), qh, ());
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    /// Nothing to read: the seat is only passed to `activate`.
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    /// Remember the name of each output.
    fn event(state: &mut Self, output: &wl_output::WlOutput, event: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = event {
            state.session.lock().outputs.insert(output.id(), name);
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    /// A new window: give it an id and wait for its `done`.
    fn event(state: &mut Self, _: &ZwlrForeignToplevelManagerV1, event: zwlr_foreign_toplevel_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                let mut inner = state.session.lock();
                inner.next_id += 1;
                let id = inner.next_id;
                inner.toplevels.push((toplevel, Toplevel { id, ..Toplevel::default() }));
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                let mut inner = state.session.lock();
                inner.has_toplevels = false;
                inner.toplevels.clear();
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    /// Fold one property into the window; publish on `done` and `closed`.
    fn event(state: &mut Self, handle: &ZwlrForeignToplevelHandleV1, event: zwlr_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        let publish = matches!(event, Event::Done | Event::Closed);
        {
            let mut inner = state.session.lock();
            let inner = &mut *inner;
            let Some(index) = inner.toplevels.iter().position(|(h, _)| h == handle) else { return };
            let toplevel = &mut inner.toplevels[index].1;
            match event {
                Event::Title { title } => toplevel.title = title,
                Event::AppId { app_id } => toplevel.app_id = app_id,
                Event::State { state } => apply_states(toplevel, &state),
                Event::OutputEnter { output } => {
                    if let Some(name) = inner.outputs.get(&output.id()) {
                        if !toplevel.outputs.contains(name) { toplevel.outputs.push(name.clone()); }
                    }
                }
                Event::OutputLeave { output } => {
                    if let Some(name) = inner.outputs.get(&output.id()) { toplevel.outputs.retain(|o| o != name); }
                }
                Event::Closed => {
                    let (handle, _) = inner.toplevels.remove(index);
                    handle.destroy();
                }
                _ => {}
            }
        }
        if publish { state.session.publish(); }
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for State {
    /// New groups and workspaces; publish on `done`.
    fn event(state: &mut Self, _: &ExtWorkspaceManagerV1, event: ext_workspace_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use ext_workspace_manager_v1::Event;
        match event {
            Event::WorkspaceGroup { workspace_group } => state.session.lock().groups.push(workspace_group),
            Event::Workspace { workspace } => state.session.lock().workspaces.push((workspace, Workspace::default())),
            Event::Done => state.session.publish(),
            Event::Finished => {
                let mut inner = state.session.lock();
                inner.workspace_manager = None;
                inner.workspaces.clear();
                inner.groups.clear();
            }
            _ => {}
        }
    }

    event_created_child!(State, ExtWorkspaceManagerV1, [
        ext_workspace_manager_v1::EVT_WORKSPACE_GROUP_OPCODE => (ExtWorkspaceGroupHandleV1, ()),
        ext_workspace_manager_v1::EVT_WORKSPACE_OPCODE => (ExtWorkspaceHandleV1, ()),
    ]);
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for State {
    /// Only removal matters: the pager lists workspaces, not groups.
    fn event(state: &mut Self, group: &ExtWorkspaceGroupHandleV1, event: ext_workspace_group_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_workspace_group_handle_v1::Event::Removed = event {
            state.session.lock().groups.retain(|g| g != group);
            group.destroy();
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, ()> for State {
    /// Fold one property into the workspace (published on the manager's
    /// `done`).
    fn event(state: &mut Self, handle: &ExtWorkspaceHandleV1, event: ext_workspace_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use ext_workspace_handle_v1::Event;
        let mut inner = state.session.lock();
        let Some(index) = inner.workspaces.iter().position(|(h, _)| h == handle) else { return };
        let workspace = &mut inner.workspaces[index].1;
        match event {
            Event::Name { name } => workspace.name = name,
            Event::Coordinates { coordinates } => workspace.coordinates = parse_coordinates(&coordinates),
            Event::State { state: WEnum::Value(flags) } => {
                workspace.active = flags.contains(ext_workspace_handle_v1::State::Active);
                workspace.hidden = flags.contains(ext_workspace_handle_v1::State::Hidden);
            }
            Event::Removed => {
                let (handle, _) = inner.workspaces.remove(index);
                handle.destroy();
            }
            _ => {}
        }
    }
}
