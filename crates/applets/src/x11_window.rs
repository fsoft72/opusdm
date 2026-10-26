//! X11 hints of an applet window: a desktop widget above the wallpaper and
//! the icon field, below every normal window, on every virtual desktop, with
//! no taskbar or pager entry, at a given position.
//!
//! Recipe (Task 10 spike, Openbox with the repo `openbox-rc.xml` in a private
//! Xephyr): window type `_NET_WM_WINDOW_TYPE_NORMAL` with the states
//! `_NET_WM_STATE_BELOW`, `STICKY`, `SKIP_TASKBAR` and `SKIP_PAGER`, written
//! as properties when the window is realized (read by the window manager
//! when it adopts the window) and sent again as EWMH client messages after
//! map (Openbox honours only the messages once the window is mapped). The
//! position goes out as a `ConfigureWindow` plus `_NET_MOVERESIZE_WINDOW`,
//! the only form Openbox obeys for a managed window. It is the same recipe as
//! the dock: the BELOW layer sits above the hub's DESKTOP window and below
//! every normal window. A `DOCK` type would stay on top of everything and a
//! `DESKTOP` type would fall under the icon field, so both are wrong here.
//!
//! Alt + left drag (Task 11): Openbox moves the window itself, with these
//! hints too. Its `Frame` context binds `A-Left` Drag to `Move` and covers the
//! client window of an undecorated NORMAL window, so the grab is Openbox's:
//! GTK got no button press or release during the drag (only `is-active`
//! went false at the grab and true again at its end), while a plain click
//! did reach GTK. Task 11 therefore uses strategy A: read the position back
//! from X11 after the move (`TranslateCoordinates` of the xid to the root,
//! what `xwininfo` showed matching the moved frame), instead of strategy B
//! (tracking the pointer itself), which would need the Openbox binding not
//! to apply to these windows.
//!
//! Not done here: a transparent background is GTK CSS, not X11 (the window
//! already has a 32 bit ARGB visual, and `background: transparent` showed
//! the desktop through it under picom); click-through on transparent areas
//! works with an input region (`gdk::Surface::set_input_region` passed clicks
//! to the desktop below), but the region would have to follow the painted
//! alpha of the content on every redraw, which is not cheap.
//!
//! On Wayland (or any non-X11 surface) nothing is done; it is logged once.

use gtk4 as gtk;
use gtk::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Window type of an applet window: a plain window, so BELOW is honoured.
const WINDOW_TYPE: &str = "_NET_WM_WINDOW_TYPE_NORMAL";

/// States of an applet window: below normal windows, on every desktop, no
/// taskbar or pager entry.
const WINDOW_STATES: [&str; 4] = ["_NET_WM_STATE_BELOW", "_NET_WM_STATE_STICKY", "_NET_WM_STATE_SKIP_TASKBAR", "_NET_WM_STATE_SKIP_PAGER"];

/// Distance of a default-placed applet from the right edge of the monitor.
const DEFAULT_RIGHT_MARGIN: i32 = 24;

/// Distance of a default-placed applet from the top of the monitor (clear of
/// a top panel).
const DEFAULT_TOP_MARGIN: i32 = 64;

/// Step between two default-placed applets, so they do not overlap exactly.
const DEFAULT_CASCADE: i32 = 32;

/// Set once the "not an X11 window" line has been logged.
static NOT_X11_LOGGED: AtomicBool = AtomicBool::new(false);

/// Make the realized `window` an applet window on X11: type, states and the
/// geometry `x`, `y`, `w` x `h` (sizes below 1 become 1). Safe to call more
/// than once (realize, map and the retries after map). Logs and returns when
/// the window is not realized, is not an X11 window (once per process), or an
/// X11 call fails; it never panics.
pub fn prepare(window: &gtk::ApplicationWindow, x: i32, y: i32, w: i32, h: i32) {
    let title = window.title().map(|t| t.to_string()).unwrap_or_default();
    if window.surface().is_none() { opusdm_ipc::err_log!(APP_TAG, "applet window '{title}': not realized, no X11 hints applied"); return; }
    let Some(xid) = opusdm_ui::x11_window_id(window) else {
        if !NOT_X11_LOGGED.swap(true, Ordering::Relaxed) { opusdm_ipc::err_log!(APP_TAG, "applet windows are not X11 windows (Wayland?): no stacking, desktop or position hints are applied"); }
        return;
    };
    if let Err(e) = _apply(xid, x, y, w.max(1) as u32, h.max(1) as u32) {
        opusdm_ipc::err_log!(APP_TAG, "applet window '{title}' (0x{xid:x}): cannot apply the X11 hints: {e}");
    }
}

/// Write the type and states of `xid` as properties, send the states and the
/// all-desktops request as client messages, then move it to the geometry.
fn _apply(xid: u32, x: i32, y: i32, w: u32, h: u32) -> Result<(), opusdm_ui::X11Error> {
    opusdm_ui::with_x11(|x11| {
        x11.set_type_and_states(xid, WINDOW_TYPE, &WINDOW_STATES)?;
        x11.stick_to_all_desktops(xid)?;
        x11.add_wm_states(xid, &WINDOW_STATES)?;
        x11.pin_geometry(xid, x, y, w, h)
    })
}

/// Default position of an applet `width` pixels wide that has no saved
/// position, on the monitor `(mx, my, mw, mh)`: the top right area, moved
/// down and left by `index` steps so successive applets do not overlap
/// exactly, and never left of or above the monitor origin.
pub fn default_position(monitor: (i32, i32, i32, i32), width: i32, index: usize) -> (i32, i32) {
    let (mx, my, mw, _mh) = monitor;
    let step = DEFAULT_CASCADE.saturating_mul(i32::try_from(index).unwrap_or(i32::MAX));
    let x = (mx + mw - width - DEFAULT_RIGHT_MARGIN).saturating_sub(step).max(mx);
    let y = (my + DEFAULT_TOP_MARGIN).saturating_add(step);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_applet_goes_to_the_top_right_area() {
        assert_eq!(default_position((0, 0, 1280, 800), 260, 0), (1280 - 260 - DEFAULT_RIGHT_MARGIN, DEFAULT_TOP_MARGIN));
    }

    #[test]
    fn later_applets_cascade_and_stay_on_their_monitor() {
        let first = default_position((1920, 0, 1280, 800), 300, 0);
        let second = default_position((1920, 0, 1280, 800), 300, 1);
        assert_eq!(first.0 - second.0, DEFAULT_CASCADE);
        assert_eq!(second.1 - first.1, DEFAULT_CASCADE);
        assert!(first.0 >= 1920);
        assert_eq!(default_position((100, 0, 400, 300), 600, 3).0, 100, "a wide applet is not put left of the monitor");
        let far = default_position((0, 0, 1280, 800), 260, usize::MAX);
        assert_eq!(far.0, 0);
    }
}
