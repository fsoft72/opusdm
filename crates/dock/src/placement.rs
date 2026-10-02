//! Pure geometry of the dock: which edge it sits on and the rectangle it
//! occupies inside a monitor. No X11 or GTK here, so it is unit-testable.

use opusdm_ipc::{DockPosition, PanelPosition};

/// Empty space around each icon, in pixels.
pub const ITEM_PAD: i32 = 6;
/// Empty space between the dock border and the first/last icon, in pixels.
pub const DOCK_PAD: i32 = 6;
/// Gap between the dock and the monitor edge it is docked to, in pixels.
pub const EDGE_MARGIN: i32 = 8;
/// Width of the dock window border drawn by the stylesheet, in pixels. It is
/// part of the window size GTK insists on, so the geometry must count it.
pub const BORDER: i32 = 1;

/// Where the dock sits: the requested edge, the icon size, the panel it must
/// keep clear of, and the monitor rect it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub position: DockPosition,
    pub icon_size: i32,
    pub panel: PanelPosition,
    pub panel_height: i32,
    pub mon_x: i32,
    pub mon_y: i32,
    pub mon_w: i32,
    pub mon_h: i32,
}

/// Whether icons run left to right (top and bottom edges) rather than top to
/// bottom.
pub fn is_horizontal(position: DockPosition) -> bool {
    matches!(position, DockPosition::Top | DockPosition::Bottom)
}

/// Size of one icon slot (icon plus padding), in pixels.
pub fn slot_size(icon_size: i32) -> i32 {
    icon_size + 2 * ITEM_PAD
}

/// Size of the dock window `(width, height)` for `count` icons. An empty dock
/// keeps one slot of room so it can still be right-clicked and dropped on.
pub fn dock_size(position: DockPosition, count: usize, icon_size: i32) -> (i32, i32) {
    let slot = slot_size(icon_size);
    let length = count.max(1) as i32 * slot + 2 * (DOCK_PAD + BORDER);
    let thickness = slot + 2 * (DOCK_PAD + BORDER);
    if is_horizontal(position) {
        (length, thickness)
    } else {
        (thickness, length)
    }
}

impl Placement {
    /// The edge the dock really uses: never the panel's edge.
    pub fn position(&self) -> DockPosition {
        self.position.avoiding(self.panel)
    }

    /// Root-relative rectangle `(x, y, w, h)` of the dock for `count` icons:
    /// centered along its edge, `EDGE_MARGIN` away from it. A left or right
    /// dock is centered in the strip the panel leaves free.
    pub fn rect(&self, count: usize) -> (i32, i32, i32, i32) {
        let position = self.position();
        let (w, h) = dock_size(position, count, self.icon_size);
        let center_x = self.mon_x + (self.mon_w - w) / 2;
        let top_band = if self.panel == PanelPosition::Top { self.panel_height } else { 0 };
        let bottom_band = if self.panel == PanelPosition::Bottom { self.panel_height } else { 0 };
        let free_top = self.mon_y + top_band;
        let free_h = self.mon_h - top_band - bottom_band;
        let center_y = free_top + (free_h - h) / 2;
        match position {
            DockPosition::Top => (center_x, self.mon_y + EDGE_MARGIN, w, h),
            DockPosition::Bottom => (center_x, self.mon_y + self.mon_h - h - EDGE_MARGIN, w, h),
            DockPosition::Left => (self.mon_x + EDGE_MARGIN, center_y, w, h),
            DockPosition::Right => (self.mon_x + self.mon_w - w - EDGE_MARGIN, center_y, w, h),
        }
    }

    /// Index at which a drop at window coordinates `(x, y)` inserts, for a
    /// dock holding `count` icons: the slot boundary nearest the pointer.
    pub fn insert_index(&self, count: usize, x: f64, y: f64) -> usize {
        let pos = if is_horizontal(self.position()) { x } else { y };
        let slot = slot_size(self.icon_size) as f64;
        let idx = ((pos - (DOCK_PAD + BORDER) as f64) / slot + 0.5).floor();
        idx.clamp(0.0, count as f64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(position: DockPosition, panel: PanelPosition) -> Placement {
        Placement {
            position,
            icon_size: 48,
            panel,
            panel_height: 40,
            mon_x: 100,
            mon_y: 50,
            mon_w: 1920,
            mon_h: 1080,
        }
    }

    #[test]
    fn size_swaps_axes_for_vertical_edges() {
        let (w, h) = dock_size(DockPosition::Bottom, 3, 48);
        assert_eq!(dock_size(DockPosition::Left, 3, 48), (h, w));
        assert_eq!((w, h), (3 * 60 + 14, 60 + 14));
    }

    #[test]
    fn empty_dock_keeps_one_slot() {
        assert_eq!(dock_size(DockPosition::Bottom, 0, 48), dock_size(DockPosition::Bottom, 1, 48));
    }

    #[test]
    fn bottom_is_centered_horizontally_above_the_edge() {
        let (x, y, w, h) = place(DockPosition::Bottom, PanelPosition::Top).rect(3);
        assert_eq!(x, 100 + (1920 - w) / 2);
        assert_eq!(y, 50 + 1080 - h - EDGE_MARGIN);
    }

    #[test]
    fn top_hugs_the_top_edge() {
        let (_, y, _, _) = place(DockPosition::Top, PanelPosition::Bottom).rect(3);
        assert_eq!(y, 50 + EDGE_MARGIN);
    }

    #[test]
    fn dock_on_the_panel_edge_jumps_to_the_opposite_one() {
        let p = place(DockPosition::Bottom, PanelPosition::Bottom);
        assert_eq!(p.position(), DockPosition::Top);
        assert_eq!(p.rect(3).1, 50 + EDGE_MARGIN);
    }

    #[test]
    fn side_docks_are_centered_in_the_strip_the_panel_leaves() {
        let (x, y, w, h) = place(DockPosition::Left, PanelPosition::Top).rect(3);
        assert_eq!(x, 100 + EDGE_MARGIN);
        assert_eq!(y, 50 + 40 + (1080 - 40 - h) / 2);
        let (x, y, ..) = place(DockPosition::Right, PanelPosition::Bottom).rect(3);
        assert_eq!(x, 100 + 1920 - w - EDGE_MARGIN);
        assert_eq!(y, 50 + (1080 - 40 - h) / 2);
    }

    #[test]
    fn insert_index_snaps_to_the_nearest_boundary() {
        let p = place(DockPosition::Bottom, PanelPosition::Top);
        assert_eq!(p.insert_index(3, 0.0, 0.0), 0);
        assert_eq!(p.insert_index(3, (DOCK_PAD + BORDER + 20) as f64, 0.0), 0);
        assert_eq!(p.insert_index(3, (DOCK_PAD + BORDER + 40) as f64, 0.0), 1);
        assert_eq!(p.insert_index(3, 5000.0, 0.0), 3);
        let v = place(DockPosition::Left, PanelPosition::Top);
        assert_eq!(v.insert_index(3, 5000.0, (DOCK_PAD + BORDER + 70) as f64), 1);
    }
}
