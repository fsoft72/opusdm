//! Window placement and resize geometry of `opusdm-compositor`.
//!
//! Pure functions on plain rectangles, with no Smithay types, so they are
//! unit tested without a display.

/// Offset between two cascaded windows, in pixels.
pub const CASCADE_STEP: i32 = 30;

/// Smallest size a window can be resized to, as `(width, height)`.
pub const MIN_WINDOW_SIZE: (i32, i32) = (64, 32);

/// A rectangle in output coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

/// Which edges of a window a resize drags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    /// The top edge moves.
    pub top: bool,
    /// The bottom edge moves.
    pub bottom: bool,
    /// The left edge moves.
    pub left: bool,
    /// The right edge moves.
    pub right: bool,
}

/// Returns the top-left position of the window with the given `index` in the cascade.
///
/// The positions are `area` origin plus `k * CASCADE_STEP` on both axes, with
/// `k = index % fit`, so the cascade wraps before the window leaves `area`.
/// A window wider or taller than `area` goes to the origin of `area`.
pub fn cascade_position(area: Rect, size: (i32, i32), index: usize) -> (i32, i32) {
    let (w, h) = size;
    if w > area.w || h > area.h { return (area.x, area.y); }
    let fit = (area.w.saturating_sub(w)).min(area.h.saturating_sub(h)) / CASCADE_STEP + 1;
    let k = index % fit as usize;
    let offset = k as i32 * CASCADE_STEP;
    (area.x + offset, area.y + offset)
}

/// Returns the rectangle after dragging the `edges` of `start` by `(dx, dy)`.
///
/// Dragging the left or top edge moves `x` or `y` and keeps the opposite edge
/// fixed. The size never goes under `min`: the dragged edge stops there and the
/// opposite edge does not move. Edges that are not set keep their size. When
/// both `left` and `right` are set, `left` wins (the same for `top` / `bottom`).
pub fn resize_rect(start: Rect, edges: Edges, dx: i32, dy: i32, min: (i32, i32)) -> Rect {
    let (min_w, min_h) = min;
    let (mut x, mut w) = (start.x, start.w);
    if edges.left {
        w = start.w.saturating_sub(dx).max(min_w);
        x = start.x.saturating_add(start.w).saturating_sub(w);
    } else if edges.right {
        w = start.w.saturating_add(dx).max(min_w);
    }
    let (mut y, mut h) = (start.y, start.h);
    if edges.top {
        h = start.h.saturating_sub(dy).max(min_h);
        y = start.y.saturating_add(start.h).saturating_sub(h);
    } else if edges.bottom {
        h = start.h.saturating_add(dy).max(min_h);
    }
    Rect { x, y, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0, y: 30, w: 1920, h: 1050 };

    #[test]
    fn windows_cascade_from_the_area_origin() {
        assert_eq!(cascade_position(AREA, (800, 600), 0), (0, 30));
        assert_eq!(cascade_position(AREA, (800, 600), 2), (60, 90));
    }

    #[test]
    fn the_cascade_wraps_before_leaving_the_area() {
        // 800x600 in 1920x1050: fit = min(1120 / 30, 450 / 30) + 1 = 16 positions.
        assert_eq!(cascade_position(AREA, (800, 600), 15), (450, 480)); // last one, bottom edge at 1080
        assert_eq!(cascade_position(AREA, (800, 600), 16), (0, 30));
        assert_eq!(cascade_position(AREA, (800, 600), 17), (30, 60));
    }

    #[test]
    fn a_window_bigger_than_the_area_goes_to_its_origin() {
        assert_eq!(cascade_position(AREA, (2500, 600), 5), (0, 30));
    }

    #[test]
    fn dragging_the_left_edge_keeps_the_right_edge() {
        let r = resize_rect(Rect { x: 100, y: 100, w: 400, h: 300 }, Edges { left: true, ..Edges::default() }, -50, 0, MIN_WINDOW_SIZE);
        assert_eq!(r, Rect { x: 50, y: 100, w: 450, h: 300 });
    }

    #[test]
    fn a_resize_stops_at_the_minimum_size() {
        let r = resize_rect(Rect { x: 100, y: 100, w: 400, h: 300 }, Edges { left: true, top: true, ..Edges::default() }, 1000, 1000, MIN_WINDOW_SIZE);
        assert_eq!(r, Rect { x: 436, y: 368, w: 64, h: 32 });
    }
}
