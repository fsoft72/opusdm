//! ext-workspace (`ext_workspace_manager_v1`, version 1): the virtual
//! desktops as workspaces, for the OpusDM pager (`crates/wayland`).
//!
//! Every bound manager gets one workspace group (no capabilities, entered
//! on every `wl_output` the client binds) and one workspace per desktop:
//! id `"opusdm-desktop-<index>"`, name `"1"`..`"N"`, coordinates `[index]`
//! (one u32 in the native byte order of the wire, little-endian on the
//! machines OpusDM runs on), capabilities `activate`, state `active` on the
//! current desktop. `activate` is recorded and applied on the manager's
//! `commit` (`State::switch_desktop`); `deactivate`, `assign`, `remove` and
//! `create_workspace` are ignored with a debug line.
//!
//! `State::workspaces_update` brings every manager in line with
//! `State::desktops`: new desktops are announced and entered in the group,
//! removed ones leave the group and get `removed`, the active state follows
//! the current desktop, and `done` closes each batch. It runs from
//! `notify_watchers` (after every client dispatch, IPC request,
//! window-manager operation and frame), so every desktop change reaches the
//! clients; it costs one comparison per manager when nothing changed.
//!
//! Ownership: `Workspaces` lives in `State` (owned by `LoopData`, owned by
//! the backend `run` for the whole run). It holds the manager, group and
//! workspace resources only to send events on them; the client owns the
//! objects. A manager entry (with its group and workspaces) leaves the
//! table on `stop` or when the manager is destroyed; a workspace leaves it
//! when its desktop is removed or the client destroys it; a group is
//! forgotten when the client destroys it. When the client dies,
//! wayland-server calls `destroyed` for every object of it, and every
//! update also drops dead resources, so nothing is kept for a gone client.

use smithay::reexports::{
    wayland_protocols::ext::workspace::v1::server::{
        ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1, GroupCapabilities},
        ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1, WorkspaceCapabilities},
        ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
    },
    wayland_server::{
        backend::{ClientId, GlobalId},
        protocol::wl_output::WlOutput,
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
    },
};

use crate::state::{State, LOG_TAG};

/// Version of the `ext_workspace_manager_v1` global.
pub const WORKSPACE_VERSION: u32 = 1;

/// Prefix of the stable workspace id; the desktop index follows.
const WORKSPACE_ID_PREFIX: &str = "opusdm-desktop-";

/// One workspace of one manager.
#[derive(Debug)]
struct Workspace {
    /// 0-based index of the desktop it stands for.
    index: u32,
    /// The workspace resource.
    handle: ExtWorkspaceHandleV1,
    /// The `active` bit last sent.
    active: bool,
}

/// One bound manager, its group and its workspaces.
#[derive(Debug)]
struct Manager {
    /// The manager resource.
    manager: ExtWorkspaceManagerV1,
    /// The workspace group (`None` once the client destroyed it).
    group: Option<ExtWorkspaceGroupHandleV1>,
    /// Outputs the group entered.
    outputs: Vec<WlOutput>,
    /// Workspaces announced and neither removed nor destroyed.
    workspaces: Vec<Workspace>,
    /// Desktops announced so far (the next new workspace has this index).
    announced: u32,
    /// Desktop asked by `activate` since the last `commit`.
    pending_activate: Option<u32>,
}

/// The ext-workspace global and its bound managers.
#[derive(Debug)]
pub struct Workspaces {
    /// The global, kept for the whole run.
    _global: GlobalId,
    /// Managers bound and not stopped.
    managers: Vec<Manager>,
}

impl Workspaces {
    /// Creates the `ext_workspace_manager_v1` global on `display`.
    pub fn new(display: &DisplayHandle) -> Workspaces {
        let global = display.create_global::<State, ExtWorkspaceManagerV1, ()>(WORKSPACE_VERSION, ());
        Workspaces { _global: global, managers: Vec::new() }
    }

    /// Number of managers and of workspaces kept (the `debug/workspaces` IPC request).
    pub fn counts(&self) -> (usize, usize) {
        (self.managers.len(), self.managers.iter().map(|m| m.workspaces.len()).sum())
    }

    /// The manager entry that announced the workspace `handle`.
    fn _manager_of_workspace(&mut self, handle: &ExtWorkspaceHandleV1) -> Option<&mut Manager> {
        self.managers.iter_mut().find(|m| m.workspaces.iter().any(|w| &w.handle == handle))
    }
}

impl State {
    /// Brings every workspace manager in line with the desktops (see the
    /// module documentation); cheap when no client bound the manager.
    pub fn workspaces_update(&mut self) {
        if self.workspaces.managers.is_empty() { return; }
        let count = self.desktops.count();
        let current = self.desktops.current();
        self.workspaces.managers.retain(|m| m.manager.is_alive());
        for i in 0..self.workspaces.managers.len() {
            self._workspaces_update_manager(i, count, current);
        }
    }

    /// Brings the manager at `i` in line with `count` desktops, `current`
    /// shown, and the outputs its client bound; `done` when anything was sent.
    fn _workspaces_update_manager(&mut self, i: usize, count: u32, current: u32) {
        let client = match self.workspaces.managers.get(i).map(|m| self.display_handle.get_client(m.manager.id())) {
            Some(Ok(client)) => client,
            Some(Err(e)) => {
                opusdm_ipc::err_log!(LOG_TAG, "workspace: manager has no client ({e}), not updated");
                return;
            }
            None => return,
        };
        let client_outputs: Vec<WlOutput> = self.output.client_outputs(&client).collect();
        let dh = self.display_handle.clone();
        let Some(m) = self.workspaces.managers.get_mut(i) else { return };
        let mut changed = false;
        m.workspaces.retain(|w| w.handle.is_alive());
        m.outputs.retain(|o| o.is_alive());
        if let Some(group) = m.group.as_ref().filter(|g| g.is_alive()) {
            for o in client_outputs {
                if m.outputs.contains(&o) { continue; }
                group.output_enter(&o);
                m.outputs.push(o);
                changed = true;
            }
        }
        let removed: Vec<Workspace> = m.workspaces.extract_if(.., |w| w.index >= count).collect();
        for w in removed {
            if let Some(group) = &m.group {
                group.workspace_leave(&w.handle);
            }
            w.handle.removed();
            opusdm_ipc::debug_log!(LOG_TAG, "workspace: desktop {} removed on {}", w.index, Resource::id(&w.handle));
            changed = true;
        }
        m.announced = m.announced.min(count);
        while m.announced < count {
            let index = m.announced;
            m.announced += 1;
            if _announce(&dh, &client, m, index, index == current) {
                changed = true;
            }
        }
        for w in m.workspaces.iter_mut() {
            let active = w.index == current;
            if w.active == active { continue; }
            w.handle.state(_state(active));
            w.active = active;
            changed = true;
        }
        if changed {
            m.manager.done();
        }
    }
}

/// Creates the workspace of desktop `index` for the manager `m` (of
/// `client`), sends `workspace` and its fields, and enters it in the group.
/// False, with an error line, when the resource cannot be created.
fn _announce(dh: &DisplayHandle, client: &Client, m: &mut Manager, index: u32, active: bool) -> bool {
    let created = client.create_resource::<ExtWorkspaceHandleV1, (), State>(dh, m.manager.version(), ());
    let handle = match created {
        Ok(handle) => handle,
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "workspace: no workspace for desktop {index} on manager {}: {e}", m.manager.id());
            return false;
        }
    };
    m.manager.workspace(&handle);
    handle.id(format!("{WORKSPACE_ID_PREFIX}{index}"));
    handle.name((index + 1).to_string());
    handle.coordinates(index.to_ne_bytes().to_vec());
    handle.capabilities(WorkspaceCapabilities::Activate);
    handle.state(_state(active));
    if let Some(group) = &m.group {
        group.workspace_enter(&handle);
    }
    opusdm_ipc::debug_log!(LOG_TAG, "workspace: desktop {index} announced as {}", Resource::id(&handle));
    m.workspaces.push(Workspace { index, handle, active });
    true
}

/// The `state` value for a workspace that is (or is not) the shown desktop.
fn _state(active: bool) -> ext_workspace_handle_v1::State {
    if active { ext_workspace_handle_v1::State::Active } else { ext_workspace_handle_v1::State::empty() }
}

impl GlobalDispatch<ExtWorkspaceManagerV1, ()> for State {
    /// A client bound the manager: it gets its group and every desktop
    /// right away (before any later reply, such as a roundtrip `done`).
    fn bind(
        state: &mut State,
        dh: &DisplayHandle,
        client: &Client,
        resource: New<ExtWorkspaceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        let manager = data_init.init(resource, ());
        let group = match client.create_resource::<ExtWorkspaceGroupHandleV1, (), State>(dh, manager.version(), ()) {
            Ok(group) => {
                manager.workspace_group(&group);
                group.capabilities(GroupCapabilities::empty());
                Some(group)
            }
            Err(e) => {
                opusdm_ipc::err_log!(LOG_TAG, "workspace: no group for manager {}: {e}", manager.id());
                None
            }
        };
        opusdm_ipc::debug_log!(LOG_TAG, "workspace: manager {} bound", manager.id());
        state.workspaces.managers.push(Manager {
            manager,
            group,
            outputs: Vec::new(),
            workspaces: Vec::new(),
            announced: 0,
            pending_activate: None,
        });
        state.workspaces_update();
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for State {
    /// `commit` applies the last `activate` since the previous commit
    /// (`switch_desktop`, which notifies every manager); `stop` ends the
    /// manager with `finished`.
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ExtWorkspaceManagerV1,
        request: ext_workspace_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Commit => {
                let entry = state.workspaces.managers.iter_mut().find(|m| &m.manager == resource);
                let Some(index) = entry.and_then(|m| m.pending_activate.take()) else {
                    opusdm_ipc::debug_log!(LOG_TAG, "workspace: commit of manager {} with nothing to apply", resource.id());
                    return;
                };
                if let Err(e) = state.switch_desktop(index) {
                    opusdm_ipc::err_log!(LOG_TAG, "workspace: activate from manager {}: {e}", resource.id());
                }
            }
            ext_workspace_manager_v1::Request::Stop => {
                state.workspaces.managers.retain(|m| &m.manager != resource);
                resource.finished();
                opusdm_ipc::debug_log!(LOG_TAG, "workspace: manager {} stopped", resource.id());
            }
            other => opusdm_ipc::err_log!(LOG_TAG, "workspace: unknown manager request {other:?} ignored"),
        }
    }

    /// The manager is gone (finished, or its client died): it leaves the table.
    fn destroyed(state: &mut State, _client: ClientId, resource: &ExtWorkspaceManagerV1, _data: &()) {
        state.workspaces.managers.retain(|m| &m.manager != resource);
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for State {
    /// `create_workspace` is not supported (no group capability): ignored.
    fn request(
        _state: &mut State,
        _client: &Client,
        resource: &ExtWorkspaceGroupHandleV1,
        request: ext_workspace_group_handle_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            ext_workspace_group_handle_v1::Request::Destroy => {}
            other => opusdm_ipc::debug_log!(LOG_TAG, "workspace: {other:?} on group {} ignored", resource.id()),
        }
    }

    /// The group is gone (destroyed by its client, or the client died): its
    /// manager stops sending events on it.
    fn destroyed(state: &mut State, _client: ClientId, resource: &ExtWorkspaceGroupHandleV1, _data: &()) {
        for m in state.workspaces.managers.iter_mut() {
            if m.group.as_ref() == Some(resource) {
                m.group = None;
            }
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, ()> for State {
    /// `activate` records the desktop for the next manager `commit`;
    /// `deactivate`, `assign` and `remove` are not supported (only the
    /// `activate` capability): ignored, as is any request on a removed
    /// workspace.
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ExtWorkspaceHandleV1,
        request: ext_workspace_handle_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use ext_workspace_handle_v1::Request;
        if let Request::Destroy = request { return; }
        let Some(m) = state.workspaces._manager_of_workspace(resource) else {
            opusdm_ipc::debug_log!(LOG_TAG, "workspace: {request:?} on removed workspace {} ignored", Resource::id(resource));
            return;
        };
        let Request::Activate = request else {
            opusdm_ipc::debug_log!(LOG_TAG, "workspace: {request:?} on workspace {} ignored", Resource::id(resource));
            return;
        };
        let index = m.workspaces.iter().find(|w| &w.handle == resource).map(|w| w.index);
        m.pending_activate = index;
    }

    /// The workspace is gone (destroyed by its client, or the client died):
    /// it leaves the table.
    fn destroyed(state: &mut State, _client: ClientId, resource: &ExtWorkspaceHandleV1, _data: &()) {
        for m in state.workspaces.managers.iter_mut() {
            m.workspaces.retain(|w| &w.handle != resource);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_shown_desktop_has_the_active_bit() {
        assert_eq!(_state(true).bits(), 1);
        assert_eq!(_state(false).bits(), 0);
    }
}
