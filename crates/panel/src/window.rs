//! Turn a GTK window into a top- or bottom-docked panel: an undecorated,
//! monitor-wide strip marked `_NET_WM_WINDOW_TYPE_DOCK`, kept above other
//! windows, and reserving screen space through `_NET_WM_STRUT_PARTIAL` so
//! maximised windows do not draw under it. Mirrors `opusdm-hub`'s `desktop.rs`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::PanelPosition;
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::xproto::{AtomEnum, PropMode};
use x11rb::wrapper::ConnectionExt as _;

/// Height of the panel strip, in pixels.
pub const PANEL_HEIGHT: i32 = 40;

/// Where one bar sits: the edge it is docked to and the monitor rect
/// `(mon_x, mon_y)`, `mon_w` x `mon_h` it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub position: PanelPosition,
    pub mon_x: i32,
    pub mon_y: i32,
    pub mon_w: i32,
    pub mon_h: i32,
}

impl Placement {
    /// Root-relative y of the bar's top edge: the monitor's top for a top
    /// panel, `PANEL_HEIGHT` above the monitor's bottom for a bottom one.
    pub fn bar_y(&self) -> i32 {
        match self.position {
            PanelPosition::Top => self.mon_y,
            PanelPosition::Bottom => self.mon_y + (self.mon_h - PANEL_HEIGHT).max(0),
        }
    }
}

/// The `_NET_WM_STRUT_PARTIAL` 12-tuple for a panel of `height` docked to
/// `position` on a monitor at horizontal offset `mon_x`, `mon_w` wide. Only
/// the slots of the docked edge are non-zero. The strut's `start_x`/`end_x`
/// are root-relative, so a monitor that isn't at the origin (a second or
/// third head) needs its own `mon_x` here, not just its width.
/// `bottom_gap` is the space between the monitor's bottom edge and the
/// screen's: a bottom strut is measured from the screen edge, so it must
/// include it (0 when the monitor touches the screen bottom).
///
/// Order: left, right, top, bottom, left_start_y, left_end_y, right_start_y,
/// right_end_y, top_start_x, top_end_x, bottom_start_x, bottom_end_x.
pub fn strut_partial(position: PanelPosition, height: i32, mon_x: i32, mon_w: i32, bottom_gap: i32) -> [u32; 12] {
    let h = height.max(0) as u32;
    let start_x = mon_x.max(0) as u32;
    let end_x = (mon_x.max(0) + mon_w.max(1) - 1) as u32;
    match position {
        PanelPosition::Top => [0, 0, h, 0, 0, 0, 0, 0, start_x, end_x, 0, 0],
        PanelPosition::Bottom => {
            [0, 0, 0, h + bottom_gap.max(0) as u32, 0, 0, 0, 0, 0, 0, start_x, end_x]
        }
    }
}

/// Apply the DOCK window type, the keep-above state, the struts and a fixed
/// full-width geometry to the realized `window`, placed on the edge and
/// monitor `place` describes. Safe to call more than once (realize and map).
/// Errors are logged and swallowed.
pub fn make_panel_window(window: &gtk::ApplicationWindow, place: Placement) {
    let Some(xid) = x11_window_id(window) else {
        opusdm_ipc::err_log!("opusdm-panel", "window is not on X11; skipping dock setup");
        return;
    };
    if let Err(e) = apply(xid, place) {
        opusdm_ipc::err_log!("opusdm-panel", "cannot set dock window hints: {e}");
    }
}

/// Re-assert position and size only. Cheap; used on `map` after the window
/// manager has run its placement policy.
pub fn pin_geometry(window: &gtk::ApplicationWindow, place: Placement) {
    let Some(xid) = x11_window_id(window) else { return };
    let (x, y, w, h) = (place.mon_x, place.bar_y(), place.mon_w.max(1) as u32, PANEL_HEIGHT.max(1) as u32);
    if let Err(e) = opusdm_ui::with_x11(|x11| x11.configure(xid, x, y, w, h)) {
        opusdm_ipc::err_log!("opusdm-panel", "cannot pin panel geometry: {e}");
    }
}

/// Ask the window manager to keep the panel above every other window and
/// raise it now. `_NET_WM_STATE` set as a property before mapping is not
/// honoured by openbox once it has adopted the window, so this goes through a
/// `ClientMessage` and an explicit restack. Used on `map`, after the desktop
/// window has finished asserting its own geometry.
pub fn raise_above(window: &gtk::ApplicationWindow) {
    let Some(xid) = x11_window_id(window) else { return };
    if let Err(e) = opusdm_ui::with_x11(|x11| x11.raise_above(xid)) {
        opusdm_ipc::err_log!("opusdm-panel", "cannot raise panel: {e}");
    }
}

/// Fetch the X11 window id backing a GTK window, if running under X11.
fn x11_window_id(window: &gtk::ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

fn apply(xid: u32, place: Placement) -> Result<(), opusdm_ui::X11Error> {
    let (mon_x, mon_y) = (place.mon_x, place.bar_y());
    let width = place.mon_w.max(1) as u32;
    let height = PANEL_HEIGHT.max(1) as u32;
    opusdm_ui::with_x11(|x11| {
        let conn = x11.conn();
        let bottom_gap = x11.screen_height() - (place.mon_y + place.mon_h);
        let dock_type = x11.atom("_NET_WM_WINDOW_TYPE_DOCK")?;
        let win_type = x11.atom("_NET_WM_WINDOW_TYPE")?;
        let wm_state = x11.atom("_NET_WM_STATE")?;
        let state_above = x11.atom("_NET_WM_STATE_ABOVE")?;
        let strut = x11.atom("_NET_WM_STRUT")?;
        let strut_partial = x11.atom("_NET_WM_STRUT_PARTIAL")?;

        conn.change_property32(PropMode::REPLACE, xid, win_type, AtomEnum::ATOM, &[dock_type])?;
        conn.change_property32(PropMode::REPLACE, xid, wm_state, AtomEnum::ATOM, &[state_above])?;

        // Reserve the docked strip. `_NET_WM_STRUT` is the legacy 4-tuple
        // (left, right, top, bottom); `_NET_WM_STRUT_PARTIAL` the modern 12-tuple.
        let partial = self::strut_partial(place.position, height as i32, mon_x, width as i32, bottom_gap);
        conn.change_property32(PropMode::REPLACE, xid, strut, AtomEnum::CARDINAL, &partial[..4])?;
        conn.change_property32(PropMode::REPLACE, xid, strut_partial, AtomEnum::CARDINAL, &partial)?;

        // Position is user-specified so openbox does not re-center us.
        let mut hints = WmSizeHints::new();
        hints.position = Some((WmSizeHintsSpecification::UserSpecified, mon_x, mon_y));
        hints.size = Some((WmSizeHintsSpecification::UserSpecified, width as i32, height as i32));
        hints.min_size = Some((width as i32, height as i32));
        hints.max_size = Some((width as i32, height as i32));
        hints.set_normal_hints(conn, xid)?.check()?;

        x11.configure(xid, mon_x, mon_y, width, height)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_strut_only_fills_the_top_slots() {
        let s = strut_partial(PanelPosition::Top, 40, 0, 1920, 0);
        assert_eq!(s[0], 0, "left");
        assert_eq!(s[1], 0, "right");
        assert_eq!(s[2], 40, "top");
        assert_eq!(s[3], 0, "bottom");
        assert_eq!(s[8], 0, "top_start_x");
        assert_eq!(s[9], 1919, "top_end_x");
        assert_eq!(s[10], 0, "bottom_start_x");
    }

    #[test]
    fn bottom_strut_only_fills_the_bottom_slots() {
        let s = strut_partial(PanelPosition::Bottom, 40, 0, 1920, 0);
        assert_eq!(s[2], 0, "top");
        assert_eq!(s[3], 40, "bottom");
        assert_eq!(s[8], 0, "top_start_x");
        assert_eq!(s[9], 0, "top_end_x");
        assert_eq!(s[10], 0, "bottom_start_x");
        assert_eq!(s[11], 1919, "bottom_end_x");
    }

    #[test]
    fn bottom_strut_includes_the_gap_below_the_monitor() {
        let s = strut_partial(PanelPosition::Bottom, 40, 0, 1920, 120);
        assert_eq!(s[3], 160);
    }

    #[test]
    fn strut_clamps_a_degenerate_width() {
        assert_eq!(strut_partial(PanelPosition::Top, 40, 0, 0, 0)[9], 0, "top_end_x never underflows");
        assert_eq!(strut_partial(PanelPosition::Bottom, 40, 0, 0, 0)[11], 0, "bottom_end_x never underflows");
    }

    #[test]
    fn strut_offsets_by_the_monitor_origin() {
        let s = strut_partial(PanelPosition::Top, 40, 1920, 1280, 0);
        assert_eq!(s[8], 1920, "top_start_x");
        assert_eq!(s[9], 3199, "top_end_x");
        let s = strut_partial(PanelPosition::Bottom, 40, 1920, 1280, 0);
        assert_eq!(s[10], 1920, "bottom_start_x");
        assert_eq!(s[11], 3199, "bottom_end_x");
    }

    #[test]
    fn bar_y_follows_the_docked_edge() {
        let place = |position| Placement { position, mon_x: 0, mon_y: 100, mon_w: 1920, mon_h: 1080 };
        assert_eq!(place(PanelPosition::Top).bar_y(), 100);
        assert_eq!(place(PanelPosition::Bottom).bar_y(), 100 + 1080 - PANEL_HEIGHT);
    }
}
