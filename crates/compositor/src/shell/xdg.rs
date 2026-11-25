//! xdg-shell: toplevel windows and popups (with grabs and the output constraint).

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, PopupKeyboardGrab, PopupKind,
        PopupPointerGrab, PopupUngrabStrategy, Window, WindowSurfaceType,
    },
    input::{
        pointer::{Focus, GrabStartData},
        Seat,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_output::WlOutput, wl_seat, wl_surface::WlSurface},
            Resource,
        },
    },
    utils::{Logical, Point, Serial},
    wayland::shell::xdg::{Configure, PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
};

use crate::geometry::{cascade_position, Edges, Rect};
use crate::state::{client_pid, has_buffer, State, WindowInfo, LOG_TAG};

impl XdgShellHandler for State {
    /// The xdg-shell state.
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    /// Tracks a new toplevel with a new id and its client pid (read once
    /// here), on the current desktop and on top of the stack; the initial configure (no size: the client decides)
    /// goes out on its initial commit, the window is mapped once its first
    /// buffer is committed.
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| state.size = None);
        let wl = surface.wl_surface().clone();
        let id = self.new_window_id();
        let window = Window::new_wayland_window(surface);
        let mut info = WindowInfo::new(id, window.clone(), self.desktops.current());
        info.pid = client_pid(&self.display_handle, &wl);
        self.windows.insert(wl.clone(), info);
        self.stack.push(id);
        self.unmapped.push(window);
        opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} is window {}", wl.id(), id.0);
    }

    /// Keeps the popup inside the output and tracks it; its initial configure
    /// goes out on its first commit. A popup created without a parent waits
    /// for a layer surface to adopt it (`WlrLayerShellHandler::new_popup`),
    /// which unconstrains it then.
    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        let id = surface.wl_surface().id();
        match surface.get_parent_surface() {
            Some(_) => self.unconstrain_popup(&surface),
            None => opusdm_ipc::debug_log!(LOG_TAG, "popup {id} has no parent yet, waiting for a layer surface"),
        }
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            opusdm_ipc::err_log!(LOG_TAG, "popup {id} not tracked: {err:?}");
        }
    }

    /// Popup grab (menus: a click outside closes them), on a popup of a
    /// toplevel or of a layer surface. Refused with a log, and the popup
    /// dismissed (`popup_done`), when the root of the popup is not a mapped
    /// toplevel or layer surface, when an exclusive keyboard layer holds the
    /// keyboard and the popup is not one of its own (the layer keeps the
    /// keyboard), when the seat has no keyboard or pointer, or when another
    /// grab is active that this request does not descend from.
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let id = surface.wl_surface().id();
        let Some(seat) = Seat::<State>::from_resource(&seat) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup grab {id} refused: unknown seat, popup dismissed");
            surface.send_popup_done();
            return;
        };
        let kind = PopupKind::Xdg(surface.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup grab {id} refused: the root of the popup is gone, popup dismissed");
            surface.send_popup_done();
            return;
        };
        if self.root_position(&root).is_none() {
            opusdm_ipc::err_log!(
                LOG_TAG,
                "popup grab {id} refused: root {} is not a mapped toplevel or layer surface, popup dismissed",
                root.id()
            );
            surface.send_popup_done();
            return;
        }
        // The popup grab would take the keyboard and, when the menu closes,
        // Smithay gives it to the popup's root, never back to the layer.
        if let Some(layer) = self.exclusive_layer.as_ref().filter(|layer| **layer != root) {
            opusdm_ipc::err_log!(
                LOG_TAG,
                "popup grab {id} refused: exclusive keyboard layer {} holds the keyboard, popup dismissed",
                layer.id()
            );
            surface.send_popup_done();
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

    /// Maximizes the window: it fills `usable_area()` until unmaximized.
    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.fill_window(&surface, Fill::Maximized);
    }

    /// Restores the window from maximized (to its previous geometry, or to
    /// fullscreen when it is fullscreen too).
    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.unfill_window(&surface, Fill::Maximized);
    }

    /// Makes the window fullscreen on the output (one output: a requested
    /// output is ignored): it fills the whole output until unfullscreened.
    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<WlOutput>) {
        if output.is_some() {
            opusdm_ipc::debug_log!(LOG_TAG, "fullscreen {}: requested output ignored, one output only", surface.wl_surface().id());
        }
        self.fill_window(&surface, Fill::Fullscreen);
    }

    /// Restores the window from fullscreen (to its previous geometry, or to
    /// maximized when it is maximized too).
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.unfill_window(&surface, Fill::Fullscreen);
    }

    /// Interactive move asked by the client (title bar drag): starts a
    /// `MoveGrab` when `serial` is the pointer's current click grab on this
    /// window; refused with a log otherwise.
    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let wl = surface.wl_surface();
        let Some(start_data) = _check_grab(&seat, wl, serial, "move") else { return };
        let Some(window) = self.mapped_window(wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "move {} refused: the toplevel is not mapped", wl.id());
            return;
        };
        self.start_move_grab(window, start_data, serial);
    }

    /// Interactive resize asked by the client (border drag): starts a
    /// `ResizeGrab` on `edges` when `serial` is the pointer's current click
    /// grab on this window; refused with a log otherwise.
    fn resize_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial, edges: xdg_toplevel::ResizeEdge) {
        let wl = surface.wl_surface();
        let Some(start_data) = _check_grab(&seat, wl, serial, "resize") else { return };
        let Some(window) = self.mapped_window(wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "resize {} refused: the toplevel is not mapped", wl.id());
            return;
        };
        self.start_resize_grab(window, _edges_from_xdg(edges), start_data, serial);
    }

    /// A toplevel acked a configure: an interactive resize waiting for its
    /// final configure moves on (`grabs::handle_ack`). Popups: nothing to do.
    fn ack_configure(&mut self, surface: WlSurface, configure: Configure) {
        let Configure::Toplevel(configure) = configure else { return };
        crate::grabs::handle_ack(self, &surface, configure.serial);
    }

    /// Minimizes the window: it leaves the space (and the keyboard) until it
    /// is activated again.
    fn minimize_request(&mut self, surface: ToplevelSurface) {
        let wl = surface.wl_surface();
        let Some(id) = self.windows.get(wl).map(|info| info.id) else {
            opusdm_ipc::err_log!(LOG_TAG, "minimize {} refused: unknown toplevel", wl.id());
            return;
        };
        if let Err(err) = self.set_minimized(id, true) {
            opusdm_ipc::err_log!(LOG_TAG, "minimize {}: {err}", wl.id());
        }
    }

    /// Removes the window from the space, the tables and the stack; when it
    /// had the keyboard, the topmost remaining window gets it.
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let wl = surface.wl_surface();
        if let Some(info) = self.windows.remove(wl) {
            self.stack.retain(|id| *id != info.id);
        }
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

/// Start data of the pointer grab a client `move` / `resize` request
/// (`what`) refers to: the pointer must have an active grab with `serial`
/// (the implicit grab of a button press) whose focus belongs to `surface`
/// (the toplevel or one of its subsurfaces). `None`, logged, otherwise.
fn _check_grab(seat: &wl_seat::WlSeat, surface: &WlSurface, serial: Serial, what: &str) -> Option<GrabStartData<State>> {
    let id = surface.id();
    let Some(seat) = Seat::<State>::from_resource(seat) else {
        opusdm_ipc::err_log!(LOG_TAG, "{what} {id} refused: unknown seat");
        return None;
    };
    let Some(pointer) = seat.get_pointer() else {
        opusdm_ipc::err_log!(LOG_TAG, "{what} {id} refused: the seat has no pointer");
        return None;
    };
    if !pointer.has_grab(serial) {
        opusdm_ipc::err_log!(LOG_TAG, "{what} {id} refused: serial {serial:?} is not the current pointer grab");
        return None;
    }
    let Some(start_data) = pointer.grab_start_data() else {
        opusdm_ipc::err_log!(LOG_TAG, "{what} {id} refused: the pointer grab has no start data");
        return None;
    };
    let on_surface = start_data.focus.as_ref().is_some_and(|(focus, _)| &super::_root_surface(focus) == surface);
    if !on_surface {
        opusdm_ipc::err_log!(LOG_TAG, "{what} {id} refused: the pointer grab is not on this window");
        return None;
    }
    Some(start_data)
}

/// The edges of an xdg resize request; `None` (and unknown values) give no edge.
fn _edges_from_xdg(edge: xdg_toplevel::ResizeEdge) -> Edges {
    use xdg_toplevel::ResizeEdge as E;
    let top = matches!(edge, E::Top | E::TopLeft | E::TopRight);
    let bottom = matches!(edge, E::Bottom | E::BottomLeft | E::BottomRight);
    let left = matches!(edge, E::Left | E::TopLeft | E::BottomLeft);
    let right = matches!(edge, E::Right | E::TopRight | E::BottomRight);
    Edges { top, bottom, left, right }
}

/// The two states where a window fills an area of the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// Fills `usable_area()` (the output minus the exclusive zones).
    Maximized,
    /// Fills the whole output, above the TOP layer.
    Fullscreen,
}

impl State {
    /// Position in the space of the geometry of `root`, the root surface of a
    /// popup: a mapped toplevel (its window geometry) or a layer surface of
    /// the layer map (its surface origin). `None` for any other root; the
    /// callers log it with their context.
    fn root_position(&self, root: &WlSurface) -> Option<Point<i32, Logical>> {
        let window = self.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == root));
        if let Some(window) = window {
            return self.space.element_geometry(window).map(|geo| geo.loc);
        }
        let output_loc = self.space.output_geometry(&self.output)?.loc;
        let map = layer_map_for_output(&self.output);
        let layer = map.layer_for_surface(root, WindowSurfaceType::TOPLEVEL)?;
        let geo = map.layer_geometry(layer)?;
        // `layer_geometry` is the bounding box of the surface tree; its
        // offset from the surface origin is the bbox location.
        Some(output_loc + geo.loc - layer.bbox().loc)
    }

    /// The mapped window of the toplevel `surface`, if it is in the space.
    fn mapped_window(&self, surface: &WlSurface) -> Option<Window> {
        self.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface)).cloned()
    }

    /// Area `fill` covers: the usable area or the whole output.
    fn fill_area(&self, fill: Fill) -> Rect {
        match fill {
            Fill::Maximized => self.usable_area(),
            Fill::Fullscreen => self.output_area(),
        }
    }

    /// Puts the toplevel `surface` in the `fill` state: stores the geometry
    /// to restore (the first time it leaves the normal state), configures the
    /// area size and moves a mapped window to the area origin. An unmapped
    /// window is placed at the origin when it maps. Always answers with a
    /// configure once the initial one was sent, as the protocol asks.
    fn fill_window(&mut self, surface: &ToplevelSurface, fill: Fill) {
        let wl = surface.wl_surface();
        let window = self.mapped_window(wl);
        let normal_geo = window.as_ref().and_then(|w| {
            let loc = self.space.element_location(w)?;
            let size = w.geometry().size;
            Some(Rect { x: loc.x, y: loc.y, w: size.w, h: size.h })
        });
        // Fullscreen wins over maximized: a maximized window going fullscreen fills the output.
        let effective = if fill == Fill::Maximized && self.windows.get(wl).is_some_and(|i| i.fullscreen) { Fill::Fullscreen } else { fill };
        let area = self.fill_area(effective);
        let Some(info) = self.windows.get_mut(wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "{fill:?} {} refused: unknown toplevel", wl.id());
            if surface.is_initial_configure_sent() {
                surface.send_configure();
            }
            return;
        };
        if !info.maximized && !info.fullscreen {
            info.restore = normal_geo;
        }
        match fill {
            Fill::Maximized => info.maximized = true,
            Fill::Fullscreen => info.fullscreen = true,
        }
        surface.with_pending_state(|state| {
            state.states.set(match fill {
                Fill::Maximized => xdg_toplevel::State::Maximized,
                Fill::Fullscreen => xdg_toplevel::State::Fullscreen,
            });
            state.size = Some((area.w, area.h).into());
        });
        opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} {fill:?} to {area:?}", wl.id());
        if let Some(window) = window {
            self.move_window(&window, (area.x, area.y));
        }
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    /// Takes the toplevel `surface` out of the `fill` state: when the other
    /// fill state is still on, it fills that area; otherwise it gets back the
    /// stored geometry (size sent explicitly, so the client goes back to it)
    /// or, with none stored, a configure that leaves the size to the client.
    fn unfill_window(&mut self, surface: &ToplevelSurface, fill: Fill) {
        let wl = surface.wl_surface();
        let Some(info) = self.windows.get_mut(wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "un{fill:?} {} refused: unknown toplevel", wl.id());
            if surface.is_initial_configure_sent() {
                surface.send_configure();
            }
            return;
        };
        match fill {
            Fill::Maximized => info.maximized = false,
            Fill::Fullscreen => info.fullscreen = false,
        }
        let other = if info.maximized { Some(Fill::Maximized) } else if info.fullscreen { Some(Fill::Fullscreen) } else { None };
        let target = match other {
            Some(other) => Some(self.fill_area(other)),
            None => info.restore.take(),
        };
        surface.with_pending_state(|state| {
            state.states.unset(match fill {
                Fill::Maximized => xdg_toplevel::State::Maximized,
                Fill::Fullscreen => xdg_toplevel::State::Fullscreen,
            });
            state.size = target.map(|r| (r.w, r.h).into());
        });
        opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} un{fill:?} to {target:?}", wl.id());
        if let (Some(window), Some(r)) = (self.mapped_window(wl), target) {
            self.move_window(&window, (r.x, r.y));
        }
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    /// Called after something may have changed the usable area (a layer
    /// surface mapped, changed or went away), with the area `before` it:
    /// when it changed, every maximized (not fullscreen) window is
    /// configured to the new area and moved to its origin.
    pub fn usable_area_changed(&mut self, before: Rect) {
        let area = self.usable_area();
        if area == before { return; }
        opusdm_ipc::debug_log!(LOG_TAG, "usable area {before:?} -> {area:?}");
        let maximized: Vec<WlSurface> =
            self.windows.iter().filter(|(_, i)| i.maximized && !i.fullscreen).map(|(s, _)| s.clone()).collect();
        for wl in maximized {
            self.refill_window(&wl, area, "maximized");
        }
    }

    /// Called after the output changed size (the nested host window was
    /// resized), with the usable area `before` it: the maximized windows
    /// follow the new usable area (`usable_area_changed`) and every
    /// fullscreen window is configured to the new output size.
    pub fn output_area_changed(&mut self, before_usable: Rect) {
        self.usable_area_changed(before_usable);
        let area = self.output_area();
        let fullscreen: Vec<WlSurface> = self.windows.iter().filter(|(_, i)| i.fullscreen).map(|(s, _)| s.clone()).collect();
        for wl in fullscreen {
            self.refill_window(&wl, area, "fullscreen");
        }
    }

    /// Configures the toplevel `wl` (mapped or not) to the size of `area`
    /// and moves it to the area origin when it is mapped; `what` names the
    /// fill state in the logs.
    fn refill_window(&mut self, wl: &WlSurface, area: Rect, what: &str) {
        let window = self.mapped_window(wl);
        // The table has every toplevel: mapped, unmapped or hidden.
        let toplevel = self.windows.get(wl).and_then(|info| info.window.toplevel()).cloned();
        let Some(toplevel) = toplevel else {
            opusdm_ipc::err_log!(LOG_TAG, "{what} toplevel {} not found, not reconfigured", wl.id());
            return;
        };
        toplevel.with_pending_state(|state| state.size = Some((area.w, area.h).into()));
        if let Some(window) = window {
            self.move_window(&window, (area.x, area.y));
        }
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    /// Moves the pending geometry of `popup` inside the output with its
    /// constraint adjustment. The target is the output geometry expressed
    /// relative to the popup's parent (its root position plus the offsets of
    /// the parent popups). An unknown root leaves the geometry as asked and
    /// logs.
    pub(super) fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let id = popup.wl_surface().id();
        let Ok(root) = find_popup_root_surface(&kind) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup {id} not unconstrained: its root is gone");
            return;
        };
        let Some(root_pos) = self.root_position(&root) else {
            opusdm_ipc::err_log!(LOG_TAG, "popup {id} not unconstrained: root {} is not a mapped toplevel or layer surface", root.id());
            return;
        };
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
/// the window and focuses it once a buffer is committed. It maps on the
/// current desktop, not minimized, on top of the stack. A fullscreen or
/// maximized window maps at the origin of its area, a new window takes the
/// next cascade position, a window unmapped by a null buffer its old one.
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
    let current = state.desktops.current();
    let Some(info) = state.windows.get_mut(surface) else {
        opusdm_ipc::err_log!(LOG_TAG, "commit: toplevel {} has no window entry, not mapped", surface.id());
        return;
    };
    info.desktop = current;
    info.minimized = false;
    let last = info.last_position.take();
    let fill = if info.fullscreen { Some(Fill::Fullscreen) } else if info.maximized { Some(Fill::Maximized) } else { None };
    let window = state.unmapped.remove(index);
    window.on_commit();
    let size = window.geometry().size;
    let (x, y) = match (fill, last) {
        (Some(fill), _) => {
            let area = state.fill_area(fill);
            (area.x, area.y)
        }
        (None, Some(pos)) => pos,
        (None, None) => {
            let pos = cascade_position(state.usable_area(), (size.w, size.h), state.cascade_index);
            state.cascade_index = state.cascade_index.wrapping_add(1);
            pos
        }
    };
    opusdm_ipc::debug_log!(LOG_TAG, "map toplevel {} {}x{} at {x},{y}", surface.id(), size.w, size.h);
    state.space.map_element(window.clone(), (x, y), true);
    state.focus_window(Some(window));
}

/// Commit of a mapped toplevel (shown, or hidden: minimized or on another
/// desktop) with no buffer (xdg unmap): removes it from the space, keeps its
/// `windows` entry with its position, puts it back in `unmapped` (its next
/// commit gets a fresh initial configure) and moves the keyboard to the
/// topmost window when it had it. True when it unmapped.
pub fn unmap_if_bufferless(state: &mut State, surface: &WlSurface) -> bool {
    let Some(info) = state.windows.get_mut(surface) else { return false };
    let window = info.window.clone();
    if state.unmapped.contains(&window) || has_buffer(&window) { return false; }
    // A hidden window is out of the space: it keeps the position stored when it was hidden.
    let position = state.space.element_location(&window).map(|p| (p.x, p.y)).or(info.last_position);
    info.last_position = position;
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
