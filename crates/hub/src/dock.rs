//! Turn the toolbar window into a top-docked X11 panel: full monitor width,
//! `_NET_WM_WINDOW_TYPE_DOCK`, and a top strut so other windows tile below it.

use gtk4 as gtk;
use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt, PropMode};
use x11rb::wrapper::ConnectionExt as _;

/// Panel height in pixels. Matches the toolbar's natural content height so the
/// reserved strut and the visible bar line up.
pub const PANEL_HEIGHT: i32 = 42;

/// Width of the primary monitor, or a sane fallback.
pub fn monitor_width(display: &gtk::gdk::Display) -> i32 {
    let monitors = display.monitors();
    monitors
        .item(0)
        .and_downcast::<gtk::gdk::Monitor>()
        .map(|m| m.geometry().width())
        .filter(|w| *w > 0)
        .unwrap_or(1280)
}

/// Apply the dock window type and a top strut to the realized `window`.
///
/// Must run after the window's X11 surface exists (connect_realize / _map).
/// Errors are logged and swallowed - the panel still works without the strut,
/// it just will not reserve space.
pub fn make_top_dock(window: &gtk::ApplicationWindow, width: i32) {
    let Some(xid) = x11_window_id(window) else {
        eprintln!("opusdm-hub: toolbar is not on X11; skipping dock/strut setup");
        return;
    };
    if let Err(e) = apply(xid, width) {
        eprintln!("opusdm-hub: cannot set panel struts: {e}");
    }
}

/// Fetch the X11 window id backing a GTK window, if running under X11.
fn x11_window_id(window: &gtk::ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

fn apply(xid: u32, width: i32) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, _screen) = x11rb::connect(None)?;

    let dock_type = conn
        .intern_atom(false, b"_NET_WM_WINDOW_TYPE_DOCK")?
        .reply()?
        .atom;
    let win_type = conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE")?.reply()?.atom;
    let strut = conn.intern_atom(false, b"_NET_WM_STRUT")?.reply()?.atom;
    let strut_partial = conn.intern_atom(false, b"_NET_WM_STRUT_PARTIAL")?.reply()?.atom;

    // Window type = DOCK.
    conn.change_property32(
        PropMode::REPLACE,
        xid,
        win_type,
        AtomEnum::ATOM,
        &[dock_type],
    )?;

    // _NET_WM_STRUT: left, right, top, bottom.
    conn.change_property32(
        PropMode::REPLACE,
        xid,
        strut,
        AtomEnum::CARDINAL,
        &[0, 0, PANEL_HEIGHT as u32, 0],
    )?;

    // _NET_WM_STRUT_PARTIAL: left, right, top, bottom,
    // left_start_y, left_end_y, right_start_y, right_end_y,
    // top_start_x, top_end_x, bottom_start_x, bottom_end_x.
    let top_end_x = width.max(1) as u32 - 1;
    conn.change_property32(
        PropMode::REPLACE,
        xid,
        strut_partial,
        AtomEnum::CARDINAL,
        &[0, 0, PANEL_HEIGHT as u32, 0, 0, 0, 0, 0, 0, top_end_x, 0, 0],
    )?;

    // Pin geometry to the top-left corner at full width.
    conn.configure_window(
        xid,
        &xproto::ConfigureWindowAux::new()
            .x(0)
            .y(0)
            .width(width.max(1) as u32)
            .height(PANEL_HEIGHT as u32),
    )?;

    conn.flush()?;
    Ok(())
}
