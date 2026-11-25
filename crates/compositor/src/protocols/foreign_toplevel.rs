//! wlr-foreign-toplevel-management (`zwlr_foreign_toplevel_manager_v1`,
//! version 3): the list of windows the panel tasklist, the fullscreen
//! detection and the tray start read, and the requests they send
//! (activate, minimize, maximize, fullscreen, close).
//!
//! Every bound manager gets one handle per window the IPC lists
//! (`State::window_views`: every mapped toplevel, shown or hidden), so the
//! two never disagree. `foreign_toplevels_apply` brings the handles in line
//! with those views: it closes the handles of the windows that went,
//! creates the handles of the new ones and sends what changed + `done`. It
//! runs from `notify_watchers` (after every client dispatch, IPC request,
//! window-manager operation and frame) and right at the xdg state changes
//! (map, unmap, destroy, maximize / fullscreen, title, app id).
//!
//! Ownership: `ForeignToplevels` lives in `State` (owned by `LoopData`,
//! owned by the backend `run` for the whole run). It holds the manager and
//! handle resources only to send events on them; the client owns the
//! objects. A handle leaves the table when it is closed (its window went)
//! or destroyed (the client destroyed it, or the client died: wayland-server
//! calls `destroyed` for every object of a dead client); a manager leaves it
//! on `stop` or when it is destroyed. Dead resources are also dropped on
//! every apply, so nothing is kept for a gone client.

use smithay::{
    reexports::{
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
            zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
        },
        wayland_server::{
            backend::{ClientId, GlobalId, ObjectId},
            protocol::wl_output::WlOutput,
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    wayland::shell::xdg::XdgShellHandler,
};

use crate::ipc::protocol::WindowView;
use crate::state::{State, LOG_TAG};
use crate::wm::WindowId;

/// Version of the `zwlr_foreign_toplevel_manager_v1` global.
pub const FOREIGN_TOPLEVEL_VERSION: u32 = 3;

/// `state` value: the window is maximized.
const STATE_MAXIMIZED: u32 = 0;

/// `state` value: the window is minimized.
const STATE_MINIMIZED: u32 = 1;

/// `state` value: the window is activated (it has the keyboard).
const STATE_ACTIVATED: u32 = 2;

/// `state` value: the window is fullscreen (handle version 2 and later).
const STATE_FULLSCREEN: u32 = 3;

/// First handle version that knows the `fullscreen` state.
const FULLSCREEN_SINCE: u32 = 2;

/// One handle of one manager for one window, with what it was last told.
#[derive(Debug)]
struct Handle {
    /// The window the handle stands for.
    id: WindowId,
    /// Id of the manager that announced the handle.
    manager: ObjectId,
    /// The handle resource.
    handle: ZwlrForeignToplevelHandleV1,
    /// Title last sent (`None` before the first one).
    title: Option<String>,
    /// App id last sent (`None` before the first one).
    app_id: Option<String>,
    /// States last sent (`None` before the first one).
    states: Option<Vec<u32>>,
    /// Outputs the handle entered.
    outputs: Vec<WlOutput>,
}

/// The foreign-toplevel global, its bound managers and their handles.
#[derive(Debug)]
pub struct ForeignToplevels {
    /// The global, kept for the whole run.
    _global: GlobalId,
    /// Managers bound and not stopped.
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    /// Handles not closed nor destroyed.
    handles: Vec<Handle>,
}

impl ForeignToplevels {
    /// Creates the `zwlr_foreign_toplevel_manager_v1` global on `display`.
    pub fn new(display: &DisplayHandle) -> ForeignToplevels {
        let global = display.create_global::<State, ZwlrForeignToplevelManagerV1, ()>(FOREIGN_TOPLEVEL_VERSION, ());
        ForeignToplevels { _global: global, managers: Vec::new(), handles: Vec::new() }
    }

    /// True when a manager or a handle exists: only then the views are worth computing.
    pub fn is_active(&self) -> bool {
        !self.managers.is_empty() || !self.handles.is_empty()
    }

    /// Number of managers and of handles kept (the `debug/foreign-toplevels` IPC request).
    pub fn counts(&self) -> (usize, usize) {
        (self.managers.len(), self.handles.len())
    }

    /// True when `resource` is a handle that is neither closed nor destroyed.
    fn is_live_handle(&self, resource: &ZwlrForeignToplevelHandleV1) -> bool {
        self.handles.iter().any(|h| &h.handle == resource)
    }
}

impl State {
    /// Brings the foreign-toplevel handles in line with the windows (see
    /// the module documentation); cheap when no client bound the manager.
    pub fn foreign_toplevels_sync(&mut self) {
        if !self.foreign_toplevels.is_active() { return; }
        let views = self.window_views();
        self.foreign_toplevels_apply(&views);
    }

    /// Sends the changed fields of the window `id` (title, app id, states,
    /// outputs) + `done` to each of its handles. A window that is not
    /// listed (gone, or not mapped) is left to `foreign_toplevels_sync`,
    /// which closes its handles.
    pub fn foreign_toplevel_update(&mut self, id: WindowId) {
        if !self.foreign_toplevels.is_active() { return; }
        let Some(view) = self.window_views().into_iter().find(|v| v.id == id.0) else {
            opusdm_ipc::debug_log!(LOG_TAG, "foreign toplevel: window {} is not listed, nothing to update", id.0);
            return;
        };
        self._foreign_toplevel_send(&view);
    }

    /// Brings the handles in line with `views` (the windows `window_views`
    /// lists, in stack order): drops dead managers and handles, closes the
    /// handles of the windows that are no longer listed, announces every
    /// listed window to the managers that have no handle for it yet, and
    /// sends what changed + `done`.
    pub fn foreign_toplevels_apply(&mut self, views: &[WindowView]) {
        let ft = &mut self.foreign_toplevels;
        ft.managers.retain(|m| m.is_alive());
        ft.handles.retain(|h| h.handle.is_alive());
        ft.handles.retain(|h| {
            if views.iter().any(|v| v.id == h.id.0) { return true; }
            h.handle.closed();
            opusdm_ipc::debug_log!(LOG_TAG, "foreign toplevel: window {} closed on handle {}", h.id.0, h.handle.id());
            false
        });
        for manager in self.foreign_toplevels.managers.clone() {
            for view in views {
                let id = WindowId(view.id);
                let known = self.foreign_toplevels.handles.iter().any(|h| h.id == id && h.manager == manager.id());
                if known { continue; }
                self._foreign_toplevel_announce(&manager, id);
            }
        }
        for view in views {
            self._foreign_toplevel_send(view);
        }
    }

    /// Creates a handle of `manager` for the window `id` and sends
    /// `toplevel`; its fields go out with the next `_foreign_toplevel_send`.
    fn _foreign_toplevel_announce(&mut self, manager: &ZwlrForeignToplevelManagerV1, id: WindowId) {
        let client = match self.display_handle.get_client(manager.id()) {
            Ok(client) => client,
            Err(e) => {
                opusdm_ipc::err_log!(LOG_TAG, "foreign toplevel: manager {} has no client ({e}), window {} not announced", manager.id(), id.0);
                return;
            }
        };
        let created = client.create_resource::<ZwlrForeignToplevelHandleV1, WindowId, State>(&self.display_handle, manager.version(), id);
        let handle = match created {
            Ok(handle) => handle,
            Err(e) => {
                opusdm_ipc::err_log!(LOG_TAG, "foreign toplevel: no handle for window {} on manager {}: {e}", id.0, manager.id());
                return;
            }
        };
        manager.toplevel(&handle);
        opusdm_ipc::debug_log!(LOG_TAG, "foreign toplevel: window {} announced as {}", id.0, handle.id());
        self.foreign_toplevels.handles.push(Handle {
            id,
            manager: manager.id(),
            handle,
            title: None,
            app_id: None,
            states: None,
            outputs: Vec::new(),
        });
    }

    /// Sends to every handle of the window of `view` the fields that differ
    /// from what it was last told, and `done` when anything was sent. Every
    /// `wl_output` the client of the handle has bound is entered (one output:
    /// every window is on it, shown or hidden), also one bound after the
    /// handle was announced.
    fn _foreign_toplevel_send(&mut self, view: &WindowView) {
        let output = &self.output;
        let dh = &self.display_handle;
        for h in self.foreign_toplevels.handles.iter_mut().filter(|h| h.id.0 == view.id) {
            let mut changed = false;
            if h.title.as_deref() != Some(view.title.as_str()) {
                h.handle.title(view.title.clone());
                h.title = Some(view.title.clone());
                changed = true;
            }
            if h.app_id.as_deref() != Some(view.app_id.as_str()) {
                h.handle.app_id(view.app_id.clone());
                h.app_id = Some(view.app_id.clone());
                changed = true;
            }
            let states = _states(view, h.handle.version());
            if h.states.as_ref() != Some(&states) {
                h.handle.state(states.iter().flat_map(|s| s.to_ne_bytes()).collect());
                h.states = Some(states);
                changed = true;
            }
            h.outputs.retain(|o| o.is_alive());
            match dh.get_client(h.handle.id()) {
                Ok(client) => {
                    for o in output.client_outputs(&client) {
                        if h.outputs.contains(&o) { continue; }
                        h.handle.output_enter(&o);
                        h.outputs.push(o);
                        changed = true;
                    }
                }
                Err(e) => opusdm_ipc::err_log!(LOG_TAG, "foreign toplevel: handle {} has no client ({e}), outputs not sent", h.handle.id()),
            }
            if changed {
                h.handle.done();
            }
        }
    }
}

/// The `state` values of `view` for a handle of `version` (no fullscreen
/// before version 2).
fn _states(view: &WindowView, version: u32) -> Vec<u32> {
    let mut states = Vec::new();
    if view.maximized { states.push(STATE_MAXIMIZED); }
    if view.minimized { states.push(STATE_MINIMIZED); }
    if view.activated { states.push(STATE_ACTIVATED); }
    if view.fullscreen && version >= FULLSCREEN_SINCE { states.push(STATE_FULLSCREEN); }
    states
}

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    /// A client bound the manager: it is told about every listed window
    /// right away (before any later reply, such as a roundtrip `done`).
    fn bind(
        state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        let manager = data_init.init(resource, ());
        opusdm_ipc::debug_log!(LOG_TAG, "foreign toplevel: manager {} bound (version {})", manager.id(), manager.version());
        state.foreign_toplevels.managers.push(manager);
        state.foreign_toplevels_sync();
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    /// `stop`: no new windows are announced to this manager; `finished`
    /// ends it (its handles keep working until closed).
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            zwlr_foreign_toplevel_manager_v1::Request::Stop => {
                state.foreign_toplevels.managers.retain(|m| m != resource);
                resource.finished();
                opusdm_ipc::debug_log!(LOG_TAG, "foreign toplevel: manager {} stopped", resource.id());
            }
            other => opusdm_ipc::err_log!(LOG_TAG, "foreign toplevel: unknown manager request {other:?} ignored"),
        }
    }

    /// The manager is gone (finished, or its client died): it leaves the table.
    fn destroyed(state: &mut State, _client: ClientId, resource: &ZwlrForeignToplevelManagerV1, _data: &()) {
        state.foreign_toplevels.managers.retain(|m| m != resource);
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, WindowId> for State {
    /// Runs a request of a handle on its window: `activate` and
    /// `unset_minimized` activate it (as Openbox), `set_minimized`
    /// minimizes it, `close` asks it to close, maximize / fullscreen go
    /// through the xdg request paths. `set_rectangle` is not used (no
    /// minimize animation). A request on a closed handle or for a window
    /// that is gone is ignored with a debug line.
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        data: &WindowId,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Request;
        let id = *data;
        if let Request::Destroy = request { return; }
        if let Request::SetRectangle { .. } = request {
            opusdm_ipc::debug_log!(LOG_TAG, "foreign toplevel: set_rectangle of window {} ignored", id.0);
            return;
        }
        if !state.foreign_toplevels.is_live_handle(resource) {
            opusdm_ipc::debug_log!(LOG_TAG, "foreign toplevel: {request:?} on closed handle {} (window {}) ignored", resource.id(), id.0);
            return;
        }
        let Some(toplevel) = state.window_by_id(id).and_then(|(_, info)| info.window.toplevel().cloned()) else {
            opusdm_ipc::debug_log!(LOG_TAG, "foreign toplevel: {request:?} for window {}, which is gone, ignored", id.0);
            return;
        };
        let result = match request {
            Request::Activate { .. } | Request::UnsetMinimized => state.activate_window(id),
            Request::SetMinimized => state.set_minimized(id, true),
            Request::Close => state.close_window(id),
            Request::SetMaximized => {
                state.maximize_request(toplevel);
                Ok(())
            }
            Request::UnsetMaximized => {
                state.unmaximize_request(toplevel);
                Ok(())
            }
            Request::SetFullscreen { output } => {
                state.fullscreen_request(toplevel, output);
                Ok(())
            }
            Request::UnsetFullscreen => {
                state.unfullscreen_request(toplevel);
                Ok(())
            }
            other => Err(format!("unknown request {other:?}")),
        };
        if let Err(e) = result {
            opusdm_ipc::err_log!(LOG_TAG, "foreign toplevel: window {}: {e}", id.0);
        }
    }

    /// The handle is gone (destroyed by its client, or the client died): it
    /// leaves the table.
    fn destroyed(state: &mut State, _client: ClientId, resource: &ZwlrForeignToplevelHandleV1, _data: &WindowId) {
        state.foreign_toplevels.handles.retain(|h| &h.handle != resource);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    /// A view of window 1 with the given fill / focus states.
    fn view(maximized: bool, minimized: bool, activated: bool, fullscreen: bool) -> WindowView {
        WindowView {
            id: 1,
            app_id: String::new(),
            title: String::new(),
            pid: None,
            geometry: Rect::default(),
            desktop: 0,
            minimized,
            maximized,
            fullscreen,
            activated,
        }
    }

    #[test]
    fn states_use_the_protocol_values_and_hide_fullscreen_from_version_1() {
        assert_eq!(_states(&view(true, true, true, true), 3), vec![0, 1, 2, 3]);
        assert_eq!(_states(&view(false, false, false, true), 1), Vec::<u32>::new());
        assert_eq!(_states(&view(false, false, true, false), 1), vec![2]);
    }
}
