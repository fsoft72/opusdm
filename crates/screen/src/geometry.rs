//! The maths of the arrangement canvas: edge snapping while a monitor is
//! dragged and the mapping between the desktop and the drawing area. No GTK,
//! so it is tested.

/// A monitor's rectangle on the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    /// The right edge.
    fn right(&self) -> i32 {
        self.x + self.w as i32
    }

    /// The bottom edge.
    fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }
}

/// The smallest shift (in absolute value) among `candidates` that is within
/// `threshold`, or 0.
fn best_shift(candidates: impl Iterator<Item = i32>, threshold: i32) -> i32 {
    candidates.filter(|d| d.abs() <= threshold).min_by_key(|d| d.abs()).unwrap_or(0)
}

/// Where a monitor of `size` dropped at `pos` ends up: each axis moves to the
/// nearest edge of another monitor (touching it, or lined up with it) when
/// that is within `threshold` desktop pixels.
pub fn snap_position(pos: (i32, i32), size: (u32, u32), others: &[Rect], threshold: i32) -> (i32, i32) {
    let me = Rect { x: pos.0, y: pos.1, w: size.0, h: size.1 };
    let dx = best_shift(others.iter().flat_map(|o| [o.right() - me.x, o.x - me.right(), o.x - me.x, o.right() - me.right()]), threshold);
    let dy = best_shift(others.iter().flat_map(|o| [o.bottom() - me.y, o.y - me.bottom(), o.y - me.y, o.bottom() - me.bottom()]), threshold);
    (pos.0 + dx, pos.1 + dy)
}

/// The largest scale the canvas uses, so one small monitor is not drawn huge.
const MAX_SCALE: f64 = 0.25;

/// How the desktop is drawn in the area: `canvas = desktop * scale + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub scale: f64,
    pub ox: f64,
    pub oy: f64,
}

impl View {
    /// A canvas point of a desktop point.
    pub fn to_canvas(&self, x: i32, y: i32) -> (f64, f64) {
        (x as f64 * self.scale + self.ox, y as f64 * self.scale + self.oy)
    }

    /// The canvas rectangle `(x, y, w, h)` of a desktop rectangle.
    pub fn rect_to_canvas(&self, r: &Rect) -> (f64, f64, f64, f64) {
        let (x, y) = self.to_canvas(r.x, r.y);
        (x, y, r.w as f64 * self.scale, r.h as f64 * self.scale)
    }

    /// Whether the canvas point is inside the desktop rectangle.
    pub fn hits(&self, r: &Rect, cx: f64, cy: f64) -> bool {
        let (x, y, w, h) = self.rect_to_canvas(r);
        cx >= x && cx <= x + w && cy >= y && cy <= y + h
    }
}

/// A view that shows all of `rects` centered in a `width` x `height` area,
/// with room around them (a third of the largest monitor) to drag into.
pub fn fit_view(rects: &[Rect], width: f64, height: f64) -> View {
    let Some(first) = rects.first() else { return View { scale: MAX_SCALE, ox: width / 2.0, oy: height / 2.0 } };
    let (mut left, mut top, mut right, mut bottom) = (first.x, first.y, first.right(), first.bottom());
    let mut biggest = 0;
    for r in rects {
        left = left.min(r.x);
        top = top.min(r.y);
        right = right.max(r.right());
        bottom = bottom.max(r.bottom());
        biggest = biggest.max(r.w).max(r.h);
    }
    let pad = biggest as f64 / 3.0;
    let (bw, bh) = ((right - left) as f64, (bottom - top) as f64);
    let scale = ((width / (bw + 2.0 * pad)).min(height / (bh + 2.0 * pad))).clamp(f64::MIN_POSITIVE, MAX_SCALE);
    View { scale, ox: (width - bw * scale) / 2.0 - left as f64 * scale, oy: (height - bh * scale) / 2.0 - top as f64 * scale }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };

    #[test]
    fn a_monitor_dropped_near_an_edge_touches_it() {
        assert_eq!(snap_position((1930, 5), (1280, 1024), &[A], 20), (1920, 0));
        assert_eq!(snap_position((-1290, -5), (1280, 1024), &[A], 20), (-1280, 0));
    }

    #[test]
    fn a_monitor_lines_up_with_the_top_or_bottom() {
        assert_eq!(snap_position((1920, 60), (1280, 1024), &[A], 20), (1920, 56), "bottoms line up");
        assert_eq!(snap_position((1920, 12), (1280, 720), &[A], 20), (1920, 0), "tops line up");
    }

    #[test]
    fn far_monitors_do_not_snap() {
        assert_eq!(snap_position((2500, 400), (1280, 720), &[A], 20), (2500, 400));
        assert_eq!(snap_position((100, 100), (10, 10), &[], 20), (100, 100));
    }

    #[test]
    fn the_nearest_edge_wins() {
        let b = Rect { x: 1900, y: 0, w: 100, h: 100 };
        assert_eq!(snap_position((2003, 500), (50, 50), &[A, b], 20).0, 2000);
    }

    #[test]
    fn fit_view_centers_and_keeps_everything_inside() {
        let rects = [A, Rect { x: 1920, y: 0, w: 1920, h: 1080 }];
        let v = fit_view(&rects, 800.0, 400.0);
        for r in &rects {
            let (x, y, w, h) = v.rect_to_canvas(r);
            assert!(x >= 0.0 && y >= 0.0 && x + w <= 800.0 && y + h <= 400.0, "{x} {y} {w} {h}");
        }
        let (x0, _, _, _) = v.rect_to_canvas(&rects[0]);
        let (x1, _, w1, _) = v.rect_to_canvas(&rects[1]);
        assert!((x0 - (800.0 - (x1 + w1))).abs() < 0.001, "centered");
    }

    #[test]
    fn a_single_monitor_is_not_drawn_huge_and_no_monitor_does_not_panic() {
        assert_eq!(fit_view(&[A], 4000.0, 4000.0).scale, 0.25);
        assert_eq!(fit_view(&[], 800.0, 400.0).scale, 0.25);
    }

    #[test]
    fn hits_finds_the_monitor_under_a_point() {
        let v = View { scale: 0.1, ox: 10.0, oy: 10.0 };
        assert!(v.hits(&A, 100.0, 50.0));
        assert!(!v.hits(&A, 5.0, 50.0));
        assert_eq!(v.to_canvas(100, 100), (20.0, 20.0));
    }
}
