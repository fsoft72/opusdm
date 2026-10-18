//! Geometry of the rectangle the user drags on the frozen screen.

/// Smallest selection side in pixels: anything smaller is a stray click.
pub const MIN_SIZE: i32 = 4;

/// A rectangle in image pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The rectangle between two drag points, whatever the drag direction, clamped
/// to an image of `bounds` (width, height). `None` when it is smaller than
/// [`MIN_SIZE`] on either side.
pub fn from_points(a: (f64, f64), b: (f64, f64), bounds: (i32, i32)) -> Option<Rect> {
    let (bw, bh) = bounds;
    if bw <= 0 || bh <= 0 { return None; }
    let clamp = |v: f64, max: i32| v.round().clamp(0.0, max as f64) as i32;
    let (x0, x1) = (clamp(a.0.min(b.0), bw), clamp(a.0.max(b.0), bw));
    let (y0, y1) = (clamp(a.1.min(b.1), bh), clamp(a.1.max(b.1), bh));
    let rect = Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
    if rect.w < MIN_SIZE || rect.h < MIN_SIZE { return None; }
    Some(rect)
}

/// The crop area `crop` (image pixels, any corner order) as a rectangle of an
/// image of `bounds`, or `None` when there is no crop or it is too small.
pub fn crop_rect(crop: Option<Area>, bounds: (i32, i32)) -> Option<Rect> {
    let (x0, y0, x1, y1) = crop?;
    from_points((x0, y0), (x1, y1), bounds)
}

/// How close (in pixels) the pointer must be to a side to grab it.
pub const GRAB: f64 = 8.0;

/// A selection in widget coordinates: x0, y0, x1, y1 (any corner order).
pub type Area = (f64, f64, f64, f64);

/// The sides of the selection under the pointer. Two sides at once is a corner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    /// True when at least one side is grabbed.
    pub fn any(&self) -> bool {
        self.left || self.right || self.top || self.bottom
    }

    /// The CSS cursor name for these sides.
    pub fn cursor(&self) -> &'static str {
        match (self.left, self.right, self.top, self.bottom) {
            (true, _, true, _) => "nw-resize",
            (_, true, true, _) => "ne-resize",
            (true, _, _, true) => "sw-resize",
            (_, true, _, true) => "se-resize",
            (true, ..) => "w-resize",
            (_, true, ..) => "e-resize",
            (_, _, true, _) => "n-resize",
            (.., true) => "s-resize",
            _ => "crosshair",
        }
    }
}

/// The corners of `area` ordered as (left, top, right, bottom).
fn normalized(area: Area) -> Area {
    (area.0.min(area.2), area.1.min(area.3), area.0.max(area.2), area.1.max(area.3))
}

/// The sides of `area` within `tol` pixels of the point `p`.
pub fn hit_edges(area: Area, p: (f64, f64), tol: f64) -> Edges {
    let (x0, y0, x1, y1) = normalized(area);
    let in_x = p.0 >= x0 - tol && p.0 <= x1 + tol;
    let in_y = p.1 >= y0 - tol && p.1 <= y1 + tol;
    let (dl, dr, dt, db) = ((p.0 - x0).abs(), (p.0 - x1).abs(), (p.1 - y0).abs(), (p.1 - y1).abs());
    // On a very thin selection both sides can match: the nearest one wins.
    Edges {
        left: in_y && dl <= tol && dl <= dr,
        right: in_y && dr <= tol && dr < dl,
        top: in_x && dt <= tol && dt <= db,
        bottom: in_x && db <= tol && db < dt,
    }
}

/// `area` with the grabbed `edges` moved by (`dx`, `dy`); the result is
/// normalized, so dragging a side past the opposite one flips the rectangle.
pub fn resize(area: Area, edges: Edges, dx: f64, dy: f64) -> Area {
    let (mut x0, mut y0, mut x1, mut y1) = normalized(area);
    if edges.left { x0 += dx; }
    if edges.right { x1 += dx; }
    if edges.top { y0 += dy; }
    if edges.bottom { y1 += dy; }
    normalized((x0, y0, x1, y1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_any_drag_direction() {
        let r = from_points((100.0, 80.0), (20.0, 10.0), (200, 200)).unwrap();
        assert_eq!(r, Rect { x: 20, y: 10, w: 80, h: 70 });
    }

    #[test]
    fn clamps_to_the_image() {
        let r = from_points((-30.0, -5.0), (500.0, 90.0), (200, 100)).unwrap();
        assert_eq!(r, Rect { x: 0, y: 0, w: 200, h: 90 });
    }

    #[test]
    fn rejects_a_tiny_selection() {
        assert_eq!(from_points((10.0, 10.0), (12.0, 50.0), (200, 200)), None);
        assert_eq!(from_points((10.0, 10.0), (10.0, 10.0), (200, 200)), None);
    }

    #[test]
    fn rejects_a_selection_fully_outside() {
        assert_eq!(from_points((300.0, 300.0), (400.0, 400.0), (200, 200)), None);
    }

    #[test]
    fn rejects_empty_bounds() {
        assert_eq!(from_points((0.0, 0.0), (50.0, 50.0), (0, 0)), None);
    }

    #[test]
    fn a_crop_is_a_rect_or_nothing() {
        assert_eq!(crop_rect(Some((50.0, 40.0, 10.0, 10.0)), (100, 100)), Some(Rect { x: 10, y: 10, w: 40, h: 30 }));
        assert_eq!(crop_rect(Some((10.0, 10.0, 11.0, 11.0)), (100, 100)), None);
        assert_eq!(crop_rect(None, (100, 100)), None);
    }

    const AREA: Area = (100.0, 100.0, 200.0, 180.0);

    #[test]
    fn hits_a_side_and_a_corner() {
        let e = hit_edges(AREA, (101.0, 140.0), GRAB);
        assert_eq!(e, Edges { left: true, ..Edges::default() });
        let c = hit_edges(AREA, (199.0, 181.0), GRAB);
        assert_eq!(c, Edges { right: true, bottom: true, ..Edges::default() });
        assert_eq!(c.cursor(), "se-resize");
    }

    #[test]
    fn misses_the_inside_and_the_far_outside() {
        assert!(!hit_edges(AREA, (150.0, 140.0), GRAB).any());
        assert!(!hit_edges(AREA, (101.0, 400.0), GRAB).any());
    }

    #[test]
    fn resizes_only_the_grabbed_sides() {
        let e = Edges { left: true, bottom: true, ..Edges::default() };
        assert_eq!(resize(AREA, e, 10.0, 20.0), (110.0, 100.0, 200.0, 200.0));
    }

    #[test]
    fn dragging_a_side_past_the_other_flips() {
        let e = Edges { right: true, ..Edges::default() };
        assert_eq!(resize(AREA, e, -150.0, 0.0), (50.0, 100.0, 100.0, 180.0));
    }
}
