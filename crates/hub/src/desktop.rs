//! Turn the hub window into the WM desktop: a full monitor-sized, undecorated
//! window marked `_NET_WM_WINDOW_TYPE_DESKTOP`, kept below every other
//! window (`_NET_WM_STATE_BELOW`) and shown on every virtual desktop
//! (`_NET_WM_DESKTOP` = all, `_NET_WM_STATE_STICKY`). No struts - the desktop
//! is the background, Listers float on top of it.
//!
//! On Wayland the same window is a layer surface in the BACKGROUND layer
//! instead, anchored to every edge of its monitor (see [`layer_spec`]).

use gtk4 as gtk;

/// Layer shell placement of the desktop window of the monitor `rect`: the
/// whole monitor, below everything, ignoring the panel's exclusive zone
/// (the panel covers the desktop, as on X11). The keyboard comes on demand,
/// for the icon field and its rename entry.
pub fn layer_spec(rect: opusdm_ui::MonitorRect) -> opusdm_ui::layer::LayerSpec {
    opusdm_ui::layer::LayerSpec::background("opusdm-desktop", rect)
}

/// Apply the desktop window type, the keep-below and sticky states, the
/// all-desktops membership, and a fixed full-monitor geometry - placed at `(x, y)`, `width` x `height` - to the
/// realized `window`. Safe to call more than once (realize and map).
///
/// Errors are logged and swallowed - the window still shows without the
/// hints, it just may not sit flush or stay below other windows.
pub fn make_desktop_window(window: &gtk::ApplicationWindow, x: i32, y: i32, width: i32, height: i32) {
    if opusdm_ui::layer::is_layer(window) {
        opusdm_ui::layer::update(window, &layer_spec((x, y, width, height)));
        return;
    }
    let Some(xid) = opusdm_ui::x11_window_id(window) else {
        opusdm_ipc::debug_log!("opusdm-hub", "window is neither X11 nor a layer surface; skipping desktop setup");
        return;
    };
    if let Err(e) = apply(xid, x, y, width, height) {
        opusdm_ipc::err_log!("opusdm-hub", "cannot set desktop window hints: {e}");
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
    let Some(xid) = opusdm_ui::x11_window_id(window) else { return };
    if let Err(e) = opusdm_ui::with_x11(|x11| x11.pin_geometry(xid, x, y, width.max(1) as u32, height.max(1) as u32)) {
        opusdm_ipc::err_log!("opusdm-hub", "cannot pin desktop geometry: {e}");
    }
}

/// Mark `xid` as the desktop window and fix it at `(x, y)`, `width` x
/// `height`, through the shared `opusdm_x11` helpers.
fn apply(xid: u32, x: i32, y: i32, width: i32, height: i32) -> Result<(), opusdm_ui::X11Error> {
    let width = width.max(1) as u32;
    let height = height.max(1) as u32;
    opusdm_ui::with_x11(|x11| {
        // Window type DESKTOP, kept below every other window and sticky
        // across virtual desktops.
        x11.set_type_and_states(xid, "_NET_WM_WINDOW_TYPE_DESKTOP", &["_NET_WM_STATE_BELOW", "_NET_WM_STATE_STICKY"])?;

        // Show it on every virtual desktop. The state property above is read
        // by the WM only when it adopts the window; once mapped, it honours
        // only the EWMH client messages, so re-send BELOW that way too.
        x11.stick_to_all_desktops(xid)?;
        x11.add_wm_states(xid, &["_NET_WM_STATE_BELOW"])?;

        // User-specified position and fixed size, so the WM does not
        // re-center the window (openbox centers windows that carry no
        // position hint), then pin it to this monitor's origin.
        x11.set_fixed_geometry(xid, x, y, width, height)
    })
}
