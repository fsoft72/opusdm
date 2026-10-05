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
}
