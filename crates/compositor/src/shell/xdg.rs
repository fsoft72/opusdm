//! xdg-shell: toplevel windows and popups (with grabs and the output constraint).

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    delegate_xdg_shell,
    desktop::{find_popup_root_surface, get_popup_toplevel_coords, PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window},
    input::{pointer::Focus, Seat},
    reexports::wayland_server::{
        protocol::{wl_seat, wl_surface::WlSurface},
        Resource,
    },
    utils::{Logical, Point, Serial},
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

    /// Keeps the popup inside the output and tracks it; its initial configure
    /// goes out on its first commit.
    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let id = surface.wl_surface().id();
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            opusdm_ipc::err_log!(LOG_TAG, "popup {id} not tracked: {err:?}");
        }
    }

    /// Popup grab (menus: a click outside closes them). Refused with a log
    /// when the root of the popup is not a mapped toplevel, when the seat has
    /// no keyboard or pointer, or when another grab is active that this
    /// request does not descend from.
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let id = surface.wl_surface().id();
        let Some(seat) = Seat::<State>::from_resource(&seat) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup grab {id} refused: unknown seat");
            return;
        };
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup grab {id} refused: the root of the popup is gone");
            return;
        };
        if self.root_position(&root).is_none() {
            opusdm_ipc::err_log!(LOG_TAG, "popup grab {id} refused: root {} is not a mapped toplevel", root.id());
            return;
        }
        let mut grab = match self.popups.grab_popup(root, kind, &seat, serial) {
            Ok(grab) => grab,
            Err(err) => {
                opusdm_ipc::err_log!(LOG_TAG, "popup grab {id} refused: {err:?}");
                return;
            }
        };
        let (Some(keyboard), Some(pointer)) = (seat.get_keyboard(), seat.get_pointer()) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup grab {id} refused: the seat has no keyboard or no pointer");
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        };
        let previous = grab.previous_serial().unwrap_or(serial);
        if keyboard.is_grabbed() && !(keyboard.has_grab(serial) || keyboard.has_grab(previous)) {
            opusdm_ipc::err_log!(LOG_TAG, "popup grab {id} refused: the keyboard is grabbed by someone else");
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        let pointer_previous = grab.previous_serial().unwrap_or_else(|| grab.serial());
        if pointer.is_grabbed() && !(pointer.has_grab(serial) || pointer.has_grab(pointer_previous)) {
            opusdm_ipc::err_log!(LOG_TAG, "popup grab {id} refused: the pointer is grabbed by someone else");
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        keyboard.set_focus(self, grab.current_grab(), serial);
        keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
    }

    /// Applies the new positioner, keeps the popup inside the output and
    /// answers with `repositioned`.
    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
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

impl State {
    /// Position in the space of the geometry of `root`, the root surface of a
    /// popup. A mapped toplevel is found in the space; a layer surface root
    /// is not supported yet (task 5 adds it here). `None` and a log for an
    /// unknown root.
    fn root_position(&self, root: &WlSurface) -> Option<Point<i32, Logical>> {
        let window = self.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == root));
        let Some(window) = window else {
            opusdm_ipc::err_log!(LOG_TAG, "popup root {} is not a mapped toplevel (layer roots come in task 5)", root.id());
            return None;
        };
        let Some(geo) = self.space.element_geometry(window) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup root {} has no geometry in the space", root.id());
            return None;
        };
        Some(geo.loc)
    }

    /// Moves the pending geometry of `popup` inside the output with its
    /// constraint adjustment. The target is the output geometry expressed
    /// relative to the popup's parent (its root position plus the offsets of
    /// the parent popups). An unknown root leaves the geometry as asked and
    /// logs.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let id = popup.wl_surface().id();
        let Ok(root) = find_popup_root_surface(&kind) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup {id} not unconstrained: its root is gone");
            return;
        };
        let Some(root_pos) = self.root_position(&root) else { return };
        let Some(output_geo) = self.space.output_geometry(&self.output) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup {id} not unconstrained: the output is not mapped");
            return;
        };
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= root_pos;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

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

/// Commit of a popup surface: lets the popup manager update its tree and
/// sends the initial configure on the first commit. A refused configure is
/// logged.
pub fn handle_popup_commit(state: &mut State, surface: &WlSurface) {
    state.popups.commit(surface);
    let Some(PopupKind::Xdg(popup)) = state.popups.find_popup(surface) else { return };
    if popup.is_initial_configure_sent() { return; }
    if let Err(err) = popup.send_configure() {
        opusdm_ipc::err_log!(LOG_TAG, "popup {} initial configure failed: {err:?}", surface.id());
    }
}
