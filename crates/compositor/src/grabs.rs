//! Interactive move and resize grabs of the pointer.
//!
//! A grab is started by Alt + click (`input.rs`) or by a client request
//! (`xdg_toplevel.move` / `resize`, `shell/xdg.rs`). While it is active the
//! pointer has no client focus; it ends when the button that started it is
//! released (or when another grab replaces it).
//!
//! A move grab snaps its window when it is released in a snap zone
//! (`snap.rs`); meanwhile `State::snap_preview` holds the zone rectangle
//! (drawn by the nested backend) and is cleared when the grab ends. The
//! grab holds a `Window` clone only for the drag; the table owns it.

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

use crate::decor::geometry::frame_rect;
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
    /// The grab ended and the final configure (serial `serial`) went out;
    /// commits are still placed (they carry the sizes of the resize) until
    /// the client acks it. After `commits_left` more commits without that
    /// ack the track gives up (a client that never acks cannot keep it).
    WaitingFinalAck {
        /// The edges that were dragged.
        edges: Edges,
        /// Geometry of the window when the resize started.
        initial: Rect,
        /// Serial of the final configure (no `Resizing` state).
        serial: Serial,
        /// Commits still placed before giving up on the ack.
        commits_left: u32,
    },
    /// The final configure was acked: its commit is placed, then `Idle`.
    WaitingFinalCommit {
        /// The edges that were dragged.
        edges: Edges,
        /// Geometry of the window when the resize started.
        initial: Rect,
    },
}

/// Commits placed after the end of a resize while the client has not acked
/// the final configure; then the track goes back to `Idle`.
pub const RESIZE_ACK_COMMIT_LIMIT: u32 = 30;

/// What a commit does to a resize track: the next track, the drag to place
/// the committed size with (if any) and whether the track gave up waiting
/// for the final ack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitStep {
    /// The track after the commit.
    pub next: ResizeTrack,
    /// Edges and initial geometry to place the committed size with.
    pub place: Option<(Edges, Rect)>,
    /// True when the final ack never came within `RESIZE_ACK_COMMIT_LIMIT`.
    pub gave_up: bool,
}

/// Decides what a commit of the window does with its resize `track`.
pub fn resize_track_on_commit(track: ResizeTrack) -> CommitStep {
    match track {
        ResizeTrack::Idle => CommitStep { next: track, place: None, gave_up: false },
        ResizeTrack::Resizing { edges, initial } => CommitStep { next: track, place: Some((edges, initial)), gave_up: false },
        ResizeTrack::WaitingFinalAck { edges, initial, serial, commits_left } => {
            let Some(left) = commits_left.checked_sub(1) else {
                return CommitStep { next: ResizeTrack::Idle, place: Some((edges, initial)), gave_up: true };
            };
            let next = ResizeTrack::WaitingFinalAck { edges, initial, serial, commits_left: left };
            CommitStep { next, place: Some((edges, initial)), gave_up: false }
        }
        ResizeTrack::WaitingFinalCommit { edges, initial } => {
            CommitStep { next: ResizeTrack::Idle, place: Some((edges, initial)), gave_up: false }
        }
    }
}

/// Decides what the ack of the configure `acked` does with the resize
/// `track`: the final configure (or a later one) acked moves it to
/// `WaitingFinalCommit`; anything else keeps it.
pub fn resize_track_on_ack(track: ResizeTrack, acked: Serial) -> ResizeTrack {
    match track {
        ResizeTrack::WaitingFinalAck { edges, initial, serial, .. } if acked >= serial => {
            ResizeTrack::WaitingFinalCommit { edges, initial }
        }
        other => other,
    }
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

/// Distance in pixels (on either axis) the pointer must move from the press
/// before a move grab moves its window (Openbox's drag threshold): a click
/// leaves the window as it was, a maximized, fullscreen or snapped window is
/// restored only once it is really dragged.
pub const MOVE_DRAG_THRESHOLD: f64 = 8.0;

/// True when the pointer moved by `delta` (from the press) is past
/// `MOVE_DRAG_THRESHOLD` on either axis.
pub fn past_drag_threshold(delta: (f64, f64)) -> bool {
    delta.0.abs() >= MOVE_DRAG_THRESHOLD || delta.1.abs() >= MOVE_DRAG_THRESHOLD
}

/// Pointer grab that moves `window` with the pointer once it is dragged past
/// `MOVE_DRAG_THRESHOLD`; released in a snap zone it snaps the window there.
pub struct MoveGrab {
    /// Pointer state when the grab started.
    pub start_data: GrabStartData<State>,
    /// The window being moved.
    pub window: Window,
    /// Position of the window when the grab started.
    pub initial_loc: Point<i32, Logical>,
    /// Size of the window during the drag: its size at the start, or the
    /// size it was restored to (maximized, fullscreen or snapped), which the
    /// client may not have committed yet. Kept as the size to restore when
    /// the window snaps.
    pub size: (i32, i32),
    /// True once the pointer went past `MOVE_DRAG_THRESHOLD`: the window
    /// follows the pointer and a release may snap it. Before that the grab
    /// changes nothing.
    pub dragging: bool,
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
    /// Once the pointer is past `MOVE_DRAG_THRESHOLD` from the press (the
    /// first time: a maximized, fullscreen or snapped window is restored,
    /// `State::restore_for_drag`), moves the window by the pointer delta
    /// from the start of the grab and shows the preview of the snap zone
    /// under the pointer; no client has the pointer focus meanwhile. A dead
    /// or unmapped window ends the grab.
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
        if data.space.element_location(&self.window).is_none() {
            opusdm_ipc::debug_log!(LOG_TAG, "move grab: the window is not mapped any more, grab ended");
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        let delta = event.location - self.start_data.location;
        if !self.dragging {
            if !past_drag_threshold((delta.x, delta.y)) { return; }
            self.dragging = true;
            if let Some((loc, size)) = data.restore_for_drag(&self.window, self.start_data.location) {
                self.initial_loc = loc;
                self.size = size;
            }
        }
        let loc = (self.initial_loc.to_f64() + delta).to_i32_round();
        data.move_window(&self.window, (loc.x, loc.y));
        let zone = data.snap_zone_at(event.location);
        data.set_snap_preview(zone);
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
    /// the grab ends it, snapping the window when it was dragged and the
    /// pointer is in a snap zone (a dead window is not snapped; a release
    /// before the drag threshold changes nothing).
    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        if handle.current_pressed().contains(&self.start_data.button) { return; }
        let zone = if self.dragging { data.snap_zone_at(handle.current_location()) } else { None };
        match zone {
            Some(zone) if self.window.alive() => data.snap_window(&self.window, zone, self.size),
            Some(zone) => opusdm_ipc::debug_log!(LOG_TAG, "move grab: released in {zone:?}, the window is gone"),
            None => {}
        }
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

    /// The move ended (released, replaced, or its window went): the snap
    /// preview goes away.
    fn unset(&mut self, data: &mut State) {
        data.set_snap_preview(None);
        opusdm_ipc::debug_log!(LOG_TAG, "move grab ended");
    }
}

impl PointerGrab<State> for ResizeGrab {
    /// Computes the new geometry from the pointer delta (never under
    /// `MIN_WINDOW_SIZE`) and sends its size with the `Resizing` state; the
    /// commits place the window (`handle_commit`). A dead or unmapped window
    /// ends the grab.
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
        if data.space.element_location(&self.window).is_none() {
            opusdm_ipc::debug_log!(LOG_TAG, "resize grab: the window is not mapped any more, grab ended");
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
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
    /// size without the `Resizing` state and waits for its ack (the commits
    /// until then are still placed, see `ResizeTrack::WaitingFinalAck`). A
    /// dead window has nothing to configure; an unmapped one only loses the
    /// pending `Resizing` state (its next initial configure carries it) and
    /// its track goes back to `Idle`.
    fn unset(&mut self, data: &mut State) {
        let Some(toplevel) = self.window.toplevel().filter(|t| t.alive()).cloned() else {
            opusdm_ipc::debug_log!(LOG_TAG, "resize grab ended: the window is gone");
            return;
        };
        let (w, h) = self.last_size;
        let mapped = data.space.element_location(&self.window).is_some();
        toplevel.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Resizing);
            if mapped {
                state.size = Some((w, h).into());
            }
        });
        let wl = toplevel.wl_surface();
        let Some(info) = data.windows.get_mut(wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "resize grab ended: toplevel {} has no window entry", wl.id());
            return;
        };
        if !mapped {
            info.resize = ResizeTrack::Idle;
            opusdm_ipc::debug_log!(LOG_TAG, "resize grab of {} ended: the window is not mapped, no final configure", wl.id());
            return;
        }
        let serial = toplevel.send_configure();
        info.resize = ResizeTrack::WaitingFinalAck {
            edges: self.edges,
            initial: self.initial,
            serial,
            commits_left: RESIZE_ACK_COMMIT_LIMIT,
        };
        opusdm_ipc::debug_log!(LOG_TAG, "resize grab of {} ended at {w}x{h}, final configure {serial:?}", wl.id());
    }
}

impl State {
    /// Starts a `MoveGrab` of the mapped `window`. Nothing changes until the
    /// pointer is dragged past `MOVE_DRAG_THRESHOLD` (`MoveGrab::motion`).
    /// Refused with a log when the seat has no pointer or the window is not
    /// a mapped toplevel.
    pub fn start_move_grab(&mut self, window: Window, start_data: GrabStartData<State>, serial: Serial) {
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "move grab refused: the seat has no pointer");
            return;
        };
        let Some(toplevel) = window.toplevel().cloned() else {
            opusdm_ipc::err_log!(LOG_TAG, "move grab refused: the window has no toplevel");
            return;
        };
        let Some(loc) = self.space.element_location(&window) else {
            opusdm_ipc::err_log!(LOG_TAG, "move grab refused: toplevel {} is not mapped", toplevel.wl_surface().id());
            return;
        };
        let geo = window.geometry().size;
        opusdm_ipc::debug_log!(LOG_TAG, "move grab of {} from {},{}", toplevel.wl_surface().id(), loc.x, loc.y);
        let grab = MoveGrab { start_data, window, initial_loc: loc, size: (geo.w, geo.h), dragging: false };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Called when a move grab of `window` goes past the drag threshold: a
    /// maximized, fullscreen or snapped window leaves that state, gets a
    /// configure of its restore size and is placed so the point under
    /// `press` (the pointer at the press) keeps its relative position in
    /// the frame (the client rectangle of a window without one). Returns
    /// the new client origin and size; `None` when the window is in the
    /// normal state or was restored to no stored size (the client decides),
    /// or is not a mapped toplevel in the table (logged).
    pub fn restore_for_drag(&mut self, window: &Window, press: Point<f64, Logical>) -> Option<(Point<i32, Logical>, (i32, i32))> {
        let Some(toplevel) = window.toplevel().cloned() else {
            opusdm_ipc::err_log!(LOG_TAG, "move drag: the window has no toplevel, not restored");
            return None;
        };
        let wl = toplevel.wl_surface();
        let Some(info) = self.windows.get(wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "move drag: toplevel {} has no window entry, not restored", wl.id());
            return None;
        };
        if !info.maximized && !info.fullscreen && info.snapped.is_none() { return None; }
        let restore = info.restore.map(|r| (r.w, r.h));
        let Some(loc) = self.space.element_location(window) else {
            opusdm_ipc::err_log!(LOG_TAG, "move drag: toplevel {} is not mapped, not restored", wl.id());
            return None;
        };
        let geo = window.geometry().size;
        // The frame now (a fullscreen window has none), then the insets of the normal state.
        let current = self.frame_rect_of(wl, Rect { x: loc.x, y: loc.y, w: geo.w, h: geo.h });
        self._leave_fill(&toplevel, restore);
        let restored = restore?;
        let insets = self.insets_of(wl);
        let restored_frame = frame_rect(Rect { x: 0, y: 0, w: restored.0, h: restored.1 }, insets);
        let p = press.to_i32_round();
        let (fx, fy) = restored_origin(current, (restored_frame.w, restored_frame.h), (p.x, p.y));
        let origin = (fx.saturating_add(insets.left), fy.saturating_add(insets.top));
        self.move_window(window, origin);
        self.notify_watchers();
        Some((origin.into(), restored))
    }

    /// Starts a `ResizeGrab` of the mapped `window` by its `edges`. A
    /// maximized or fullscreen window leaves that state first and is resized
    /// from its current geometry; a snapped one is no longer snapped (its
    /// size to restore is dropped). Refused with a log when no edge is given,
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
        if info.snapped.take().is_some() {
            info.restore = None;
            opusdm_ipc::debug_log!(LOG_TAG, "resize of {}: no longer snapped", wl.id());
        }
        let filled = info.maximized || info.fullscreen;
        if filled {
            self._leave_fill(&toplevel, Some((initial.w, initial.h)));
        }
        opusdm_ipc::debug_log!(LOG_TAG, "resize grab of {} {edges:?} from {initial:?}", wl.id());
        let grab = ResizeGrab { start_data, window, edges, initial, last_size: (initial.w, initial.h) };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Takes the toplevel out of maximized, fullscreen and snapped for an
    /// interactive move or resize: clears the three and the stored restore
    /// geometry, unsets both states and configures `size` (`None`: the
    /// client decides).
    fn _leave_fill(&mut self, toplevel: &ToplevelSurface, size: Option<(i32, i32)>) {
        let wl = toplevel.wl_surface();
        match self.windows.get_mut(wl) {
            Some(info) => {
                info.maximized = false;
                info.fullscreen = false;
                info.snapped = None;
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

/// Commit of a toplevel: while it is resized (and until the client has
/// committed the final configure of the resize, see `ResizeTrack`), places it
/// so the edges opposite to the dragged top / left edges stay where they
/// were, using the size the client committed. A track that gave up waiting
/// for the final ack, or whose window is no longer mapped, goes back to
/// `Idle` with a debug trace. Not a toplevel in a resize: nothing to do.
pub fn handle_commit(state: &mut State, surface: &WlSurface) {
    let Some(info) = state.windows.get_mut(surface) else { return };
    if info.resize == ResizeTrack::Idle { return; }
    let step = resize_track_on_commit(info.resize);
    info.resize = step.next;
    if step.gave_up {
        opusdm_ipc::debug_log!(LOG_TAG, "resize of {}: final configure never acked, placement stopped", surface.id());
    }
    let Some((edges, initial)) = step.place else { return };
    let found = state.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface)).cloned();
    let Some(window) = found else {
        opusdm_ipc::debug_log!(LOG_TAG, "resize of {}: the window is not mapped, placement stopped", surface.id());
        if let Some(info) = state.windows.get_mut(surface) {
            info.resize = ResizeTrack::Idle;
        }
        return;
    };
    if !edges.left && !edges.top { return; }
    let size = window.geometry().size;
    let origin = resized_origin(initial, edges, (size.w, size.h));
    let current = state.space.element_location(&window).map(|p| (p.x, p.y));
    if current == Some(origin) { return; }
    state.move_window(&window, origin);
}

/// Ack of a toplevel configure: moves the resize track of `surface` on when
/// the final configure of a resize is acked (`resize_track_on_ack`).
pub fn handle_ack(state: &mut State, surface: &WlSurface, acked: Serial) {
    let Some(info) = state.windows.get_mut(surface) else { return };
    let next = resize_track_on_ack(info.resize, acked);
    if next == info.resize { return; }
    opusdm_ipc::debug_log!(LOG_TAG, "resize of {}: final configure acked ({acked:?})", surface.id());
    info.resize = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Edges, Rect};

    #[test]
    fn the_drag_threshold_is_8_pixels_on_either_axis() {
        assert!(!past_drag_threshold((0.0, 0.0)));
        assert!(!past_drag_threshold((7.9, -7.9)));
        assert!(past_drag_threshold((8.0, 0.0)));
        assert!(past_drag_threshold((0.0, -8.0)));
    }

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

    const TOP_LEFT: Edges = Edges { top: true, left: true, bottom: false, right: false };
    const INITIAL: Rect = Rect { x: 100, y: 100, w: 400, h: 300 };

    /// The track right after a grab ended with final configure serial 10.
    fn waiting(commits_left: u32) -> ResizeTrack {
        ResizeTrack::WaitingFinalAck { edges: TOP_LEFT, initial: INITIAL, serial: Serial::from(10), commits_left }
    }

    #[test]
    fn commits_before_the_final_ack_are_placed_and_keep_waiting() {
        let step = resize_track_on_commit(waiting(RESIZE_ACK_COMMIT_LIMIT));
        assert_eq!(step.place, Some((TOP_LEFT, INITIAL)));
        assert_eq!(step.next, waiting(RESIZE_ACK_COMMIT_LIMIT - 1));
        assert!(!step.gave_up);
    }

    #[test]
    fn an_older_ack_does_not_end_the_wait() {
        assert_eq!(resize_track_on_ack(waiting(5), Serial::from(9)), waiting(5));
    }

    #[test]
    fn the_final_ack_then_its_commit_end_the_resize() {
        let acked = resize_track_on_ack(waiting(5), Serial::from(10));
        assert_eq!(acked, ResizeTrack::WaitingFinalCommit { edges: TOP_LEFT, initial: INITIAL });
        let step = resize_track_on_commit(acked);
        assert_eq!(step.place, Some((TOP_LEFT, INITIAL)));
        assert_eq!(step.next, ResizeTrack::Idle);
        assert!(!step.gave_up);
    }

    #[test]
    fn a_later_ack_also_ends_the_wait() {
        let acked = resize_track_on_ack(waiting(5), Serial::from(12));
        assert_eq!(acked, ResizeTrack::WaitingFinalCommit { edges: TOP_LEFT, initial: INITIAL });
    }

    #[test]
    fn a_client_that_never_acks_cannot_keep_the_track() {
        let mut track = waiting(RESIZE_ACK_COMMIT_LIMIT);
        let mut commits = 0;
        while track != ResizeTrack::Idle {
            let step = resize_track_on_commit(track);
            commits += 1;
            assert_eq!(step.gave_up, step.next == ResizeTrack::Idle);
            track = step.next;
            assert!(commits <= RESIZE_ACK_COMMIT_LIMIT + 1, "the track never gave up");
        }
        assert_eq!(commits, RESIZE_ACK_COMMIT_LIMIT + 1);
        assert_eq!(resize_track_on_commit(track).place, None);
    }

    #[test]
    fn commits_while_resizing_are_placed_and_idle_is_left_alone() {
        let resizing = ResizeTrack::Resizing { edges: TOP_LEFT, initial: INITIAL };
        assert_eq!(resize_track_on_commit(resizing), CommitStep { next: resizing, place: Some((TOP_LEFT, INITIAL)), gave_up: false });
        assert_eq!(resize_track_on_commit(ResizeTrack::Idle), CommitStep { next: ResizeTrack::Idle, place: None, gave_up: false });
        assert_eq!(resize_track_on_ack(resizing, Serial::from(99)), resizing);
    }

    #[test]
    fn restoring_from_an_empty_geometry_puts_the_origin_at_the_pointer() {
        let empty = Rect { x: 0, y: 0, w: 0, h: 0 };
        assert_eq!(restored_origin(empty, (800, 600), (100, 50)), (100, 50));
    }
}
