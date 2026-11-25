//! Interactive move and resize grabs of the pointer.
//!
//! A grab is started by Alt + click (`input.rs`) or by a client request
//! (`xdg_toplevel.move` / `resize`, `shell/xdg.rs`). While it is active the
//! pointer has no client focus; it ends when the button that started it is
//! released (or when another grab replaces it).

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{protocol::wl_surface::WlSurface, Resource},
    },
    utils::{IsAlive, Logical, Point, Serial},
    wayland::shell::xdg::ToplevelSurface,
};

use crate::geometry::{resize_rect, Edges, Rect, MIN_WINDOW_SIZE};
use crate::state::{State, LOG_TAG};

/// Where a window is in an interactive resize; kept in `WindowInfo` so the
/// commits of the client can move the window when it is resized by its top
/// or left edge (the opposite edges stay where they were).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResizeTrack {
    /// No resize going on.
    #[default]
    Idle,
    /// A `ResizeGrab` is active.
    Resizing {
        /// The edges being dragged.
        edges: Edges,
        /// Geometry of the window when the resize started.
        initial: Rect,
    },
    /// The grab ended; the next commit (the final size) is still placed,
    /// then the track goes back to `Idle`.
    WaitingLastCommit {
        /// The edges that were dragged.
        edges: Edges,
        /// Geometry of the window when the resize started.
        initial: Rect,
    },
}

/// Returns the edges an Alt + right button resize drags: the ones of the
/// quadrant of `window` the pointer is in. `point` is relative to the window
/// origin; the middle lines belong to the right and bottom halves.
pub fn edges_for_point(window: Rect, point: (i32, i32)) -> Edges {
    let (x, y) = point;
    let left = x < window.w / 2;
    let top = y < window.h / 2;
    Edges { top, bottom: !top, left, right: !left }
}

/// Returns the origin of a window resized from `initial` to `size` by its
/// `edges`: a dragged left or top edge moves the origin so the right or
/// bottom edge stays where it was; other edges keep the origin.
pub fn resized_origin(initial: Rect, edges: Edges, size: (i32, i32)) -> (i32, i32) {
    let (w, h) = size;
    let x = if edges.left { initial.x.saturating_add(initial.w).saturating_sub(w) } else { initial.x };
    let y = if edges.top { initial.y.saturating_add(initial.h).saturating_sub(h) } else { initial.y };
    (x, y)
}

/// Returns the origin of a window that goes from the `current` geometry
/// (maximized or fullscreen) back to the `restore` size when a move grab
/// starts at `pointer`: the pointer keeps the same relative position inside
/// the window, so the window stays under it.
pub fn restored_origin(current: Rect, restore: (i32, i32), pointer: (i32, i32)) -> (i32, i32) {
    let (rw, rh) = restore;
    let (px, py) = pointer;
    let scale = |offset: i32, new: i32, old: i32| -> i32 {
        if old <= 0 { return 0; }
        let scaled = i64::from(offset) * i64::from(new) / i64::from(old);
        i32::try_from(scaled).unwrap_or(0)
    };
    let x = px.saturating_sub(scale(px.saturating_sub(current.x), rw, current.w));
    let y = py.saturating_sub(scale(py.saturating_sub(current.y), rh, current.h));
    (x, y)
}

/// Pointer grab that moves `window` with the pointer.
pub struct MoveGrab {
    /// Pointer state when the grab started.
    pub start_data: GrabStartData<State>,
    /// The window being moved.
    pub window: Window,
    /// Position of the window when the grab started.
    pub initial_loc: Point<i32, Logical>,
}

/// Pointer grab that resizes `window` by its `edges`.
pub struct ResizeGrab {
    /// Pointer state when the grab started.
    pub start_data: GrabStartData<State>,
    /// The window being resized.
    pub window: Window,
    /// The edges being dragged.
    pub edges: Edges,
    /// Geometry of the window when the grab started.
    pub initial: Rect,
    /// Last size sent to the client, sent again without `Resizing` at the end.
    last_size: (i32, i32),
}

impl PointerGrab<State> for MoveGrab {
    /// Moves the window by the pointer delta from the start of the grab; no
    /// client has the pointer focus meanwhile. A dead window ends the grab.
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        if !self.window.alive() {
            opusdm_ipc::debug_log!(LOG_TAG, "move grab: the window is gone, grab ended");
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        let delta = event.location - self.start_data.location;
        let loc = (self.initial_loc.to_f64() + delta).to_i32_round();
        data.move_window(&self.window, (loc.x, loc.y));
    }

    /// Relative motion goes on as usual (no client has the focus).
    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    /// The button events are consumed; the release of the button that started
    /// the grab ends it.
    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        if handle.current_pressed().contains(&self.start_data.button) { return; }
        handle.unset_grab(self, data, event.serial, event.time, true);
    }

    /// Scrolling goes on as usual.
    fn axis(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, details: AxisFrame) {
        handle.axis(data, details);
    }

    /// Ends a pointer frame.
    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    /// Forwards a swipe begin.
    fn gesture_swipe_begin(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureSwipeBeginEvent) {
        handle.gesture_swipe_begin(data, event);
    }

    /// Forwards a swipe update.
    fn gesture_swipe_update(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureSwipeUpdateEvent) {
        handle.gesture_swipe_update(data, event);
    }

    /// Forwards a swipe end.
    fn gesture_swipe_end(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureSwipeEndEvent) {
        handle.gesture_swipe_end(data, event);
    }

    /// Forwards a pinch begin.
    fn gesture_pinch_begin(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GesturePinchBeginEvent) {
        handle.gesture_pinch_begin(data, event);
    }

    /// Forwards a pinch update.
    fn gesture_pinch_update(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GesturePinchUpdateEvent) {
        handle.gesture_pinch_update(data, event);
    }

    /// Forwards a pinch end.
    fn gesture_pinch_end(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GesturePinchEndEvent) {
        handle.gesture_pinch_end(data, event);
    }

    /// Forwards a hold begin.
    fn gesture_hold_begin(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureHoldBeginEvent) {
        handle.gesture_hold_begin(data, event);
    }

    /// Forwards a hold end.
    fn gesture_hold_end(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureHoldEndEvent) {
        handle.gesture_hold_end(data, event);
    }

    /// Pointer state when the grab started.
    fn start_data(&self) -> &GrabStartData<State> {
        &self.start_data
    }

    /// Nothing to undo when the move ends.
    fn unset(&mut self, _data: &mut State) {
        opusdm_ipc::debug_log!(LOG_TAG, "move grab ended");
    }
}

impl PointerGrab<State> for ResizeGrab {
    /// Computes the new geometry from the pointer delta (never under
    /// `MIN_WINDOW_SIZE`) and sends its size with the `Resizing` state; the
    /// commits place the window (`handle_commit`). A dead window ends the grab.
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let Some(toplevel) = self.window.toplevel().filter(|t| t.alive()).cloned() else {
            opusdm_ipc::debug_log!(LOG_TAG, "resize grab: the window is gone, grab ended");
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        };
        let delta = (event.location - self.start_data.location).to_i32_round();
        let r = resize_rect(self.initial, self.edges, delta.x, delta.y, MIN_WINDOW_SIZE);
        self.last_size = (r.w, r.h);
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
            state.size = Some((r.w, r.h).into());
        });
        toplevel.send_pending_configure();
    }

    /// Relative motion goes on as usual (no client has the focus).
    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    /// The button events are consumed; the release of the button that started
    /// the grab ends it (`unset` sends the final configure).
    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        if handle.current_pressed().contains(&self.start_data.button) { return; }
        handle.unset_grab(self, data, event.serial, event.time, true);
    }

    /// Scrolling goes on as usual.
    fn axis(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, details: AxisFrame) {
        handle.axis(data, details);
    }

    /// Ends a pointer frame.
    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    /// Forwards a swipe begin.
    fn gesture_swipe_begin(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureSwipeBeginEvent) {
        handle.gesture_swipe_begin(data, event);
    }

    /// Forwards a swipe update.
    fn gesture_swipe_update(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureSwipeUpdateEvent) {
        handle.gesture_swipe_update(data, event);
    }

    /// Forwards a swipe end.
    fn gesture_swipe_end(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureSwipeEndEvent) {
        handle.gesture_swipe_end(data, event);
    }

    /// Forwards a pinch begin.
    fn gesture_pinch_begin(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GesturePinchBeginEvent) {
        handle.gesture_pinch_begin(data, event);
    }

    /// Forwards a pinch update.
    fn gesture_pinch_update(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GesturePinchUpdateEvent) {
        handle.gesture_pinch_update(data, event);
    }

    /// Forwards a pinch end.
    fn gesture_pinch_end(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GesturePinchEndEvent) {
        handle.gesture_pinch_end(data, event);
    }

    /// Forwards a hold begin.
    fn gesture_hold_begin(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureHoldBeginEvent) {
        handle.gesture_hold_begin(data, event);
    }

    /// Forwards a hold end.
    fn gesture_hold_end(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureHoldEndEvent) {
        handle.gesture_hold_end(data, event);
    }

    /// Pointer state when the grab started.
    fn start_data(&self) -> &GrabStartData<State> {
        &self.start_data
    }

    /// The resize ends (button released or grab replaced): sends the last
    /// size without the `Resizing` state and lets the next commit place the
    /// window. A dead window has nothing to configure.
    fn unset(&mut self, data: &mut State) {
        let Some(toplevel) = self.window.toplevel().filter(|t| t.alive()).cloned() else {
            opusdm_ipc::debug_log!(LOG_TAG, "resize grab ended: the window is gone");
            return;
        };
        let (w, h) = self.last_size;
        toplevel.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Resizing);
            state.size = Some((w, h).into());
        });
        toplevel.send_pending_configure();
        let wl = toplevel.wl_surface();
        match data.windows.get_mut(wl) {
            Some(info) => info.resize = ResizeTrack::WaitingLastCommit { edges: self.edges, initial: self.initial },
            None => opusdm_ipc::err_log!(LOG_TAG, "resize grab ended: toplevel {} has no window entry", wl.id()),
        }
        opusdm_ipc::debug_log!(LOG_TAG, "resize grab of {} ended at {w}x{h}", wl.id());
    }
}

impl State {
    /// Starts a `MoveGrab` of the mapped `window`. A maximized or fullscreen
    /// window is first restored: its states are cleared with a configure of
    /// its restore size, and it is placed so it stays under the pointer.
    /// Refused with a log when the seat has no pointer or the window is not a
    /// mapped toplevel.
    pub fn start_move_grab(&mut self, window: Window, start_data: GrabStartData<State>, serial: Serial) {
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "move grab refused: the seat has no pointer");
            return;
        };
        let Some(toplevel) = window.toplevel().cloned() else {
            opusdm_ipc::err_log!(LOG_TAG, "move grab refused: the window has no toplevel");
            return;
        };
        let Some(mut loc) = self.space.element_location(&window) else {
            opusdm_ipc::err_log!(LOG_TAG, "move grab refused: toplevel {} is not mapped", toplevel.wl_surface().id());
            return;
        };
        let info = self.windows.get(toplevel.wl_surface()).cloned().unwrap_or_default();
        if info.maximized || info.fullscreen {
            let restore = info.restore.map(|r| (r.w, r.h));
            self._leave_fill(&toplevel, restore);
            if let Some(size) = restore {
                let geo = window.geometry().size;
                let current = Rect { x: loc.x, y: loc.y, w: geo.w, h: geo.h };
                let p = start_data.location.to_i32_round();
                let origin = restored_origin(current, size, (p.x, p.y));
                self.move_window(&window, origin);
                loc = origin.into();
            }
        }
        opusdm_ipc::debug_log!(LOG_TAG, "move grab of {} from {},{}", toplevel.wl_surface().id(), loc.x, loc.y);
        let grab = MoveGrab { start_data, window, initial_loc: loc };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Starts a `ResizeGrab` of the mapped `window` by its `edges`. A
    /// maximized or fullscreen window leaves that state first and is resized
    /// from its current geometry. Refused with a log when no edge is given,
    /// the seat has no pointer or the window is not a known mapped toplevel.
    pub fn start_resize_grab(&mut self, window: Window, edges: Edges, start_data: GrabStartData<State>, serial: Serial) {
        if edges == Edges::default() {
            opusdm_ipc::err_log!(LOG_TAG, "resize grab refused: no edge to drag");
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "resize grab refused: the seat has no pointer");
            return;
        };
        let Some(toplevel) = window.toplevel().cloned() else {
            opusdm_ipc::err_log!(LOG_TAG, "resize grab refused: the window has no toplevel");
            return;
        };
        let wl = toplevel.wl_surface().clone();
        let Some(loc) = self.space.element_location(&window) else {
            opusdm_ipc::err_log!(LOG_TAG, "resize grab refused: toplevel {} is not mapped", wl.id());
            return;
        };
        let size = window.geometry().size;
        let initial = Rect { x: loc.x, y: loc.y, w: size.w, h: size.h };
        let Some(info) = self.windows.get_mut(&wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "resize grab refused: toplevel {} has no window entry", wl.id());
            return;
        };
        info.resize = ResizeTrack::Resizing { edges, initial };
        let filled = info.maximized || info.fullscreen;
        if filled {
            self._leave_fill(&toplevel, Some((initial.w, initial.h)));
        }
        opusdm_ipc::debug_log!(LOG_TAG, "resize grab of {} {edges:?} from {initial:?}", wl.id());
        let grab = ResizeGrab { start_data, window, edges, initial, last_size: (initial.w, initial.h) };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Takes the toplevel out of maximized and fullscreen for an interactive
    /// move or resize: clears both flags and the stored restore geometry,
    /// unsets both states and configures `size` (`None`: the client decides).
    fn _leave_fill(&mut self, toplevel: &ToplevelSurface, size: Option<(i32, i32)>) {
        let wl = toplevel.wl_surface();
        match self.windows.get_mut(wl) {
            Some(info) => {
                info.maximized = false;
                info.fullscreen = false;
                info.restore = None;
            }
            None => opusdm_ipc::err_log!(LOG_TAG, "leave fill: toplevel {} has no window entry", wl.id()),
        }
        toplevel.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = size.map(Into::into);
        });
        opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} restored to {size:?} for a grab", wl.id());
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}

/// Commit of a toplevel: while it is resized (or right after), places it so
/// the edges opposite to the dragged top / left edges stay where they were,
/// using the size the client committed. Not a mapped toplevel in a resize:
/// nothing to do.
pub fn handle_commit(state: &mut State, surface: &WlSurface) {
    let Some(info) = state.windows.get_mut(surface) else { return };
    let (edges, initial) = match info.resize {
        ResizeTrack::Idle => return,
        ResizeTrack::Resizing { edges, initial } => (edges, initial),
        ResizeTrack::WaitingLastCommit { edges, initial } => {
            info.resize = ResizeTrack::Idle;
            (edges, initial)
        }
    };
    if !edges.left && !edges.top { return; }
    let found = state.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface)).cloned();
    let Some(window) = found else { return };
    let size = window.geometry().size;
    let origin = resized_origin(initial, edges, (size.w, size.h));
    let current = state.space.element_location(&window).map(|p| (p.x, p.y));
    if current == Some(origin) { return; }
    state.move_window(&window, origin);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Edges, Rect};

    #[test]
    fn the_resize_edges_follow_the_quadrant_of_the_pointer() {
        let r = Rect { x: 0, y: 0, w: 400, h: 300 };
        assert_eq!(edges_for_point(r, (10, 10)), Edges { top: true, left: true, ..Edges::default() });
        assert_eq!(edges_for_point(r, (390, 290)), Edges { bottom: true, right: true, ..Edges::default() });
    }

    #[test]
    fn the_other_two_quadrants_pick_the_mixed_corners() {
        let r = Rect { x: 500, y: 200, w: 400, h: 300 };
        assert_eq!(edges_for_point(r, (390, 10)), Edges { top: true, right: true, ..Edges::default() });
        assert_eq!(edges_for_point(r, (10, 290)), Edges { bottom: true, left: true, ..Edges::default() });
    }

    #[test]
    fn a_resize_by_the_top_left_keeps_the_bottom_right_corner() {
        let initial = Rect { x: 100, y: 100, w: 400, h: 300 };
        let edges = Edges { top: true, left: true, ..Edges::default() };
        assert_eq!(resized_origin(initial, edges, (450, 320)), (50, 80));
    }

    #[test]
    fn a_resize_by_the_bottom_right_keeps_the_origin() {
        let initial = Rect { x: 100, y: 100, w: 400, h: 300 };
        let edges = Edges { bottom: true, right: true, ..Edges::default() };
        assert_eq!(resized_origin(initial, edges, (450, 320)), (100, 100));
    }

    #[test]
    fn a_restored_window_stays_under_the_pointer() {
        let maximized = Rect { x: 0, y: 30, w: 1920, h: 1050 };
        // Pointer at the middle of the title bar: it stays in the middle of the 800 wide window.
        assert_eq!(restored_origin(maximized, (800, 600), (960, 40)), (560, 35));
    }

    #[test]
    fn restoring_from_an_empty_geometry_puts_the_origin_at_the_pointer() {
        let empty = Rect { x: 0, y: 0, w: 0, h: 0 };
        assert_eq!(restored_origin(empty, (800, 600), (100, 50)), (100, 50));
    }
}
