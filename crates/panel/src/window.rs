//! Turn a GTK window into a top-docked panel: an undecorated, monitor-wide
//! strip marked `_NET_WM_WINDOW_TYPE_DOCK`, kept above other windows, and
//! reserving screen space through `_NET_WM_STRUT_PARTIAL` so maximised
//! windows do not draw under it. Mirrors `opusdm-hub`'s `desktop.rs`.

use gtk4 as gtk;
use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::xproto::{
    self, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode, StackMode,
};
use x11rb::wrapper::ConnectionExt as _;

/// `_NET_WM_STATE` action: add the listed property.
const NET_WM_STATE_ADD: u32 = 1;

/// Height of the panel strip, in pixels.
pub const PANEL_HEIGHT: i32 = 40;

/// The `_NET_WM_STRUT_PARTIAL` 12-tuple for a top panel of `height` sitting on
/// a monitor at horizontal offset `mon_x`, `mon_w` wide: only the `top`,
/// `top_start_x` and `top_end_x` slots are non-zero. The strut's `start_x`/
/// `end_x` are root-relative, so a monitor that isn't at the origin (a second
/// or third head) needs its own `mon_x` here, not just its width.
///
/// Order: left, right, top, bottom, left_start_y, left_end_y, right_start_y,
/// right_end_y, top_start_x, top_end_x, bottom_start_x, bottom_end_x.
pub fn strut_partial(height: i32, mon_x: i32, mon_w: i32) -> [u32; 12] {
    let h = height.max(0) as u32;
    let start_x = mon_x.max(0) as u32;
    let end_x = (mon_x.max(0) + mon_w.max(1) - 1) as u32;
    [0, 0, h, 0, 0, 0, 0, 0, start_x, end_x, 0, 0]
}

/// Apply the DOCK window type, the keep-above state, the struts and a fixed
/// full-width geometry to the realized `window`, placed at `(mon_x, mon_y)`
/// and `mon_w` wide - the origin and width of the monitor this bar belongs
/// to. Safe to call more than once (realize and map). Errors are logged and
/// swallowed.
pub fn make_panel_window(window: &gtk::ApplicationWindow, mon_x: i32, mon_y: i32, mon_w: i32) {
    let Some(xid) = x11_window_id(window) else {
        eprintln!("opusdm-panel: window is not on X11; skipping dock setup");
        return;
    };
    if let Err(e) = apply(xid, mon_x, mon_y, mon_w) {
        eprintln!("opusdm-panel: cannot set dock window hints: {e}");
    }
}

/// Re-assert position and size only. Cheap; used on `map` after the window
/// manager has run its placement policy.
pub fn pin_geometry(window: &gtk::ApplicationWindow, mon_x: i32, mon_y: i32, mon_w: i32) {
    let Some(xid) = x11_window_id(window) else { return };
    if let Ok((conn, _)) = x11rb::connect(None) {
        let _ = conn.configure_window(
            xid,
            &xproto::ConfigureWindowAux::new()
                .x(mon_x)
                .y(mon_y)
                .width(mon_w.max(1) as u32)
                .height(PANEL_HEIGHT.max(1) as u32),
        );
        let _ = conn.flush();
    }
}

/// Ask the window manager to keep the panel above every other window and
/// raise it now. `_NET_WM_STATE` set as a property before mapping is not
/// honoured by openbox once it has adopted the window, so this goes through a
/// `ClientMessage` and an explicit restack. Used on `map`, after the desktop
/// window has finished asserting its own geometry.
pub fn raise_above(window: &gtk::ApplicationWindow) {
    let Some(xid) = x11_window_id(window) else { return };
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let wm_state = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
        let above = conn.intern_atom(false, b"_NET_WM_STATE_ABOVE")?.reply()?.atom;
        let ev = ClientMessageEvent::new(32, xid, wm_state, [NET_WM_STATE_ADD, above, 0, 0, 0]);
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ev,
        )?;
        conn.configure_window(
            xid,
            &xproto::ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        conn.flush()?;
        Ok(())
    };
    if let Err(e) = run() {
        eprintln!("opusdm-panel: cannot raise panel: {e}");
    }
}

/// Fetch the X11 window id backing a GTK window, if running under X11.
fn x11_window_id(window: &gtk::ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

fn apply(xid: u32, mon_x: i32, mon_y: i32, mon_w: i32) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, _screen) = x11rb::connect(None)?;
    let width = mon_w.max(1) as u32;
    let height = PANEL_HEIGHT.max(1) as u32;

    let dock_type = conn
        .intern_atom(false, b"_NET_WM_WINDOW_TYPE_DOCK")?
        .reply()?
        .atom;
    let win_type = conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE")?.reply()?.atom;
    let wm_state = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
    let state_above = conn.intern_atom(false, b"_NET_WM_STATE_ABOVE")?.reply()?.atom;
    let strut = conn.intern_atom(false, b"_NET_WM_STRUT")?.reply()?.atom;
    let strut_partial = conn
        .intern_atom(false, b"_NET_WM_STRUT_PARTIAL")?
        .reply()?
        .atom;

    conn.change_property32(PropMode::REPLACE, xid, win_type, AtomEnum::ATOM, &[dock_type])?;
    conn.change_property32(PropMode::REPLACE, xid, wm_state, AtomEnum::ATOM, &[state_above])?;

    // Reserve the top strip. `_NET_WM_STRUT` is the legacy 4-tuple
    // (left, right, top, bottom); `_NET_WM_STRUT_PARTIAL` the modern 12-tuple.
    conn.change_property32(PropMode::REPLACE, xid, strut, AtomEnum::CARDINAL, &[0, 0, height, 0])?;
    conn.change_property32(
        PropMode::REPLACE,
        xid,
        strut_partial,
        AtomEnum::CARDINAL,
        &strut_partial_values(height as i32, mon_x, width as i32),
    )?;

    // Position is user-specified so openbox does not re-center us.
    let mut hints = WmSizeHints::new();
    hints.position = Some((WmSizeHintsSpecification::UserSpecified, mon_x, mon_y));
    hints.size = Some((WmSizeHintsSpecification::UserSpecified, width as i32, height as i32));
    hints.min_size = Some((width as i32, height as i32));
    hints.max_size = Some((width as i32, height as i32));
    hints.set_normal_hints(&conn, xid)?.check()?;

    conn.configure_window(
        xid,
        &xproto::ConfigureWindowAux::new().x(mon_x).y(mon_y).width(width).height(height),
    )?;

    conn.flush()?;
    Ok(())
}

/// `strut_partial` as an owned array `change_property32` can borrow.
fn strut_partial_values(height: i32, mon_x: i32, mon_w: i32) -> [u32; 12] {
    strut_partial(height, mon_x, mon_w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strut_partial_only_fills_the_top_slots() {
        let s = strut_partial(40, 0, 1920);
        assert_eq!(s[0], 0, "left");
        assert_eq!(s[1], 0, "right");
        assert_eq!(s[2], 40, "top");
        assert_eq!(s[3], 0, "bottom");
        assert_eq!(s[8], 0, "top_start_x");
        assert_eq!(s[9], 1919, "top_end_x");
        assert_eq!(s[10], 0, "bottom_start_x");
    }

    #[test]
    fn strut_partial_clamps_a_degenerate_width() {
        let s = strut_partial(40, 0, 0);
        assert_eq!(s[9], 0, "top_end_x never underflows");
    }

    #[test]
    fn strut_partial_offsets_by_the_monitor_origin() {
        let s = strut_partial(40, 1920, 1280);
        assert_eq!(s[8], 1920, "top_start_x");
        assert_eq!(s[9], 3199, "top_end_x");
    }
}
