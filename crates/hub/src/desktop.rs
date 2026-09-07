//! Turn the hub window into the WM desktop: a full monitor-sized, undecorated
//! window marked `_NET_WM_WINDOW_TYPE_DESKTOP` and kept below every other
//! window (`_NET_WM_STATE_BELOW`). No struts - the desktop is the background,
//! Listers float on top of it.

use gtk4 as gtk;
use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt, PropMode};
use x11rb::wrapper::ConnectionExt as _;

/// Geometry of the primary monitor in pixels, or a sane fallback.
pub fn monitor_geometry(display: &gtk::gdk::Display) -> (i32, i32) {
    let monitors = display.monitors();
    monitors
        .item(0)
        .and_downcast::<gtk::gdk::Monitor>()
        .map(|m| {
            let g = m.geometry();
            (g.width(), g.height())
        })
        .filter(|(w, h)| *w > 0 && *h > 0)
        .unwrap_or((1280, 800))
}

/// Apply the desktop window type, the keep-below state, and a fixed
/// full-monitor geometry to the realized `window`. Safe to call more than
/// once (realize and map).
///
/// Errors are logged and swallowed - the window still shows without the
/// hints, it just may not sit flush or stay below other windows.
pub fn make_desktop_window(window: &gtk::ApplicationWindow, width: i32, height: i32) {
    let Some(xid) = x11_window_id(window) else {
        eprintln!("opusdm-hub: window is not on X11; skipping desktop setup");
        return;
    };
    if let Err(e) = apply(xid, width, height) {
        eprintln!("opusdm-hub: cannot set desktop window hints: {e}");
    }
}

/// Re-assert the full-monitor geometry only. Cheap; used on the `map` signal
/// after the window manager has run its placement policy.
pub fn pin_geometry(window: &gtk::ApplicationWindow, width: i32, height: i32) {
    let Some(xid) = x11_window_id(window) else { return };
    if let Ok((conn, _)) = x11rb::connect(None) {
        let _ = conn.configure_window(
            xid,
            &xproto::ConfigureWindowAux::new()
                .x(0)
                .y(0)
                .width(width.max(1) as u32)
                .height(height.max(1) as u32),
        );
        let _ = conn.flush();
    }
}

/// Fetch the X11 window id backing a GTK window, if running under X11.
fn x11_window_id(window: &gtk::ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

fn apply(xid: u32, width: i32, height: i32) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, _screen) = x11rb::connect(None)?;
    let width = width.max(1) as u32;
    let height = height.max(1) as u32;

    let desktop_type = conn
        .intern_atom(false, b"_NET_WM_WINDOW_TYPE_DESKTOP")?
        .reply()?
        .atom;
    let win_type = conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE")?.reply()?.atom;
    let wm_state = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
    let state_below = conn.intern_atom(false, b"_NET_WM_STATE_BELOW")?.reply()?.atom;

    // Window type = DESKTOP.
    conn.change_property32(PropMode::REPLACE, xid, win_type, AtomEnum::ATOM, &[desktop_type])?;

    // Keep it below every other window.
    conn.change_property32(PropMode::REPLACE, xid, wm_state, AtomEnum::ATOM, &[state_below])?;

    // Tell the WM the position is user-specified so it does not re-center the
    // window (openbox centers windows that carry no position hint).
    let mut hints = WmSizeHints::new();
    hints.position = Some((WmSizeHintsSpecification::UserSpecified, 0, 0));
    hints.size = Some((WmSizeHintsSpecification::UserSpecified, width as i32, height as i32));
    hints.min_size = Some((width as i32, height as i32));
    hints.max_size = Some((width as i32, height as i32));
    hints.set_normal_hints(&conn, xid)?.check()?;

    // Pin geometry to the top-left corner at full monitor size.
    conn.configure_window(
        xid,
        &xproto::ConfigureWindowAux::new()
            .x(0)
            .y(0)
            .width(width)
            .height(height),
    )?;

    conn.flush()?;
    Ok(())
}
