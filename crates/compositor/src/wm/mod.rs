//! Window management: stable window ids, virtual desktops and minimize.
//!
//! `State::windows` owns the `Window` of every toplevel for its whole life;
//! the space holds only the shown ones (a buffer, not minimized, on the
//! current desktop), in the order of `State::stack`. Every operation changes
//! the table first and then calls `sync_space`, which puts the space and the
//! keyboard focus in line with it.

pub mod desktops;

use std::time::Duration;

use smithay::{
    desktop::{find_popup_root_surface, PopupManager, Window},
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
};

use crate::state::{has_buffer, State, WindowInfo, LOG_TAG};
use desktops::Desktops;

/// Pace of the frame callbacks of the hidden windows that have a buffer
/// (minimized or on another desktop): slow, but their clients never stall.
pub const HIDDEN_FRAME_INTERVAL: Duration = Duration::from_secs(1);

/// Stable id of a toplevel: ids start at 1 and are never reused in a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u32);

impl State {
    /// A new window id (the first is 1).
    pub fn new_window_id(&mut self) -> WindowId {
        let id = WindowId(self.next_window_id);
        self.next_window_id = self.next_window_id.saturating_add(1);
        id
    }

    /// The `wl_surface` and the info of the window `id`, if it exists.
    pub fn window_by_id(&self, id: WindowId) -> Option<(&WlSurface, &WindowInfo)> {
        self.windows.iter().find(|(_, info)| info.id == id)
    }

    /// True when the window has a buffer, is not minimized and is on the
    /// current desktop: it belongs in the space.
    pub fn is_shown(info: &WindowInfo, desktops: &Desktops) -> bool {
        !info.minimized && info.desktop == desktops.current() && has_buffer(&info.window)
    }

    /// Re-maps exactly the shown windows, in `stack` order, and unmaps the
    /// rest (a hidden window keeps its position and loses its popups and the
    /// `Activated` state). When the window with the keyboard (or the root of
    /// the popup with it) is no longer shown, or nothing has the keyboard,
    /// the topmost shown window gets it; an exclusive keyboard layer keeps it.
    pub fn sync_space(&mut self) {
        let in_space: Vec<Window> = self.space.elements().cloned().collect();
        for window in in_space {
            let shown = self._info_of(&window).is_some_and(|i| Self::is_shown(i, &self.desktops));
            if !shown {
                self._hide(&window);
            }
        }
        for id in self.stack.clone() {
            let Some((wl, info)) = self.window_by_id(id) else {
                opusdm_ipc::err_log!(LOG_TAG, "sync space: window {} is in the stack but not in the table", id.0);
                continue;
            };
            if !Self::is_shown(info, &self.desktops) || self.unmapped.contains(&info.window) { continue; }
            let wl = wl.clone();
            let window = info.window.clone();
            let loc = match self.space.element_location(&window) {
                Some(p) => (p.x, p.y),
                None => {
                    // Commits of a hidden window do not refresh its cached bounding box (`commit` only
                    // does it for the windows of the space): refresh it before it is shown again.
                    window.on_commit();
                    self._show_position(&wl)
                }
            };
            // In stack order: each `map_element` puts the window on top.
            self.space.map_element(window, loc, false);
        }
        self._refocus_if_hidden();
    }

    /// Activates the window `id`: shows its desktop, clears its minimized
    /// state, raises it to the top of the stack and gives it the keyboard.
    /// A window without a buffer yet is only raised (it maps on top).
    // Called by the IPC and the tasklist from stage 2 task 2 on.
    #[allow(dead_code)]
    pub fn activate_window(&mut self, id: WindowId) -> Result<(), String> {
        let info = self._info_by_id_mut(id).ok_or_else(|| format!("activate: unknown window {}", id.0))?;
        info.minimized = false;
        let desktop = info.desktop;
        let window = info.window.clone();
        if !self.desktops.switch_to(desktop) {
            return Err(format!("activate: window {} is on desktop {desktop}, out of range", id.0));
        }
        self._raise_id(id);
        self.sync_space();
        if self.space.element_location(&window).is_some() {
            self.focus_window(Some(window));
        }
        opusdm_ipc::debug_log!(LOG_TAG, "window {} activated on desktop {desktop}", id.0);
        Ok(())
    }

    /// Minimizes (or unminimizes, without focusing it) the window `id`.
    pub fn set_minimized(&mut self, id: WindowId, minimized: bool) -> Result<(), String> {
        let info = self._info_by_id_mut(id).ok_or_else(|| format!("minimize: unknown window {}", id.0))?;
        info.minimized = minimized;
        opusdm_ipc::debug_log!(LOG_TAG, "window {} minimized: {minimized}", id.0);
        self.sync_space();
        Ok(())
    }

    /// Moves the window `id` to the desktop `desktop` (0-based).
    // Called by the IPC and the tasklist from stage 2 task 2 on.
    #[allow(dead_code)]
    pub fn move_to_desktop(&mut self, id: WindowId, desktop: u32) -> Result<(), String> {
        let count = self.desktops.count();
        if desktop >= count {
            return Err(format!("move to desktop: desktop {desktop} is out of range (count {count})"));
        }
        let info = self._info_by_id_mut(id).ok_or_else(|| format!("move to desktop: unknown window {}", id.0))?;
        info.desktop = desktop;
        opusdm_ipc::debug_log!(LOG_TAG, "window {} moved to desktop {desktop}", id.0);
        self.sync_space();
        Ok(())
    }

    /// Shows the desktop `index` (0-based).
    // Called by the IPC and the tasklist from stage 2 task 2 on.
    #[allow(dead_code)]
    pub fn switch_desktop(&mut self, index: u32) -> Result<(), String> {
        if !self.desktops.switch_to(index) {
            return Err(format!("switch desktop: desktop {index} is out of range (count {})", self.desktops.count()));
        }
        opusdm_ipc::debug_log!(LOG_TAG, "switched to desktop {index}");
        self.sync_space();
        Ok(())
    }

    /// Sets the number of desktops (clamped) and returns it; the windows of
    /// removed desktops move to the new last desktop.
    // Called by the IPC and the tasklist from stage 2 task 2 on.
    #[allow(dead_code)]
    pub fn set_desktop_count(&mut self, count: u32) -> u32 {
        let count = self.desktops.set_count(count);
        let desktops = self.desktops;
        for info in self.windows.values_mut() {
            info.desktop = desktops.clamp_index(info.desktop);
        }
        opusdm_ipc::debug_log!(LOG_TAG, "{count} desktops, current {}", desktops.current());
        self.sync_space();
        count
    }

    /// Asks the window `id` to close (`xdg_toplevel.close`).
    // Called by the IPC and the tasklist from stage 2 task 2 on.
    #[allow(dead_code)]
    pub fn close_window(&mut self, id: WindowId) -> Result<(), String> {
        let (_, info) = self.window_by_id(id).ok_or_else(|| format!("close: unknown window {}", id.0))?;
        let toplevel = info.window.toplevel().ok_or_else(|| format!("close: window {} has no toplevel", id.0))?;
        toplevel.send_close();
        Ok(())
    }

    /// Moves the window of `window` to the top of `stack`; logs a window
    /// that is not in the table.
    pub fn raise_in_stack(&mut self, window: &Window) {
        let Some(id) = self._info_of(window).map(|i| i.id) else {
            opusdm_ipc::err_log!(LOG_TAG, "raise: the window is not in the table, stack unchanged");
            return;
        };
        self._raise_id(id);
    }

    /// Moves `id` to the top of `stack`.
    fn _raise_id(&mut self, id: WindowId) {
        self.stack.retain(|i| *i != id);
        self.stack.push(id);
    }

    /// The info of the toplevel `window`, if it is in the table.
    fn _info_of(&self, window: &Window) -> Option<&WindowInfo> {
        self.windows.get(window.toplevel()?.wl_surface())
    }

    /// The mutable info of the window `id`, if it exists.
    fn _info_by_id_mut(&mut self, id: WindowId) -> Option<&mut WindowInfo> {
        self.windows.values_mut().find(|info| info.id == id)
    }

    /// Takes `window` out of the space: stores its position, dismisses its
    /// popups and takes its `Activated` state away.
    fn _hide(&mut self, window: &Window) {
        let position = self.space.element_location(window).map(|p| (p.x, p.y));
        self.space.unmap_elem(window);
        window.set_activated(false);
        let Some(toplevel) = window.toplevel().cloned() else {
            opusdm_ipc::err_log!(LOG_TAG, "hide: a window of the space has no toplevel, unmapped");
            return;
        };
        let wl = toplevel.wl_surface();
        match self.windows.get_mut(wl) {
            Some(info) => info.last_position = position,
            None => opusdm_ipc::err_log!(LOG_TAG, "hide: toplevel {} has no window entry", wl.id()),
        }
        for (popup, _) in PopupManager::popups_for_surface(wl) {
            if let Err(err) = PopupManager::dismiss_popup(wl, &popup) {
                opusdm_ipc::err_log!(LOG_TAG, "hide: popup of {} not dismissed: {err:?}", wl.id());
            }
        }
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} hidden at {position:?}", wl.id());
    }

    /// Where the hidden toplevel `wl` is shown again: the origin of its fill
    /// area when it is fullscreen or maximized, else the position it had
    /// (the origin of the usable area when it has none).
    fn _show_position(&mut self, wl: &WlSurface) -> (i32, i32) {
        let usable = self.usable_area();
        let output = self.output_area();
        let Some(info) = self.windows.get_mut(wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "show: toplevel {} has no window entry, placed at the usable origin", wl.id());
            return (usable.x, usable.y);
        };
        if info.fullscreen { return (output.x, output.y); }
        if info.maximized { return (usable.x, usable.y); }
        info.last_position.take().unwrap_or((usable.x, usable.y))
    }

    /// Gives the keyboard to the topmost shown window when the keyboard is
    /// on nothing, on a hidden window or on a popup of a hidden window. An
    /// exclusive keyboard layer keeps it.
    fn _refocus_if_hidden(&mut self) {
        if self.exclusive_layer.is_some() { return; }
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "sync space: the seat has no keyboard, focus is not checked");
            return;
        };
        let lost = match keyboard.current_focus() {
            None => true,
            Some(focus) => self._owner_is_hidden(&focus),
        };
        if lost {
            self.focus_topmost();
        }
    }

    /// True when `surface` is a toplevel (or a popup of a toplevel) that is
    /// not in the space.
    fn _owner_is_hidden(&self, surface: &WlSurface) -> bool {
        let root = match self.popups.find_popup(surface) {
            Some(kind) => match find_popup_root_surface(&kind) {
                Ok(root) => root,
                Err(_) => {
                    opusdm_ipc::debug_log!(LOG_TAG, "focus: the root of popup {} is gone, left to refresh", surface.id());
                    return false;
                }
            },
            None => surface.clone(),
        };
        self.windows.get(&root).is_some_and(|info| self.space.element_location(&info.window).is_none())
    }
}

#[cfg(test)]
mod tests {
    use smithay::{
        input::keyboard::XkbConfig,
        output::{Mode, Output, PhysicalProperties, Subpixel},
        reexports::wayland_server::Display,
    };

    use super::*;

    /// A state with `desktops` desktops, an output and a keyboard, no client.
    fn state(desktops: u32) -> (Display<State>, State) {
        let display: Display<State> = Display::new().expect("display");
        let props = PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "t".into(), model: "t".into() };
        let output = Output::new("TEST-1".into(), props);
        let mode = Mode { size: (1920, 1080).into(), refresh: 60_000 };
        output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
        let mut state = State::new(&display.handle(), output, desktops);
        state.seat.add_keyboard(XkbConfig::default(), 600, 25).expect("keyboard");
        (display, state)
    }

    #[test]
    fn unknown_window_ids_are_errors() {
        let (_display, mut s) = state(4);
        let id = WindowId(42);
        assert!(s.window_by_id(id).is_none());
        assert!(s.activate_window(id).is_err());
        assert!(s.set_minimized(id, true).is_err());
        assert!(s.move_to_desktop(id, 1).is_err());
        assert!(s.close_window(id).is_err());
    }

    #[test]
    fn switching_desktops_stays_inside_the_count() {
        let (_display, mut s) = state(4);
        assert!(s.switch_desktop(3).is_ok());
        assert_eq!(s.desktops.current(), 3);
        assert!(s.switch_desktop(4).is_err());
        assert_eq!(s.desktops.current(), 3);
    }

    #[test]
    fn the_desktop_count_is_clamped_and_shrinking_moves_the_current_desktop() {
        let (_display, mut s) = state(40);
        assert_eq!(s.desktops.count(), 16);
        assert!(s.switch_desktop(10).is_ok());
        assert_eq!(s.set_desktop_count(3), 3);
        assert_eq!(s.desktops.current(), 2);
        assert_eq!(s.set_desktop_count(0), 1);
    }

    #[test]
    fn window_ids_start_at_one_and_are_not_reused() {
        let (_display, mut s) = state(4);
        assert_eq!(s.new_window_id(), WindowId(1));
        assert_eq!(s.new_window_id(), WindowId(2));
    }
}
