//! Turn a GTK window into the dock: an undecorated, ordinary window kept in
//! the `_NET_WM_STATE_BELOW` layer, hidden from the taskbar and pager. It
//! deliberately does NOT use `_NET_WM_WINDOW_TYPE_DOCK`: that type tells the
//! window manager to always keep the window on top (the same type the panel
//! uses, which wants exactly that), which no `_NET_WM_STATE_BELOW` on top of
//! it can override. A plain `NORMAL` window honours `BELOW` instead, so
//! maximised windows cover the dock as intended. It reserves NO screen space
//! (no strut), so it stays visible only over the desktop.

use crate::placement::Placement;
use gtk4 as gtk;
use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::xproto::{
    self, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode,
};
use x11rb::wrapper::ConnectionExt as _;

/// `_NET_WM_STATE` action: add the listed property.
const NET_WM_STATE_ADD: u32 = 1;

/// Apply the DOCK window type and a fixed geometry to the realized `window`.
/// Safe to call more than once (realize and map). Errors are logged and
/// swallowed.
pub fn make_dock_window(window: &gtk::ApplicationWindow, place: Placement, count: usize) {
    let Some(xid) = x11_window_id(window) else {
        eprintln!("opusdm-dock: window is not on X11; skipping dock setup");
        return;
    };
    if let Err(e) = apply(xid, place, count) {
        eprintln!("opusdm-dock: cannot set dock window hints: {e}");
    }
}

/// Re-assert position and size only. Cheap; used on `map` after the window
/// manager has run its placement policy.
pub fn pin_geometry(window: &gtk::ApplicationWindow, place: Placement, count: usize) {
    let Some(xid) = x11_window_id(window) else { return };
    let (x, y, w, h) = place.rect(count);
    if let Ok((conn, _)) = x11rb::connect(None) {
        let _ = conn.configure_window(
            xid,
            &xproto::ConfigureWindowAux::new().x(x).y(y).width(w as u32).height(h as u32),
        );
        let _ = conn.flush();
    }
}

/// Ask the window manager to put the dock in the BELOW layer, and hide it
/// from the taskbar and pager: above the desktop window, below every normal
/// window. A `_NET_WM_STATE` property set before mapping is not honoured by
/// openbox once it has adopted the window, so this goes through one
/// `ClientMessage` per state (each message carries at most two atoms).
pub fn set_below(window: &gtk::ApplicationWindow) {
    let Some(xid) = x11_window_id(window) else { return };
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let wm_state = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
        let below = conn.intern_atom(false, b"_NET_WM_STATE_BELOW")?.reply()?.atom;
        let skip_taskbar = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_TASKBAR")?.reply()?.atom;
        let skip_pager = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_PAGER")?.reply()?.atom;
        for state in [below, skip_taskbar, skip_pager] {
            let ev = ClientMessageEvent::new(32, xid, wm_state, [NET_WM_STATE_ADD, state, 0, 0, 0]);
            conn.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ev,
            )?;
        }
        conn.flush()?;
        Ok(())
    };
    if let Err(e) = run() {
        eprintln!("opusdm-dock: cannot lower dock: {e}");
    }
}

/// Fetch the X11 window id backing a GTK window, if running under X11.
fn x11_window_id(window: &gtk::ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

fn apply(xid: u32, place: Placement, count: usize) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, _) = x11rb::connect(None)?;
    let (x, y, w, h) = place.rect(count);

    // NORMAL, not DOCK: see the module doc for why. `_NET_WM_STATE` is set
    // here as a property before mapping, which some window managers honour
    // for BELOW/SKIP_* (unlike ABOVE); `set_below` in `connect_map` covers
    // openbox, which does not.
    let normal_type = conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE_NORMAL")?.reply()?.atom;
    let win_type = conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE")?.reply()?.atom;
    let wm_state = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
    let state_below = conn.intern_atom(false, b"_NET_WM_STATE_BELOW")?.reply()?.atom;
    let skip_taskbar = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_TASKBAR")?.reply()?.atom;
    let skip_pager = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_PAGER")?.reply()?.atom;

    conn.change_property32(PropMode::REPLACE, xid, win_type, AtomEnum::ATOM, &[normal_type])?;
    conn.change_property32(
        PropMode::REPLACE,
        xid,
        wm_state,
        AtomEnum::ATOM,
        &[state_below, skip_taskbar, skip_pager],
    )?;

    // Position is user-specified so openbox does not re-center us.
    let mut hints = WmSizeHints::new();
    hints.position = Some((WmSizeHintsSpecification::UserSpecified, x, y));
    hints.size = Some((WmSizeHintsSpecification::UserSpecified, w, h));
    hints.min_size = Some((w, h));
    hints.max_size = Some((w, h));
    hints.set_normal_hints(&conn, xid)?.check()?;

    conn.configure_window(
        xid,
        &xproto::ConfigureWindowAux::new().x(x).y(y).width(w as u32).height(h as u32),
    )?;
    conn.flush()?;
    Ok(())
}
