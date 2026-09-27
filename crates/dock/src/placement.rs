//! Pure geometry of the dock: which edge it sits on and the rectangle it
//! occupies inside a monitor. No X11 or GTK here, so it is unit-testable.

/// Screen edge the dock is docked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DockEdge {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
}

/// Size of one launcher icon, in pixels.
pub const ICON_SIZE: i32 = 48;
/// Empty space around each icon, in pixels.
pub const ITEM_PAD: i32 = 6;
/// Empty space between the dock border and the first/last icon, in pixels.
pub const DOCK_PAD: i32 = 6;
/// Gap between the dock and the monitor edge it is docked to, in pixels.
pub const EDGE_MARGIN: i32 = 8;

/// Where the dock sits: the edge and the monitor rect it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub edge: DockEdge,
    pub mon_x: i32,
    pub mon_y: i32,
    pub mon_w: i32,
    pub mon_h: i32,
}

/// Size of the dock window `(width, height)` for `count` icons on `edge`.
pub fn dock_size(edge: DockEdge, count: usize) -> (i32, i32) {
    let slot = ICON_SIZE + 2 * ITEM_PAD;
    let length = count as i32 * slot + 2 * DOCK_PAD;
    let thickness = slot + 2 * DOCK_PAD;
    match edge {
        DockEdge::Top | DockEdge::Bottom => (length, thickness),
        DockEdge::Left | DockEdge::Right => (thickness, length),
    }
}

impl Placement {
    /// Root-relative rectangle `(x, y, w, h)` of the dock for `count` icons:
    /// centered along its edge, `EDGE_MARGIN` away from it.
    pub fn rect(&self, count: usize) -> (i32, i32, i32, i32) {
        let (w, h) = dock_size(self.edge, count);
        let center_x = self.mon_x + (self.mon_w - w) / 2;
        let center_y = self.mon_y + (self.mon_h - h) / 2;
        match self.edge {
            DockEdge::Top => (center_x, self.mon_y + EDGE_MARGIN, w, h),
            DockEdge::Bottom => (center_x, self.mon_y + self.mon_h - h - EDGE_MARGIN, w, h),
            DockEdge::Left => (self.mon_x + EDGE_MARGIN, center_y, w, h),
            DockEdge::Right => (self.mon_x + self.mon_w - w - EDGE_MARGIN, center_y, w, h),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(edge: DockEdge) -> Placement {
        Placement { edge, mon_x: 100, mon_y: 50, mon_w: 1920, mon_h: 1080 }
    }

    #[test]
    fn size_swaps_axes_for_vertical_edges() {
        let (w, h) = dock_size(DockEdge::Bottom, 3);
        assert_eq!(dock_size(DockEdge::Left, 3), (h, w));
        assert_eq!((w, h), (3 * 60 + 12, 60 + 12));
    }

    #[test]
    fn bottom_is_centered_horizontally_above_the_edge() {
        let (x, y, w, h) = place(DockEdge::Bottom).rect(3);
        assert_eq!(x, 100 + (1920 - w) / 2);
        assert_eq!(y, 50 + 1080 - h - EDGE_MARGIN);
    }

    #[test]
    fn top_hugs_the_top_edge() {
        let (_, y, _, _) = place(DockEdge::Top).rect(3);
        assert_eq!(y, 50 + EDGE_MARGIN);
    }

    #[test]
    fn left_and_right_are_centered_vertically() {
        let (x, y, w, h) = place(DockEdge::Left).rect(3);
        assert_eq!((x, y), (100 + EDGE_MARGIN, 50 + (1080 - h) / 2));
        let (x, _, _, _) = place(DockEdge::Right).rect(3);
        assert_eq!(x, 100 + 1920 - w - EDGE_MARGIN);
    }
}
