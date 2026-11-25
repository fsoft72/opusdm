//! Edge snapping of a window dragged by a move grab: the zones near the
//! edges of the usable area and the rectangle a window snapped to a zone
//! takes.
//!
//! The pure part (`snap_zone`, `snap_rect`) has no Smithay types, so it is
//! unit tested without a display. The `State` part keeps no handle: the
//! snapped state is `WindowInfo::snapped` / `restore` (owned by the window
//! table) and the preview is a plain rectangle in `State::snap_preview`.

use smithay::{
    desktop::Window,
    reexports::wayland_server::Resource,
    utils::{Logical, Point},
    wayland::shell::xdg::XdgShellHandler,
};

use crate::geometry::Rect;
use crate::state::{State, LOG_TAG};

/// A zone a dragged window snaps to when it is released there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapZone {
    /// The left half of the usable area.
    Left,
    /// The right half of the usable area.
    Right,
    /// The whole usable area: the window is maximized.
    Top,
}

/// The zone under `pointer`: within `threshold` pixels of the left, right
/// or top edge of `area` (or past it), the left and right edges first;
/// `None` elsewhere, and always with a `threshold` of 0 or less.
pub fn snap_zone(pointer: (i32, i32), area: Rect, threshold: i32) -> Option<SnapZone> {
    if threshold <= 0 { return None; }
    let (x, y) = pointer;
    if x < area.x.saturating_add(threshold) { return Some(SnapZone::Left); }
    if x >= area.x.saturating_add(area.w).saturating_sub(threshold) { return Some(SnapZone::Right); }
    if y < area.y.saturating_add(threshold) { return Some(SnapZone::Top); }
    None
}

/// The rectangle of a window snapped to `zone` in `area`: the left or right
/// half (the right half takes the odd pixel), or the whole area for `Top`.
pub fn snap_rect(zone: SnapZone, area: Rect) -> Rect {
    let left = area.w / 2;
    match zone {
        SnapZone::Left => Rect { w: left, ..area },
        SnapZone::Right => Rect { x: area.x.saturating_add(left), w: area.w.saturating_sub(left), ..area },
        SnapZone::Top => area,
    }
}

impl State {
    /// The snap zone under the space position `location` while a window is
    /// dragged: `None` when `snap_enabled` is off in `desktop.json`.
    pub fn snap_zone_at(&self, location: Point<f64, Logical>) -> Option<SnapZone> {
        if !self.wm.snap_enabled { return None; }
        let p = location.to_i32_round();
        let threshold = i32::try_from(self.config.snap_threshold).unwrap_or(i32::MAX);
        snap_zone((p.x, p.y), self.usable_area(), threshold)
    }

    /// The rectangle a window snapped to `zone` takes now: the only place
    /// that computes it (the drag preview, the snap, a usable area change).
    pub fn snap_target(&self, zone: SnapZone) -> Rect {
        snap_rect(zone, self.usable_area())
    }

    /// Shows the preview of `zone` (`None`: no preview), with a debug trace
    /// when it changes.
    pub fn set_snap_preview(&mut self, zone: Option<SnapZone>) {
        let preview = zone.map(|z| self.snap_target(z));
        if preview == self.snap_preview { return; }
        opusdm_ipc::debug_log!(LOG_TAG, "snap preview {zone:?} {preview:?}");
        self.snap_preview = preview;
    }

    /// Snaps the mapped `window`, released by a move grab in `zone`, that
    /// has (or was just configured to) `size`: `Left` / `Right` configure it
    /// to the half of the usable area (its frame fills the half: the client
    /// gets the half minus the insets) and move it there, `Top` maximizes it
    /// (the stage 1 maximize path). Its geometry before the snap (position
    /// now, `size`) is kept in `restore`, so a later drag (or unmaximize)
    /// gives the size back. A window that is gone, not mapped, not in the
    /// table, or maximized / fullscreen meanwhile is not snapped (logged).
    pub fn snap_window(&mut self, window: &Window, zone: SnapZone, size: (i32, i32)) {
        let Some(toplevel) = window.toplevel().cloned() else {
            opusdm_ipc::err_log!(LOG_TAG, "snap {zone:?} refused: the window has no toplevel");
            return;
        };
        let wl = toplevel.wl_surface().clone();
        let Some(loc) = self.space.element_location(window) else {
            opusdm_ipc::debug_log!(LOG_TAG, "snap {zone:?} of {}: the window is not mapped, not snapped", wl.id());
            return;
        };
        let pre_snap = Rect { x: loc.x, y: loc.y, w: size.0, h: size.1 };
        let target = self.client_rect_in(&wl, self.snap_target(zone));
        let Some(info) = self.windows.get_mut(&wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "snap {zone:?} refused: toplevel {} has no window entry", wl.id());
            return;
        };
        if info.maximized || info.fullscreen {
            opusdm_ipc::debug_log!(LOG_TAG, "snap {zone:?} of {}: maximized or fullscreen meanwhile, not snapped", wl.id());
            return;
        }
        if zone == SnapZone::Top {
            self.maximize_request(toplevel);
            // `fill_window` stored the committed size, which may still be the size before the drag.
            match self.windows.get_mut(&wl) {
                Some(info) => info.restore = Some(pre_snap),
                None => opusdm_ipc::err_log!(LOG_TAG, "snap Top: toplevel {} has no window entry, no restore size", wl.id()),
            }
            opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} snapped to the top (maximized), restore {pre_snap:?}", wl.id());
            self.notify_watchers();
            return;
        }
        info.snapped = Some(zone);
        info.restore = Some(pre_snap);
        toplevel.with_pending_state(|state| state.size = Some((target.w, target.h).into()));
        self.move_window(window, (target.x, target.y));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} snapped {zone:?} to {target:?}, restore {pre_snap:?}", wl.id());
        self.notify_watchers();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0, y: 40, w: 1920, h: 1040 };

    #[test]
    fn a_pointer_near_the_left_edge_is_in_the_left_zone() {
        assert_eq!(snap_zone((5, 500), AREA, 10), Some(SnapZone::Left));
        assert_eq!(snap_zone((15, 500), AREA, 10), None);
    }

    #[test]
    fn the_right_and_top_edges_have_their_zones() {
        assert_eq!(snap_zone((1915, 500), AREA, 10), Some(SnapZone::Right));
        assert_eq!(snap_zone((1905, 500), AREA, 10), None);
        assert_eq!(snap_zone((900, 45), AREA, 10), Some(SnapZone::Top));
        assert_eq!(snap_zone((900, 55), AREA, 10), None);
    }

    #[test]
    fn a_pointer_past_the_edge_of_the_area_is_in_its_zone() {
        // Over a top panel (above the usable area): the top zone.
        assert_eq!(snap_zone((900, 10), AREA, 10), Some(SnapZone::Top));
        let left_panel = Rect { x: 50, y: 0, w: 1870, h: 1080 };
        assert_eq!(snap_zone((0, 500), left_panel, 10), Some(SnapZone::Left));
    }

    #[test]
    fn a_corner_belongs_to_the_side_zone() {
        assert_eq!(snap_zone((2, 42), AREA, 10), Some(SnapZone::Left));
        assert_eq!(snap_zone((1918, 42), AREA, 10), Some(SnapZone::Right));
    }

    #[test]
    fn a_zero_threshold_has_no_zone() {
        assert_eq!(snap_zone((0, 40), AREA, 0), None);
        assert_eq!(snap_zone((1919, 500), AREA, 0), None);
    }

    #[test]
    fn the_halves_of_a_1920x1040_area() {
        assert_eq!(snap_rect(SnapZone::Left, AREA), Rect { x: 0, y: 40, w: 960, h: 1040 });
        assert_eq!(snap_rect(SnapZone::Right, AREA), Rect { x: 960, y: 40, w: 960, h: 1040 });
        assert_eq!(snap_rect(SnapZone::Top, AREA), AREA);
    }

    #[test]
    fn the_right_half_takes_the_odd_pixel() {
        let odd = Rect { x: 10, y: 0, w: 1001, h: 700 };
        assert_eq!(snap_rect(SnapZone::Left, odd), Rect { x: 10, y: 0, w: 500, h: 700 });
        assert_eq!(snap_rect(SnapZone::Right, odd), Rect { x: 510, y: 0, w: 501, h: 700 });
    }
}
