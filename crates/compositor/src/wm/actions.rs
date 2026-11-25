//! Built-in window manager actions run by the key bindings of
//! `compositor.json` (Openbox `rc.xml` keybinds): close, desktop steps,
//! send-and-follow, show desktop, lower, directional focus and the switcher.
//!
//! The pure parts (`relative_desktop`, `directional_target`) have no Smithay
//! type; the `State` methods change the window table and call `sync_space`.

use crate::geometry::Rect;
use crate::state::{State, LOG_TAG};
use crate::wm::WindowId;

/// A built-in action bound to a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Window switcher, next window of the current desktop.
    SwitcherNext,
    /// Window switcher, previous window of the current desktop.
    SwitcherPrev,
    /// Window switcher over the windows of every desktop.
    SwitcherAll,
    /// Asks the active window to close.
    Close,
    /// Shows the desktop on the left.
    DesktopLeft,
    /// Shows the desktop on the right.
    DesktopRight,
    /// Shows the desktop with this 0-based index.
    Desktop(u32),
    /// Moves the active window to the desktop on the left and follows it.
    SendLeft,
    /// Moves the active window to the desktop on the right and follows it.
    SendRight,
    /// Minimizes every window of the desktop, or restores them.
    ShowDesktop,
    /// Puts the active window at the bottom of the stack.
    Lower,
    /// Focuses the nearest window in a direction.
    Focus(Direction),
}

/// A direction of the directional focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Towards the left edge.
    Left,
    /// Towards the right edge.
    Right,
    /// Towards the top edge.
    Up,
    /// Towards the bottom edge.
    Down,
}

/// The desktop `delta` steps from `current` among `count` desktops: with
/// `wrap` it goes around, without it stops at the first / last desktop.
pub fn relative_desktop(current: u32, count: u32, delta: i32, wrap: bool) -> u32 {
    if count == 0 { return 0; }
    let target = i64::from(current) + i64::from(delta);
    let count = i64::from(count);
    let index = if wrap { target.rem_euclid(count) } else { target.clamp(0, count - 1) };
    u32::try_from(index).unwrap_or(0)
}

/// The window of `candidates` nearest to `from` whose centre lies in the
/// half-plane of `dir` (strictly), by centre distance. `candidates` come in
/// stacking order, top first: on equal distance the first one wins.
pub fn directional_target(from: Rect, candidates: &[(WindowId, Rect)], dir: Direction) -> Option<WindowId> {
    let (fx, fy) = _double_centre(from);
    let mut best: Option<(WindowId, i64)> = None;
    for (id, rect) in candidates {
        let (cx, cy) = _double_centre(*rect);
        let inside = match dir {
            Direction::Left => cx < fx,
            Direction::Right => cx > fx,
            Direction::Up => cy < fy,
            Direction::Down => cy > fy,
        };
        if !inside { continue; }
        let distance = (cx - fx).pow(2) + (cy - fy).pow(2);
        if best.is_none_or(|(_, d)| distance < d) {
            best = Some((*id, distance));
        }
    }
    best.map(|(id, _)| id)
}

/// Twice the centre of `rect` (whole numbers, no rounding), as `i64`.
fn _double_centre(rect: Rect) -> (i64, i64) {
    (2 * i64::from(rect.x) + i64::from(rect.w), 2 * i64::from(rect.y) + i64::from(rect.h))
}

impl State {
    /// Runs the built-in `action`. The actions on a window act on the
    /// active window; with none they only write a `debug_log!` line. A
    /// failure is logged with `err_log!`.
    pub fn run_action(&mut self, action: Action) {
        let result = match action {
            Action::SwitcherNext => {
                self.open_switcher(false, false);
                Ok(())
            }
            Action::SwitcherPrev => {
                self.open_switcher(false, true);
                Ok(())
            }
            Action::SwitcherAll => {
                self.open_switcher(true, false);
                Ok(())
            }
            Action::DesktopLeft => self._step_desktop(-1),
            Action::DesktopRight => self._step_desktop(1),
            Action::Desktop(index) => self._desktop_action(index),
            Action::ShowDesktop => {
                self.toggle_show_desktop();
                Ok(())
            }
            Action::Close | Action::SendLeft | Action::SendRight | Action::Lower | Action::Focus(_) => {
                let Some(id) = self.active_window else {
                    opusdm_ipc::debug_log!(LOG_TAG, "{}: no active window, nothing to do", _action_name(action));
                    return;
                };
                self._window_action(action, id)
            }
        };
        if let Err(e) = result {
            opusdm_ipc::err_log!(LOG_TAG, "{} failed: {e}", _action_name(action));
        }
    }

    /// The window switcher (Task 4). For now only a `debug_log!` line:
    /// `all` covers every desktop, `backwards` starts with the previous window.
    pub fn open_switcher(&mut self, all: bool, backwards: bool) {
        opusdm_ipc::debug_log!(LOG_TAG, "switcher (all {all}, backwards {backwards}): not implemented yet");
    }

    /// Puts the window `id` at the bottom of the stack (Openbox `Lower`,
    /// `FocusToBottom`, `Unfocus`). When it was shown, the keyboard goes to
    /// the new topmost shown window (itself when it is alone), as after a
    /// desktop switch.
    pub fn lower_window(&mut self, id: WindowId) -> Result<(), String> {
        let (_, info) = self.window_by_id(id).ok_or_else(|| format!("lower: unknown window {}", id.0))?;
        let was_shown = self.space.element_location(&info.window).is_some();
        self.stack.retain(|i| *i != id);
        self.stack.insert(0, id);
        self.sync_space();
        if was_shown {
            self._focus_desktop_top();
        }
        opusdm_ipc::debug_log!(LOG_TAG, "window {} lowered", id.0);
        self.notify_watchers();
        Ok(())
    }

    /// Show desktop (Openbox `ToggleShowDesktop`): minimizes every shown
    /// window of the current desktop and remembers them; the next toggle
    /// restores those still alive and gives the keyboard to the topmost.
    /// Leaving the mode any other way (`leave_show_desktop`) restores nothing.
    pub fn toggle_show_desktop(&mut self) {
        if let Some(ids) = self.show_desktop.take() {
            let mut restored = 0;
            for id in &ids {
                let Some(info) = self._info_by_id_mut(*id) else {
                    opusdm_ipc::debug_log!(LOG_TAG, "show desktop: window {} is gone, not restored", id.0);
                    continue;
                };
                info.minimized = false;
                restored += 1;
            }
            self.sync_space();
            self._focus_desktop_top();
            opusdm_ipc::debug_log!(LOG_TAG, "show desktop off: {restored} of {} windows restored", ids.len());
            self.notify_watchers();
            return;
        }
        let desktops = self.desktops;
        let ids: Vec<WindowId> = self
            .stack
            .iter()
            .copied()
            .filter(|id| self.window_by_id(*id).is_some_and(|(_, info)| Self::is_shown(info, &desktops)))
            .collect();
        for id in &ids {
            if let Some(info) = self._info_by_id_mut(*id) {
                info.minimized = true;
            }
        }
        opusdm_ipc::debug_log!(LOG_TAG, "show desktop on: {} windows minimized", ids.len());
        self.show_desktop = Some(ids);
        self.sync_space();
        self.notify_watchers();
    }

    /// Leaves show-desktop mode without restoring its windows (a window
    /// was activated or mapped, or the desktop changed); `why` names the
    /// cause in the log. Nothing when the mode is off.
    pub fn leave_show_desktop(&mut self, why: &str) {
        let Some(ids) = self.show_desktop.take() else { return };
        opusdm_ipc::debug_log!(LOG_TAG, "show desktop left ({why}): {} windows stay minimized", ids.len());
    }

    /// Moves the window `id` `delta` desktops away from its own (wrap per
    /// `WmSettings`) and follows it: its desktop is shown and it becomes the
    /// active window. At an edge without wrap nothing changes.
    pub fn send_and_follow(&mut self, id: WindowId, delta: i32) -> Result<(), String> {
        let (_, info) = self.window_by_id(id).ok_or_else(|| format!("send: unknown window {}", id.0))?;
        let from = info.desktop;
        let target = relative_desktop(from, self.desktops.count(), delta, self.wm.wrap_desktops);
        if target == from {
            opusdm_ipc::debug_log!(LOG_TAG, "send: window {} stays on desktop {from} (edge, no wrap)", id.0);
            return Ok(());
        }
        // Not `move_to_desktop`: its `sync_space` would hand the keyboard to another window of the old
        // desktop for a moment; `activate_window` shows the new desktop and focuses the window at once.
        let info = self._info_by_id_mut(id).ok_or_else(|| format!("send: unknown window {}", id.0))?;
        info.desktop = target;
        opusdm_ipc::debug_log!(LOG_TAG, "send: window {} moved to desktop {target}, following it", id.0);
        self.activate_window(id)
    }

    /// Shows the desktop `delta` steps from the current one (wrap per
    /// `WmSettings`); at an edge without wrap nothing changes.
    fn _step_desktop(&mut self, delta: i32) -> Result<(), String> {
        let current = self.desktops.current();
        let target = relative_desktop(current, self.desktops.count(), delta, self.wm.wrap_desktops);
        if target == current {
            opusdm_ipc::debug_log!(LOG_TAG, "desktop step {delta}: stays on desktop {current} (edge, no wrap)");
            return Ok(());
        }
        self.switch_desktop(target)
    }

    /// Shows the desktop `index`; an index past the count is an error.
    fn _desktop_action(&mut self, index: u32) -> Result<(), String> {
        let count = self.desktops.count();
        if index >= count { return Err(format!("desktop {} does not exist (count {count})", index + 1)); }
        self.switch_desktop(index)
    }

    /// Runs the window `action` on the window `id` (the active window).
    fn _window_action(&mut self, action: Action, id: WindowId) -> Result<(), String> {
        match action {
            Action::Close => self.close_window(id),
            Action::SendLeft => self.send_and_follow(id, -1),
            Action::SendRight => self.send_and_follow(id, 1),
            Action::Lower => self.lower_window(id),
            Action::Focus(dir) => self._focus_direction(id, dir),
            other => Err(format!("{other:?} is not a window action")),
        }
    }

    /// Activates the shown window nearest to the window `id` in the
    /// direction `dir` (`directional_target`); none: nothing changes.
    fn _focus_direction(&mut self, id: WindowId, dir: Direction) -> Result<(), String> {
        let (_, info) = self.window_by_id(id).ok_or_else(|| format!("focus: unknown window {}", id.0))?;
        let from = self._space_rect(&info.window).ok_or_else(|| format!("focus: window {} is not shown", id.0))?;
        // Top first, so that on equal distance the upper window wins.
        let candidates: Vec<(WindowId, Rect)> = self
            .space
            .elements()
            .rev()
            .filter_map(|w| {
                let other = self._info_of(w)?.id;
                if other == id { return None; }
                Some((other, self._space_rect(w)?))
            })
            .collect();
        let Some(target) = directional_target(from, &candidates, dir) else {
            opusdm_ipc::debug_log!(LOG_TAG, "focus {dir:?}: no window that way from window {}", id.0);
            return Ok(());
        };
        self.activate_window(target)
    }

    /// The rectangle of the shown `window` in the space (`None` when hidden).
    fn _space_rect(&self, window: &smithay::desktop::Window) -> Option<Rect> {
        let loc = self.space.element_location(window)?;
        let size = window.geometry().size;
        Some(Rect { x: loc.x, y: loc.y, w: size.w, h: size.h })
    }
}

/// The `compositor.json` style name of `action`, for the logs.
fn _action_name(action: Action) -> String {
    match action {
        Action::Desktop(index) => format!("desktop_{}", index + 1),
        Action::Focus(dir) => format!("focus_{}", format!("{dir:?}").to_lowercase()),
        Action::SwitcherNext => "switcher_next".into(),
        Action::SwitcherPrev => "switcher_prev".into(),
        Action::SwitcherAll => "switcher_all".into(),
        Action::Close => "close".into(),
        Action::DesktopLeft => "desktop_left".into(),
        Action::DesktopRight => "desktop_right".into(),
        Action::SendLeft => "send_left".into(),
        Action::SendRight => "send_right".into(),
        Action::ShowDesktop => "show_desktop".into(),
        Action::Lower => "lower".into(),
    }
}

/// The action of the built-in binding `name` of `compositor.json`; `None`
/// for `quit`, `window_menu` (bindings of their own) and an unknown name.
pub fn action_for(name: &str) -> Option<Action> {
    let action = match name {
        "switcher_next" => Action::SwitcherNext,
        "switcher_prev" => Action::SwitcherPrev,
        "switcher_all" => Action::SwitcherAll,
        "close" => Action::Close,
        "desktop_left" => Action::DesktopLeft,
        "desktop_right" => Action::DesktopRight,
        "desktop_1" => Action::Desktop(0),
        "desktop_2" => Action::Desktop(1),
        "desktop_3" => Action::Desktop(2),
        "desktop_4" => Action::Desktop(3),
        "send_left" => Action::SendLeft,
        "send_right" => Action::SendRight,
        "show_desktop" => Action::ShowDesktop,
        "lower" => Action::Lower,
        "focus_left" => Action::Focus(Direction::Left),
        "focus_right" => Action::Focus(Direction::Right),
        "focus_up" => Action::Focus(Direction::Up),
        "focus_down" => Action::Focus(Direction::Down),
        _ => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100 x 100 rectangle at `(x, y)`.
    fn square(x: i32, y: i32) -> Rect {
        Rect { x, y, w: 100, h: 100 }
    }

    #[test]
    fn relative_desktop_wraps_or_stops_at_the_edges() {
        assert_eq!(relative_desktop(0, 4, -1, true), 3);
        assert_eq!(relative_desktop(0, 4, -1, false), 0);
        assert_eq!(relative_desktop(3, 4, 1, true), 0);
        assert_eq!(relative_desktop(3, 4, 1, false), 3);
        assert_eq!(relative_desktop(1, 4, 1, false), 2);
        assert_eq!(relative_desktop(0, 1, 1, true), 0);
    }

    #[test]
    fn left_picks_the_nearest_window_on_the_left() {
        let from = square(500, 500);
        let candidates = [(WindowId(1), square(100, 500)), (WindowId(2), square(300, 520)), (WindowId(3), square(900, 500))];
        assert_eq!(directional_target(from, &candidates, Direction::Left), Some(WindowId(2)));
        assert_eq!(directional_target(from, &candidates, Direction::Right), Some(WindowId(3)));
        assert_eq!(directional_target(from, &candidates, Direction::Down), Some(WindowId(2)));
        assert_eq!(directional_target(from, &candidates, Direction::Up), None);
    }

    #[test]
    fn nothing_on_the_left_is_none() {
        let from = square(100, 100);
        let candidates = [(WindowId(1), square(100, 400)), (WindowId(2), square(600, 100))];
        assert_eq!(directional_target(from, &candidates, Direction::Left), None);
        assert_eq!(directional_target(from, &[], Direction::Right), None);
    }

    #[test]
    fn on_equal_distance_the_top_window_wins() {
        let from = square(500, 500);
        let candidates = [(WindowId(7), square(300, 500)), (WindowId(8), square(300, 500))];
        assert_eq!(directional_target(from, &candidates, Direction::Left), Some(WindowId(7)));
    }

    #[test]
    fn every_built_in_but_quit_and_window_menu_has_an_action() {
        for (name, _) in crate::config::BUILTIN_DEFAULTS {
            let expected = !matches!(name, "quit" | "window_menu");
            assert_eq!(action_for(name).is_some(), expected, "{name}");
        }
        assert_eq!(action_for("desktop_2"), Some(Action::Desktop(1)));
        assert_eq!(action_for("nope"), None);
    }
}
