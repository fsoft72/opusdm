//! X11 details GTK4 does not expose: keeping a window out of the taskbar
//! and pager, and reading or setting its position. Every function is a no-op
//! (with a log line for real errors) when the session is not X11.

use gdk4_x11::X11Surface;
use gtk4 as gtk;
use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, ClientMessageEvent, ConnectionExt, EventMask};

/// `_NET_WM_STATE` action: add the property.
const NET_WM_STATE_ADD: u32 = 1;

/// The X11 window id behind a GTK window, if it runs under X11 and is mapped.
fn xid(window: &gtk::Window) -> Option<u32> {
    let surface = window.surface()?;
    let x11 = surface.downcast::<X11Surface>().ok()?;
    Some(x11.xid() as u32)
}

/// Ask the window manager to keep `window` out of the taskbar (window list)
/// and the pager. One `ClientMessage` per state, like the dock does.
pub fn set_skip_taskbar(window: &gtk::Window) {
    let Some(xid) = xid(window) else { return };
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let wm_state = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
        let skip_taskbar = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_TASKBAR")?.reply()?.atom;
        let skip_pager = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_PAGER")?.reply()?.atom;
        for state in [skip_taskbar, skip_pager] {
            let ev = ClientMessageEvent::new(32, xid, wm_state, [NET_WM_STATE_ADD, state, 0, 0, 0]);
            conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)?;
        }
        conn.flush()?;
        Ok(())
    };
    if let Err(e) = run() {
        opusdm_ipc::err_log!("buttonbank", "cannot hide a bank window from the taskbar: {e}");
    }
}

/// `_NET_MOVERESIZE_WINDOW` flags: gravity NorthWest (the position is the
/// top-left of the window frame), x and y are set, source is an application.
const MOVE_FLAGS: u32 = 1 | (1 << 8) | (1 << 9) | (1 << 12);

/// Move `window` so the top-left corner of its frame is at `(x, y)` on the
/// screen. The request goes to the window manager, which ignores a plain
/// `ConfigureWindow` from another client.
pub fn move_to(window: &gtk::Window, x: i32, y: i32) {
    let Some(xid) = xid(window) else { return };
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let atom = conn.intern_atom(false, b"_NET_MOVERESIZE_WINDOW")?.reply()?.atom;
        let ev = ClientMessageEvent::new(32, xid, atom, [MOVE_FLAGS, x as u32, y as u32, 0, 0]);
        conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)?;
        conn.flush()?;
        Ok(())
    };
    if let Err(e) = run() {
        opusdm_ipc::err_log!("buttonbank", "cannot move a bank window: {e}");
    }
}

/// The position of the top-left corner of the frame of `window` on the
/// screen (the same point [`move_to`] takes), or `None` when it cannot be
/// read (not X11, not mapped).
pub fn position(window: &gtk::Window) -> Option<(i32, i32)> {
    let xid = xid(window)?;
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn.translate_coordinates(xid, root, 0, 0).ok()?.reply().ok()?;
    let extents = conn.intern_atom(false, b"_NET_FRAME_EXTENTS").ok()?.reply().ok()?.atom;
    let prop = conn.get_property(false, xid, extents, xproto::AtomEnum::CARDINAL, 0, 4).ok()?.reply().ok()?;
    let (left, top) = match prop.value32().map(|v| v.collect::<Vec<u32>>()) {
        Some(v) if v.len() == 4 => (v[0] as i32, v[2] as i32),
        _ => (0, 0),
    };
    Some((i32::from(reply.dst_x) - left, i32::from(reply.dst_y) - top))
}
