//! Turn the hub window into the WM desktop: a full monitor-sized, undecorated
//! window marked `_NET_WM_WINDOW_TYPE_DESKTOP` and kept below every other
//! window (`_NET_WM_STATE_BELOW`). No struts - the desktop is the background,
//! Listers float on top of it.

use gtk4 as gtk;
use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::xproto::{
    self, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode,
};
use x11rb::wrapper::ConnectionExt as _;

/// `_NET_MOVERESIZE_WINDOW` flags: keep the window gravity, all four of x, y,
/// width and height are supplied, and the request comes from the application.
const MOVERESIZE_FLAGS: u32 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 12);

/// Apply the desktop window type, the keep-below state, and a fixed
/// full-monitor geometry - placed at `(x, y)`, `width` x `height` - to the
/// realized `window`. Safe to call more than once (realize and map).
///
/// Errors are logged and swallowed - the window still shows without the
/// hints, it just may not sit flush or stay below other windows.
pub fn make_desktop_window(window: &gtk::ApplicationWindow, x: i32, y: i32, width: i32, height: i32) {
    let Some(xid) = x11_window_id(window) else {
        eprintln!("opusdm-hub: window is not on X11; skipping desktop setup");
        return;
    };
    if let Err(e) = apply(xid, x, y, width, height) {
        eprintln!("opusdm-hub: cannot set desktop window hints: {e}");
    }
}

/// Re-assert the full-monitor geometry only. Cheap; used on the `map` signal
/// after the window manager has run its placement policy.
///
/// The geometry goes out twice: a plain `ConfigureWindow`, and a
/// `_NET_MOVERESIZE_WINDOW` message. Once the window manager has reparented
/// the window, the position in a `ConfigureWindow` is relative to the frame,
/// so only the EWMH message can pull the frame itself back to its monitor's
/// origin - openbox otherwise parks the desktop at the work area origin,
/// which sits below the panel's strut.
pub fn pin_geometry(window: &gtk::ApplicationWindow, x: i32, y: i32, width: i32, height: i32) {
    let Some(xid) = x11_window_id(window) else { return };
    let Ok((conn, screen_num)) = x11rb::connect(None) else { return };
    let width = width.max(1) as u32;
    let height = height.max(1) as u32;

    let _ = conn.configure_window(
        xid,
        &xproto::ConfigureWindowAux::new().x(x).y(y).width(width).height(height),
    );

    let moveresize = conn
        .intern_atom(false, b"_NET_MOVERESIZE_WINDOW")
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| reply.atom);
    if let Some(atom) = moveresize {
        let root = conn.setup().roots[screen_num].root;
        let ev = ClientMessageEvent::new(32, xid, atom, [MOVERESIZE_FLAGS, x as u32, y as u32, width, height]);
        let _ = conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ev,
        );
    }

    if let Err(e) = opusdm_ui::x11_sync(&conn) {
        eprintln!("opusdm-hub: cannot pin desktop geometry: {e}");
    }
}

/// Fetch the X11 window id backing a GTK window, if running under X11.
fn x11_window_id(window: &gtk::ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

fn apply(xid: u32, x: i32, y: i32, width: i32, height: i32) -> Result<(), Box<dyn std::error::Error>> {
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
    hints.position = Some((WmSizeHintsSpecification::UserSpecified, x, y));
    hints.size = Some((WmSizeHintsSpecification::UserSpecified, width as i32, height as i32));
    hints.min_size = Some((width as i32, height as i32));
    hints.max_size = Some((width as i32, height as i32));
    hints.set_normal_hints(&conn, xid)?.check()?;

    // Pin geometry to this monitor's origin at its full size.
    conn.configure_window(
        xid,
        &xproto::ConfigureWindowAux::new().x(x).y(y).width(width).height(height),
    )?;

    opusdm_ui::x11_sync(&conn)?;
    Ok(())
}
