//! xdg-shell: toplevel windows (popups come in task 4).

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    delegate_xdg_shell,
    desktop::Window,
    reexports::wayland_server::{
        protocol::{wl_seat, wl_surface::WlSurface},
        Resource,
    },
    utils::Serial,
    wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
};

use crate::geometry::cascade_position;
use crate::state::{State, WindowInfo, LOG_TAG};

impl XdgShellHandler for State {
    /// The xdg-shell state.
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    /// Tracks a new toplevel; the initial configure (no size: the client
    /// decides) goes out on its initial commit, the window is mapped once its
    /// first buffer is committed.
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| state.size = None);
        self.windows.insert(surface.wl_surface().clone(), WindowInfo::default());
        self.unmapped.push(Window::new_wayland_window(surface));
    }

    /// Popups are not handled before task 4: logged and left unconfigured.
    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        opusdm_ipc::err_log!(LOG_TAG, "popup {} ignored: xdg popups are not supported yet", surface.wl_surface().id());
    }

    /// Popup grabs come with popups (task 4).
    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        opusdm_ipc::err_log!(LOG_TAG, "popup grab {} ignored: xdg popups are not supported yet", surface.wl_surface().id());
    }

    /// Popup repositioning comes with popups (task 4).
    fn reposition_request(&mut self, surface: PopupSurface, _positioner: PositionerState, _token: u32) {
        opusdm_ipc::err_log!(LOG_TAG, "popup reposition {} ignored: xdg popups are not supported yet", surface.wl_surface().id());
    }

    /// No tasklist before stage 2: the window stays shown.
    fn minimize_request(&mut self, _surface: ToplevelSurface) {
        opusdm_ipc::debug_log!(LOG_TAG, "minimize ignored: no tasklist before stage 2");
    }

    /// Removes the window from the space and the tables; when it had the
    /// keyboard, the topmost remaining window gets it.
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let wl = surface.wl_surface();
        self.windows.remove(wl);
        self.unmapped.retain(|w| w.toplevel().is_some_and(|t| t.wl_surface() != wl));
        let mapped = self.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == wl)).cloned();
        if let Some(window) = mapped {
            self.space.unmap_elem(&window);
        }
        opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} destroyed, {} left", wl.id(), self.windows.len());
        if self.focus_is_gone_or(wl) {
            self.focus_topmost();
        }
    }
}

delegate_xdg_shell!(State);

/// Commit of a toplevel not mapped yet: sends the initial configure, or maps
/// the window and focuses it once a buffer is committed. A new window takes
/// the next cascade position, a window unmapped by a null buffer its old one.
pub fn handle_commit(state: &mut State, surface: &WlSurface) {
    let found = state.unmapped.iter().position(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface));
    let Some(index) = found else { return };
    let Some(toplevel) = state.unmapped.get(index).and_then(|w| w.toplevel()) else {
        opusdm_ipc::err_log!(LOG_TAG, "commit: unmapped window {} has no toplevel", surface.id());
        return;
    };
    if !toplevel.is_initial_configure_sent() {
        toplevel.send_configure();
        return;
    }
    let has_buffer = with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
    if !has_buffer { return; }
    let window = state.unmapped.remove(index);
    window.on_commit();
    let size = window.geometry().size;
    let last = state.windows.get_mut(surface).and_then(|info| info.last_position.take());
    let (x, y) = match last {
        Some(pos) => pos,
        None => {
            let pos = cascade_position(state.usable_area(), (size.w, size.h), state.cascade_index);
            state.cascade_index = state.cascade_index.wrapping_add(1);
            pos
        }
    };
    opusdm_ipc::debug_log!(LOG_TAG, "map toplevel {} {}x{} at {x},{y}", surface.id(), size.w, size.h);
    state.space.map_element(window.clone(), (x, y), true);
    state.focus_window(Some(window));
}

/// Commit of a mapped toplevel with no buffer (xdg unmap): removes it from the
/// space, keeps its `windows` entry with its position, puts it back in
/// `unmapped` (its next commit gets a fresh initial configure) and moves the
/// keyboard to the topmost window when it had it. True when it unmapped.
pub fn unmap_if_bufferless(state: &mut State, surface: &WlSurface) -> bool {
    let found = state.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface)).cloned();
    let Some(window) = found else { return false };
    let has_buffer = with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
    if has_buffer { return false; }
    let position = state.space.element_location(&window).map(|p| (p.x, p.y));
    match state.windows.get_mut(surface) {
        Some(info) => info.last_position = position,
        None => opusdm_ipc::err_log!(LOG_TAG, "unmap: toplevel {} has no window entry", surface.id()),
    }
    state.space.unmap_elem(&window);
    window.set_activated(false);
    state.unmapped.push(window);
    opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} unmapped (null buffer) at {position:?}", surface.id());
    if state.focus_is_gone_or(surface) {
        state.focus_topmost();
    }
    true
}
