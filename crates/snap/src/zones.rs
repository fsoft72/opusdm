//! Snap zones: where the pointer must be to arm a zone, and the rectangle a
//! window takes once it is snapped there.

/// Distance in pixels from a monitor edge inside which a zone is armed.
pub const EDGE_THRESHOLD: i32 = 20;

/// Length in pixels, along an edge, of the band next to a corner that arms the
/// corner zone instead of an edge zone.
const CORNER_BAND: i32 = 80;

/// A rectangle in root window coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// Build a rectangle from its origin and size.
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Whether `point` lies inside the rectangle.
    pub fn contains(&self, point: (i32, i32)) -> bool {
        point.0 >= self.x && point.0 < self.x + self.w && point.1 >= self.y && point.1 < self.y + self.h
    }
}

/// A place a window can be snapped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Zone {
    /// Top edge: the window is maximized.
    Maximize,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// Left half, full height.
    Left,
    /// Right half, full height.
    Right,
    /// Left half, upper third of the height.
    LeftTop,
    /// Right half, upper third of the height.
    RightTop,
    /// Left half, lower third of the height.
    LeftBottom,
    /// Right half, lower third of the height.
    RightBottom,
}

/// The zone armed by `point` on a monitor whose usable area is `area`, or
/// `None` when the pointer is outside the area or not near a zone.
pub fn zone_at(point: (i32, i32), area: Rect) -> Option<Zone> {
    if !area.contains(point) || area.w <= 0 || area.h <= 0 { return None; }
    let (x, y) = (point.0 - area.x, point.1 - area.y);
    let (dx_left, dx_right) = (x, area.w - 1 - x);
    let (dy_top, dy_bottom) = (y, area.h - 1 - y);
    let near_left = dx_left < EDGE_THRESHOLD;
    let near_right = dx_right < EDGE_THRESHOLD;
    let near_top = dy_top < EDGE_THRESHOLD;
    let near_bottom = dy_bottom < EDGE_THRESHOLD;
    if !(near_left || near_right || near_top || near_bottom) { return None; }

    let in_band_x = dx_left.min(dx_right) < CORNER_BAND;
    let in_band_y = dy_top.min(dy_bottom) < CORNER_BAND;
    if in_band_x && in_band_y {
        let left = dx_left < dx_right;
        let top = dy_top < dy_bottom;
        return Some(match (left, top) {
            (true, true) => Zone::TopLeft,
            (false, true) => Zone::TopRight,
            (true, false) => Zone::BottomLeft,
            (false, false) => Zone::BottomRight,
        });
    }
    if near_top { return Some(Zone::Maximize); }
    if !(near_left || near_right) { return None; }
    let third = (y * 3 / area.h).min(2);
    Some(match (near_left, third) {
        (true, 0) => Zone::LeftTop,
        (true, 1) => Zone::Left,
        (true, _) => Zone::LeftBottom,
        (false, 0) => Zone::RightTop,
        (false, 1) => Zone::Right,
        (false, _) => Zone::RightBottom,
    })
}

/// The rectangle a window takes when snapped to `zone` on a monitor whose
/// usable area is `area`. Right-hand rectangles take the remaining width and
/// bottom ones the remaining height, so odd sizes leave no gap.
pub fn zone_rect(zone: Zone, area: Rect) -> Rect {
    let left_w = area.w / 2;
    let right_x = area.x + left_w;
    let right_w = area.w - left_w;
    let half_h = area.h / 2;
    let third_h = area.h / 3;
    let bottom_y = area.y + half_h;
    let bottom_h = area.h - half_h;
    match zone {
        Zone::Maximize => area,
        Zone::TopLeft => Rect::new(area.x, area.y, left_w, half_h),
        Zone::TopRight => Rect::new(right_x, area.y, right_w, half_h),
        Zone::BottomLeft => Rect::new(area.x, bottom_y, left_w, bottom_h),
        Zone::BottomRight => Rect::new(right_x, bottom_y, right_w, bottom_h),
        Zone::Left => Rect::new(area.x, area.y, left_w, area.h),
        Zone::Right => Rect::new(right_x, area.y, right_w, area.h),
        Zone::LeftTop => Rect::new(area.x, area.y, left_w, third_h),
        Zone::RightTop => Rect::new(right_x, area.y, right_w, third_h),
        Zone::LeftBottom => Rect::new(area.x, area.y + area.h - third_h, left_w, third_h),
        Zone::RightBottom => Rect::new(right_x, area.y + area.h - third_h, right_w, third_h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1920x1080 monitor with a 30 px panel on top.
    const AREA: Rect = Rect { x: 0, y: 30, w: 1920, h: 1050 };

    #[test]
    fn corners_are_quarters() {
        assert_eq!(zone_at((0, 30), AREA), Some(Zone::TopLeft));
        assert_eq!(zone_at((1919, 30), AREA), Some(Zone::TopRight));
        assert_eq!(zone_at((0, 1079), AREA), Some(Zone::BottomLeft));
        assert_eq!(zone_at((1919, 1079), AREA), Some(Zone::BottomRight));
    }

    #[test]
    fn top_edge_between_the_corners_maximizes() {
        assert_eq!(zone_at((960, 30), AREA), Some(Zone::Maximize));
        assert_eq!(zone_at((960, 49), AREA), Some(Zone::Maximize));
        assert_eq!(zone_at((960, 50), AREA), None);
    }

    #[test]
    fn side_edges_are_split_in_thirds() {
        assert_eq!(zone_at((0, 200), AREA), Some(Zone::LeftTop));
        assert_eq!(zone_at((0, 600), AREA), Some(Zone::Left));
        assert_eq!(zone_at((0, 900), AREA), Some(Zone::LeftBottom));
        assert_eq!(zone_at((1919, 200), AREA), Some(Zone::RightTop));
        assert_eq!(zone_at((1919, 600), AREA), Some(Zone::Right));
        assert_eq!(zone_at((1919, 900), AREA), Some(Zone::RightBottom));
    }

    #[test]
    fn bottom_edge_between_the_corners_has_no_zone() {
        assert_eq!(zone_at((960, 1079), AREA), None);
        assert_eq!(zone_at((500, 1079), AREA), None);
    }

    #[test]
    fn the_threshold_is_exclusive() {
        assert_eq!(zone_at((19, 600), AREA), Some(Zone::Left));
        assert_eq!(zone_at((20, 600), AREA), None);
        assert_eq!(zone_at((960, 500), AREA), None);
    }

    #[test]
    fn corner_wins_over_the_side_third_at_the_boundary() {
        // 79 px along the edge from the top is still the corner, 80 px is not.
        assert_eq!(zone_at((0, 30 + 79), AREA), Some(Zone::TopLeft));
        assert_eq!(zone_at((0, 30 + 80), AREA), Some(Zone::LeftTop));
        assert_eq!(zone_at((1919, 1079 - 79), AREA), Some(Zone::BottomRight));
        assert_eq!(zone_at((1919, 1079 - 80), AREA), Some(Zone::RightBottom));
    }

    #[test]
    fn a_point_outside_the_area_arms_nothing() {
        assert_eq!(zone_at((0, 10), AREA), None);
        assert_eq!(zone_at((1920, 600), AREA), None);
        assert_eq!(zone_at((-1, 600), AREA), None);
    }

    #[test]
    fn zones_follow_a_monitor_with_an_offset() {
        let second = Rect::new(1920, 0, 1280, 1024);
        assert_eq!(zone_at((1920, 500), second), Some(Zone::Left));
        assert_eq!(zone_at((3199, 500), second), Some(Zone::Right));
        assert_eq!(zone_at((1920, 0), second), Some(Zone::TopLeft));
        assert_eq!(zone_rect(Zone::Right, second), Rect::new(2560, 0, 640, 1024));
    }

    #[test]
    fn rectangles_match_the_spec_table() {
        assert_eq!(zone_rect(Zone::Maximize, AREA), AREA);
        assert_eq!(zone_rect(Zone::TopLeft, AREA), Rect::new(0, 30, 960, 525));
        assert_eq!(zone_rect(Zone::TopRight, AREA), Rect::new(960, 30, 960, 525));
        assert_eq!(zone_rect(Zone::BottomLeft, AREA), Rect::new(0, 555, 960, 525));
        assert_eq!(zone_rect(Zone::BottomRight, AREA), Rect::new(960, 555, 960, 525));
        assert_eq!(zone_rect(Zone::Left, AREA), Rect::new(0, 30, 960, 1050));
        assert_eq!(zone_rect(Zone::Right, AREA), Rect::new(960, 30, 960, 1050));
        assert_eq!(zone_rect(Zone::LeftTop, AREA), Rect::new(0, 30, 960, 350));
        assert_eq!(zone_rect(Zone::RightTop, AREA), Rect::new(960, 30, 960, 350));
        assert_eq!(zone_rect(Zone::LeftBottom, AREA), Rect::new(0, 730, 960, 350));
        assert_eq!(zone_rect(Zone::RightBottom, AREA), Rect::new(960, 730, 960, 350));
    }

    #[test]
    fn odd_sizes_leave_no_gap_between_the_halves() {
        let odd = Rect::new(0, 0, 1921, 1081);
        let (l, r) = (zone_rect(Zone::Left, odd), zone_rect(Zone::Right, odd));
        assert_eq!(l.x + l.w, r.x);
        assert_eq!(r.x + r.w, odd.x + odd.w);
        let (t, b) = (zone_rect(Zone::TopLeft, odd), zone_rect(Zone::BottomLeft, odd));
        assert_eq!(t.y + t.h, b.y);
        assert_eq!(b.y + b.h, odd.y + odd.h);
    }

    #[test]
    fn every_point_of_the_side_edge_arms_a_zone_containing_it() {
        for y in AREA.y..AREA.y + AREA.h {
            let zone = zone_at((0, y), AREA).unwrap_or_else(|| panic!("no zone at y={y}"));
            assert!(zone_rect(zone, AREA).contains((0, y)), "{zone:?} does not contain y={y}");
        }
    }
}
