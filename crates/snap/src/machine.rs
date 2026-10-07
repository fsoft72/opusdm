//! The drag state machine: turns samples of the pointer and of the active
//! window into zone, placement and restore actions. Pure, so it is unit tested
//! without an X server.

use crate::restore::restore_rect;
use crate::zones::{zone_at, zone_rect, Rect, Zone};
use std::collections::HashMap;

/// Pointer travel in pixels, from where the button went down, after which a
/// snapped window is restored to its pre-snap size.
pub const RESTORE_THRESHOLD: i32 = 8;

/// Window travel in pixels after which a press counts as a window move.
const MOVE_THRESHOLD: i32 = 3;

/// Difference in pixels, per field, under which two rectangles are the same.
const RECT_TOLERANCE: i32 = 2;

/// An X11 window id.
pub type WindowId = u32;

/// One reading of the world.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// Whether the left button is down.
    pub button: bool,
    /// Pointer position in root coordinates.
    pub pointer: (i32, i32),
    /// The active window and its FRAME rectangle; `None` when there is none
    /// or it must be ignored (dock, desktop, not resizable...).
    pub window: Option<(WindowId, Rect)>,
    /// Usable area of the monitor under the pointer.
    pub area: Rect,
}

/// What the caller has to do after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Show the overlay on this zone, or hide it (`None`).
    Zone(Option<Zone>),
    /// Put `window` into `rect`, the rectangle of `zone`.
    Place { window: WindowId, zone: Zone, rect: Rect },
    /// Give `window` back its pre-snap size at `rect`.
    Restore { window: WindowId, rect: Rect },
}

/// A snapped window: its frame rectangle before the snap and right after.
#[derive(Debug, Clone, Copy)]
struct Snapped {
    pre: Rect,
    at: Rect,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Idle,
    /// Button down on `window`, whose frame was `start` then.
    Pressed { window: WindowId, start: Rect, press: (i32, i32) },
    /// The window is being moved; `pre` is what to remember if it is snapped.
    Dragging { window: WindowId, zone: Option<Zone>, pre: Rect },
}

/// The state machine, plus the memory of the windows that are snapped.
pub struct Machine {
    state: State,
    snapped: HashMap<WindowId, Snapped>,
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine {
    /// A machine with no drag in progress and no snapped window.
    pub fn new() -> Self {
        Self { state: State::Idle, snapped: HashMap::new() }
    }

    /// Whether no press or drag is in progress.
    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    /// Forget a window that was closed.
    pub fn forget(&mut self, window: WindowId) {
        self.snapped.remove(&window);
    }

    /// Record the rectangle `window` really has after being placed: it may
    /// differ from the requested one (size increments, minimum size), and the
    /// restore check compares against it.
    pub fn settle(&mut self, window: WindowId, actual: Rect) {
        if let Some(snapped) = self.snapped.get_mut(&window) {
            snapped.at = actual;
        }
    }

    /// Advance with a new `sample` and return the resulting actions.
    pub fn step(&mut self, sample: &Sample) -> Vec<Action> {
        let mut actions = Vec::new();
        match self.state {
            State::Idle => {
                let Some((window, start)) = sample.window.filter(|_| sample.button) else { return actions };
                self.state = State::Pressed { window, start, press: sample.pointer };
            }
            State::Pressed { window, start, press } => {
                if !sample.button {
                    self.state = State::Idle;
                    return actions;
                }
                let Some((current, rect)) = sample.window else {
                    self.state = State::Idle;
                    return actions;
                };
                if current != window {
                    self.state = State::Pressed { window: current, start: rect, press: sample.pointer };
                    return actions;
                }
                if !is_move(start, rect) { return actions; }
                let Some(pre) = self.begin_drag(window, start, press, sample, &mut actions) else { return actions };
                self.state = State::Dragging { window, zone: None, pre };
                self.drag(sample, &mut actions);
            }
            State::Dragging { .. } => self.drag(sample, &mut actions),
        }
        actions
    }

    /// The window moved: decide about the restore. Returns the rectangle to
    /// remember if the window is snapped later, or `None` to wait for the
    /// pointer to travel far enough to restore a snapped window.
    fn begin_drag(&mut self, window: WindowId, start: Rect, press: (i32, i32), sample: &Sample, actions: &mut Vec<Action>) -> Option<Rect> {
        let Some(snapped) = self.snapped.get(&window).copied() else { return Some(start) };
        if !same_rect(snapped.at, start) {
            self.snapped.remove(&window);
            return Some(start);
        }
        let travel = (sample.pointer.0 - press.0).abs().max((sample.pointer.1 - press.1).abs());
        if travel <= RESTORE_THRESHOLD { return None; }
        self.snapped.remove(&window);
        let rect = restore_rect(snapped.pre, snapped.at, press, sample.pointer, sample.area);
        actions.push(Action::Restore { window, rect });
        Some(rect)
    }

    /// One step of a drag in progress: follow the zone, or finish on release.
    fn drag(&mut self, sample: &Sample, actions: &mut Vec<Action>) {
        let State::Dragging { window, zone, pre } = self.state else { return };
        if sample.button {
            let now = zone_at(sample.pointer, sample.area);
            if now != zone {
                actions.push(Action::Zone(now));
                self.state = State::Dragging { window, zone: now, pre };
            }
            return;
        }
        self.state = State::Idle;
        let Some(zone) = zone else { return };
        let rect = zone_rect(zone, sample.area);
        actions.push(Action::Zone(None));
        actions.push(Action::Place { window, zone, rect });
        self.snapped.insert(window, Snapped { pre, at: rect });
    }
}

/// Whether `now` is `start` moved, not resized.
fn is_move(start: Rect, now: Rect) -> bool {
    start.w == now.w
        && start.h == now.h
        && ((now.x - start.x).abs() >= MOVE_THRESHOLD || (now.y - start.y).abs() >= MOVE_THRESHOLD)
}

/// Whether two rectangles are the same, within [`RECT_TOLERANCE`].
fn same_rect(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= RECT_TOLERANCE
        && (a.y - b.y).abs() <= RECT_TOLERANCE
        && (a.w - b.w).abs() <= RECT_TOLERANCE
        && (a.h - b.h).abs() <= RECT_TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0, y: 30, w: 1920, h: 1050 };
    const W: WindowId = 7;
    const START: Rect = Rect { x: 400, y: 300, w: 600, h: 400 };

    fn sample(button: bool, pointer: (i32, i32), window: Rect) -> Sample {
        Sample { button, pointer, window: Some((W, window)), area: AREA }
    }

    /// `START` moved by `(dx, dy)`.
    fn moved(dx: i32, dy: i32) -> Rect {
        Rect::new(START.x + dx, START.y + dy, START.w, START.h)
    }

    /// Press on the title bar and drag the window to `pointer`, as a window
    /// manager would: the window follows the pointer. Returns every action.
    fn drag_to(m: &mut Machine, from: Rect, pointer: (i32, i32)) -> Vec<Action> {
        let press = (from.x + 100, from.y + 10);
        let mut out = m.step(&sample(true, press, from));
        let shifted = Rect::new(from.x + pointer.0 - press.0, from.y + pointer.1 - press.1, from.w, from.h);
        out.extend(m.step(&sample(true, pointer, shifted)));
        out
    }

    /// Release at `pointer`.
    fn release(m: &mut Machine, pointer: (i32, i32), window: Rect) -> Vec<Action> {
        m.step(&sample(false, pointer, window))
    }

    #[test]
    fn idle_with_the_button_up_does_nothing() {
        let mut m = Machine::new();
        assert!(m.step(&sample(false, (10, 10), START)).is_empty());
    }

    #[test]
    fn a_press_without_a_window_move_does_nothing() {
        // Rubber band selection or a click inside the window.
        let mut m = Machine::new();
        assert!(m.step(&sample(true, (500, 400), START)).is_empty());
        assert!(m.step(&sample(true, (5, 600), START)).is_empty());
        assert!(release(&mut m, (5, 600), START).is_empty());
    }

    #[test]
    fn a_resize_is_not_a_drag() {
        let mut m = Machine::new();
        m.step(&sample(true, (1000, 700), START));
        let resized = Rect::new(START.x, START.y, 900, 700);
        assert!(m.step(&sample(true, (5, 600), resized)).is_empty());
        assert!(release(&mut m, (5, 600), resized).is_empty());
    }

    #[test]
    fn an_ignored_window_does_nothing() {
        let mut m = Machine::new();
        let ignored = Sample { window: None, ..sample(true, (5, 600), START) };
        assert!(m.step(&ignored).is_empty());
        assert!(m.step(&ignored).is_empty());
    }

    #[test]
    fn dragging_to_an_edge_shows_the_zone_then_places_on_release() {
        let mut m = Machine::new();
        assert!(drag_to(&mut m, START, (500, 400)).is_empty());
        let shown = m.step(&sample(true, (5, 600), moved(-400, 200)));
        assert_eq!(shown, vec![Action::Zone(Some(Zone::Left))]);
        let end = release(&mut m, (5, 600), moved(-400, 200));
        assert_eq!(
            end,
            vec![
                Action::Zone(None),
                Action::Place { window: W, zone: Zone::Left, rect: Rect::new(0, 30, 960, 1050) },
            ]
        );
    }

    #[test]
    fn the_zone_is_reported_on_the_step_the_drag_starts() {
        let mut m = Machine::new();
        let out = drag_to(&mut m, START, (5, 600));
        assert_eq!(out, vec![Action::Zone(Some(Zone::Left))]);
    }

    #[test]
    fn leaving_the_zone_hides_the_overlay_and_releasing_places_nothing() {
        let mut m = Machine::new();
        drag_to(&mut m, START, (5, 600));
        assert_eq!(m.step(&sample(true, (600, 600), moved(0, 0))), vec![Action::Zone(None)]);
        assert!(release(&mut m, (600, 600), moved(0, 0)).is_empty());
    }

    #[test]
    fn dragging_a_snapped_window_far_enough_restores_its_size() {
        let mut m = Machine::new();
        drag_to(&mut m, START, (5, 600));
        let snapped = zone_rect(Zone::Left, AREA);
        release(&mut m, (5, 600), snapped);

        // Press on the snapped window's title bar, then drag away.
        m.step(&sample(true, (480, 40), snapped));
        let nudge = Rect::new(snapped.x + 5, snapped.y, snapped.w, snapped.h);
        assert!(m.step(&sample(true, (485, 40), nudge)).is_empty(), "under the threshold nothing happens");
        let away = Rect::new(snapped.x + 200, snapped.y + 100, snapped.w, snapped.h);
        let out = m.step(&sample(true, (680, 140), away));
        let Some(Action::Restore { window, rect }) = out.first().copied() else { panic!("no restore in {out:?}") };
        assert_eq!(window, W);
        assert_eq!((rect.w, rect.h), (START.w, START.h));
    }

    #[test]
    fn a_stale_snapped_entry_is_not_restored() {
        let mut m = Machine::new();
        drag_to(&mut m, START, (5, 600));
        release(&mut m, (5, 600), zone_rect(Zone::Left, AREA));
        // The window was moved by other means before the next drag.
        let elsewhere = Rect::new(700, 200, 960, 1050);
        m.step(&sample(true, (780, 210), elsewhere));
        let away = Rect::new(900, 300, 960, 1050);
        assert!(m.step(&sample(true, (980, 310), away)).is_empty());
    }

    #[test]
    fn snapping_again_keeps_the_original_size() {
        let mut m = Machine::new();
        drag_to(&mut m, START, (5, 600));
        let left = zone_rect(Zone::Left, AREA);
        release(&mut m, (5, 600), left);

        // Drag away (restore), then snap to the right.
        m.step(&sample(true, (480, 40), left));
        let away = Rect::new(200, 130, left.w, left.h);
        let restored = m.step(&sample(true, (680, 140), away));
        assert!(matches!(restored[0], Action::Restore { .. }));
        let edge = m.step(&sample(true, (1915, 600), away));
        assert_eq!(edge, vec![Action::Zone(Some(Zone::Right))]);
        release(&mut m, (1915, 600), away);

        // Drag away again: the size is still the very first one.
        let right = zone_rect(Zone::Right, AREA);
        m.step(&sample(true, (1400, 40), right));
        let out = m.step(&sample(true, (1200, 140), Rect::new(right.x - 200, right.y + 100, right.w, right.h)));
        let Some(Action::Restore { rect, .. }) = out.first().copied() else { panic!("no restore in {out:?}") };
        assert_eq!((rect.w, rect.h), (START.w, START.h));
    }

    #[test]
    fn a_window_that_could_not_take_the_exact_size_still_restores_after_settle() {
        let mut m = Machine::new();
        drag_to(&mut m, START, (5, 600));
        release(&mut m, (5, 600), zone_rect(Zone::Left, AREA));
        // A terminal snaps its size to character cells.
        let actual = Rect::new(0, 30, 954, 1043);
        m.settle(W, actual);
        m.step(&sample(true, (480, 40), actual));
        let out = m.step(&sample(true, (680, 140), Rect::new(200, 130, actual.w, actual.h)));
        assert!(matches!(out.first(), Some(Action::Restore { .. })));
    }

    #[test]
    fn a_forgotten_window_is_not_restored() {
        let mut m = Machine::new();
        drag_to(&mut m, START, (5, 600));
        let left = zone_rect(Zone::Left, AREA);
        release(&mut m, (5, 600), left);
        m.forget(W);
        m.step(&sample(true, (480, 40), left));
        assert!(m.step(&sample(true, (680, 140), Rect::new(200, 130, left.w, left.h))).is_empty());
    }

    #[test]
    fn pressing_another_window_starts_over() {
        let mut m = Machine::new();
        m.step(&sample(true, (500, 310), START));
        // The click focused another window, which is the one that moves.
        let other = Rect::new(50, 50, 300, 300);
        let first = Sample { window: Some((9, other)), ..sample(true, (60, 60), other) };
        assert!(m.step(&first).is_empty());
        let moved_other = Rect::new(60, 60, 300, 300);
        let out = m.step(&Sample { window: Some((9, moved_other)), ..sample(true, (5, 600), moved_other) });
        assert_eq!(out, vec![Action::Zone(Some(Zone::Left))]);
        let end = m.step(&Sample { window: Some((9, moved_other)), ..sample(false, (5, 600), moved_other) });
        assert!(matches!(end[1], Action::Place { window: 9, .. }));
    }
}
