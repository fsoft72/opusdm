//! X11 details GTK4 does not expose: keeping a window out of the taskbar
//! and pager, and reading or setting its position. Every function is a no-op
//! (with a log line for real errors) when the session is not X11.

use gtk4 as gtk;
use x11rb::protocol::xproto::{self, ConnectionExt};

/// Ask the window manager to keep `window` out of the taskbar (window list)
/// and the pager. One `ClientMessage` per state, like the dock does.
pub fn set_skip_taskbar(window: &gtk::Window) {
    let Some(xid) = opusdm_ui::x11_window_id(window) else { return };
    if let Err(e) = opusdm_ui::with_x11(|x11| x11.add_wm_states(xid, &["_NET_WM_STATE_SKIP_TASKBAR", "_NET_WM_STATE_SKIP_PAGER"])) {
        opusdm_ipc::err_log!("buttonbank", "cannot hide a bank window from the taskbar: {e}");
    }
}

/// Ask the window manager to show `window` on every virtual desktop when
/// `on`, or only on the current one when not.
pub fn set_all_desktops(window: &gtk::Window, on: bool) {
    let Some(xid) = opusdm_ui::x11_window_id(window) else { return };
    let result = opusdm_ui::with_x11(|x11| if on { x11.stick_to_all_desktops(xid) } else { x11.unstick_to_current_desktop(xid) });
    if let Err(e) = result {
        opusdm_ipc::err_log!("buttonbank", "cannot change the desktops of a bank window: {e}");
    }
}

/// `_NET_MOVERESIZE_WINDOW` flags: gravity NorthWest (the position is the
/// top-left of the window frame), x and y are set, source is an application.
const MOVE_FLAGS: u32 = 1 | (1 << 8) | (1 << 9) | (1 << 12);

/// Move `window` so the top-left corner of its frame is at `(x, y)` on the
/// screen. The request goes to the window manager, which ignores a plain
/// `ConfigureWindow` from another client.
pub fn move_to(window: &gtk::Window, x: i32, y: i32) {
    let Some(xid) = opusdm_ui::x11_window_id(window) else { return };
    if let Err(e) = opusdm_ui::with_x11(|x11| x11.move_resize(xid, MOVE_FLAGS, x, y, 0, 0)) {
        opusdm_ipc::err_log!("buttonbank", "cannot move a bank window: {e}");
    }
}

/// The position of the top-left corner of the frame of `window` on the
/// screen (the same point [`move_to`] takes), or `None` when it cannot be
/// read (not X11, not mapped).
pub fn position(window: &gtk::Window) -> Option<(i32, i32)> {
    let xid = opusdm_ui::x11_window_id(window)?;
    opusdm_ui::with_x11(|x11| {
        let conn = x11.conn();
        let reply = conn.translate_coordinates(xid, x11.root(), 0, 0)?.reply()?;
        let extents = x11.atom("_NET_FRAME_EXTENTS")?;
        let prop = conn.get_property(false, xid, extents, xproto::AtomEnum::CARDINAL, 0, 4)?.reply()?;
        let (left, top) = match prop.value32().map(|v| v.collect::<Vec<u32>>()) {
            Some(v) if v.len() == 4 => (v[0] as i32, v[2] as i32),
            _ => (0, 0),
        };
        Ok((i32::from(reply.dst_x) - left, i32::from(reply.dst_y) - top))
    })
    .ok()
}
