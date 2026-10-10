//! Turn a GTK window into the dock: an undecorated, ordinary window kept in
//! the `_NET_WM_STATE_BELOW` layer, hidden from the taskbar and pager. It
//! deliberately does NOT use `_NET_WM_WINDOW_TYPE_DOCK`: that type tells the
//! window manager to always keep the window on top (the same type the panel
//! uses, which wants exactly that), which no `_NET_WM_STATE_BELOW` on top of
//! it can override. A plain `NORMAL` window honours `BELOW` instead, so
//! maximised windows cover the dock as intended. It reserves NO screen space
//! (no strut), so it stays visible only over the desktop. It is sticky, shown
//! on every virtual desktop.
//!
//! On Wayland the dock is a layer surface in the BOTTOM layer instead: above
//! the desktop, below normal windows, anchored to its edge only so the
//! compositor centers it there (see [`layer_spec`]).

use crate::placement::Placement;
use gtk4 as gtk;
use gtk::prelude::*;
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::xproto::{AtomEnum, PropMode};
use x11rb::wrapper::ConnectionExt as _;

/// Layer shell placement of the dock placed by `place`: anchored to its own
/// edge, `EDGE_MARGIN` away from it, inside the space the panel leaves free,
/// with no exclusive zone (maximised windows cover it, as on X11).
pub fn layer_spec(place: Placement) -> opusdm_ui::layer::LayerSpec {
    use crate::placement::EDGE_MARGIN;
    use opusdm_ipc::DockPosition;
    use opusdm_ui::layer::{Anchors, Exclusive, KeyboardMode, Layer, LayerSpec, Margins};
    let mut anchors = Anchors::default();
    let mut margins = Margins::default();
    match place.position() {
        DockPosition::Top => (anchors.top, margins.top) = (true, EDGE_MARGIN),
        DockPosition::Bottom => (anchors.bottom, margins.bottom) = (true, EDGE_MARGIN),
        DockPosition::Left => (anchors.left, margins.left) = (true, EDGE_MARGIN),
        DockPosition::Right => (anchors.right, margins.right) = (true, EDGE_MARGIN),
    }
    LayerSpec {
        layer: Layer::Bottom,
        namespace: "opusdm-dock",
        anchors,
        margins,
        exclusive: Exclusive::None,
        keyboard: KeyboardMode::None,
        monitor: Some((place.mon_x, place.mon_y, place.mon_w, place.mon_h)),
    }
}

/// Apply the DOCK window type and a fixed geometry to the realized `window`.
/// Safe to call more than once (realize and map). Errors are logged and
/// swallowed.
pub fn make_dock_window(window: &gtk::ApplicationWindow, place: Placement, count: usize) {
    if opusdm_ui::layer::is_layer(window) {
        opusdm_ui::layer::update(window, &layer_spec(place));
        return;
    }
    let Some(xid) = x11_window_id(window) else {
        opusdm_ipc::debug_log!("opusdm-dock", "window is neither X11 nor a layer surface; skipping dock setup");
        return;
    };
    if let Err(e) = apply(xid, place, count) {
        opusdm_ipc::err_log!("opusdm-dock", "cannot set dock window hints: {e}");
    }
}

/// Re-assert position and size only. Cheap; used on `map` after the window
/// manager has run its placement policy.
///
/// The geometry goes out twice, as in the hub desktop window: a plain
/// `ConfigureWindow`, and a `_NET_MOVERESIZE_WINDOW` message. Once openbox
/// has adopted the window it ignores the position of a `ConfigureWindow` and
/// leaves the dock where its placement policy put it (top left with `Smart`,
/// the middle of the screen with `<center>yes</center>`); only the EWMH
/// message moves the frame itself.
pub fn pin_geometry(window: &gtk::ApplicationWindow, place: Placement, count: usize) {
    let Some(xid) = x11_window_id(window) else { return };
    let (x, y, w, h) = place.rect(count);
    if let Err(e) = opusdm_ui::with_x11(|x11| x11.pin_geometry(xid, x, y, w.max(1) as u32, h.max(1) as u32)) {
        opusdm_ipc::err_log!("opusdm-dock", "cannot pin dock geometry: {e}");
    }
}

/// Ask the window manager to put the dock in the BELOW layer, and hide it
/// from the taskbar and pager: above the desktop window, below every normal
/// window. A `_NET_WM_STATE` property set before mapping is not honoured by
/// openbox once it has adopted the window, so this goes through one
/// `ClientMessage` per state (each message carries at most two atoms).
pub fn set_below(window: &gtk::ApplicationWindow) {
    let Some(xid) = x11_window_id(window) else { return };
    let states = ["_NET_WM_STATE_BELOW", "_NET_WM_STATE_SKIP_TASKBAR", "_NET_WM_STATE_SKIP_PAGER"];
    if let Err(e) = opusdm_ui::with_x11(|x11| x11.add_wm_states(xid, &states)) {
        opusdm_ipc::err_log!("opusdm-dock", "cannot lower dock: {e}");
    }
}

/// Fetch the X11 window id backing a GTK window, if running under X11.
fn x11_window_id(window: &gtk::ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

fn apply(xid: u32, place: Placement, count: usize) -> Result<(), opusdm_ui::X11Error> {
    let (x, y, w, h) = place.rect(count);
    opusdm_ui::with_x11(|x11| {
        let conn = x11.conn();
        // NORMAL, not DOCK: see the module doc for why. `_NET_WM_STATE` is set
        // here as a property before mapping, which some window managers honour
        // for BELOW/SKIP_* (unlike ABOVE); `set_below` in `connect_map` covers
        // openbox, which does not.
        let normal_type = x11.atom("_NET_WM_WINDOW_TYPE_NORMAL")?;
        let win_type = x11.atom("_NET_WM_WINDOW_TYPE")?;
        let wm_state = x11.atom("_NET_WM_STATE")?;
        let state_below = x11.atom("_NET_WM_STATE_BELOW")?;
        let skip_taskbar = x11.atom("_NET_WM_STATE_SKIP_TASKBAR")?;
        let skip_pager = x11.atom("_NET_WM_STATE_SKIP_PAGER")?;
        let state_sticky = x11.atom("_NET_WM_STATE_STICKY")?;

        conn.change_property32(PropMode::REPLACE, xid, win_type, AtomEnum::ATOM, &[normal_type])?;
        conn.change_property32(
            PropMode::REPLACE,
            xid,
            wm_state,
            AtomEnum::ATOM,
            &[state_below, skip_taskbar, skip_pager, state_sticky],
        )?;

        // Keep the dock on every virtual desktop (property for adoption,
        // client messages once mapped).
        x11.stick_to_all_desktops(xid)?;

        // Position is user-specified so openbox does not re-center us.
        let mut hints = WmSizeHints::new();
        hints.position = Some((WmSizeHintsSpecification::UserSpecified, x, y));
        hints.size = Some((WmSizeHintsSpecification::UserSpecified, w, h));
        hints.min_size = Some((w, h));
        hints.max_size = Some((w, h));
        hints.set_normal_hints(conn, xid)?.check()?;

        x11.configure(xid, x, y, w as u32, h as u32)
    })
}
