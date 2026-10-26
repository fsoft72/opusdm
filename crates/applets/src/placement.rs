//! Pure geometry of the applet windows: keep a rect on the monitors, move a
//! rect by a pointer drag, keep a size at or above the manifest minimum.
//!
//! A rect is `(x, y, width, height)` in root window coordinates, the same
//! tuple as [`opusdm_ui::MonitorRect`].

/// A rect: x, y, width, height.
pub type Rect = (i32, i32, i32, i32);

/// Put `rect` inside the monitor that holds most of it (the first monitor
/// when it overlaps none). A rect larger than that monitor shrinks to fit it.
/// With no monitor the rect is returned unchanged.
pub fn clamp_to_monitors(rect: Rect, monitors: &[Rect]) -> Rect {
    let Some(&first) = monitors.first() else { return rect };
    let (mut best, mut best_area) = (first, 0);
    for &monitor in monitors {
        let area = _overlap(rect, monitor);
        if area > best_area { (best, best_area) = (monitor, area); }
    }
    let (mx, my, mw, mh) = best;
    let (x, y, w, h) = rect;
    let (w, h) = (w.min(mw), h.min(mh));
    (x.clamp(mx, mx + mw - w), y.clamp(my, my + mh - h), w, h)
}

/// The area shared by rects `a` and `b` (0 when they do not touch).
fn _overlap(a: Rect, b: Rect) -> i64 {
    let left = a.0.max(b.0);
    let right = a.0.saturating_add(a.2).min(b.0.saturating_add(b.2));
    let top = a.1.max(b.1);
    let bottom = a.1.saturating_add(a.3).min(b.1.saturating_add(b.3));
    if right <= left || bottom <= top { return 0; }
    i64::from(right - left) * i64::from(bottom - top)
}

/// The rect `start_rect` moved by how far the pointer went from
/// `start_pointer` to `pointer`; the size is kept.
pub fn dragged(start_rect: Rect, start_pointer: (i32, i32), pointer: (i32, i32)) -> Rect {
    let (x, y, w, h) = start_rect;
    (x + pointer.0 - start_pointer.0, y + pointer.1 - start_pointer.1, w, h)
}

/// `rect` with its width and height raised to at least `min` (min width,
/// min height); the position is kept.
pub fn at_least(rect: Rect, min: (i32, i32)) -> Rect {
    let (x, y, w, h) = rect;
    (x, y, w.max(min.0), h.max(min.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_HD: Rect = (0, 0, 1920, 1080);

    #[test]
    fn clamp_keeps_a_rect_that_is_inside() {
        assert_eq!(clamp_to_monitors((100, 200, 300, 250), &[FULL_HD]), (100, 200, 300, 250));
        assert_eq!(clamp_to_monitors((1620, 830, 300, 250), &[FULL_HD]), (1620, 830, 300, 250), "touching the corner is inside");
    }

    #[test]
    fn clamp_moves_an_offscreen_rect_back() {
        assert_eq!(clamp_to_monitors((5000, 100, 300, 250), &[FULL_HD]), (1620, 100, 300, 250));
        assert_eq!(clamp_to_monitors((-400, -900, 300, 250), &[FULL_HD]), (0, 0, 300, 250));
        assert_eq!(clamp_to_monitors((1800, 1000, 300, 250), &[FULL_HD]), (1620, 830, 300, 250), "a rect half out goes back in");
    }

    #[test]
    fn clamp_picks_the_monitor_with_most_overlap() {
        let monitors = [FULL_HD, (1920, 0, 1280, 1024)];
        // 100 px on the first monitor, 200 px on the second: it goes right.
        assert_eq!(clamp_to_monitors((1820, 50, 300, 200), &monitors), (1920, 50, 300, 200));
        // 200 px on the first, 100 px on the second: it goes left.
        assert_eq!(clamp_to_monitors((1720, 50, 300, 200), &monitors), (1620, 50, 300, 200));
        // Below the second monitor only (x range of the second): no overlap
        // at all, so the first monitor takes it.
        assert_eq!(clamp_to_monitors((2500, 3000, 300, 200), &monitors), (1620, 880, 300, 200));
        // Inside the second monitor: kept.
        assert_eq!(clamp_to_monitors((2500, 700, 300, 200), &monitors), (2500, 700, 300, 200));
    }

    #[test]
    fn clamp_shrinks_a_rect_bigger_than_the_monitor() {
        assert_eq!(clamp_to_monitors((50, 50, 2500, 1500), &[FULL_HD]), (0, 0, 1920, 1080));
        assert_eq!(clamp_to_monitors((1920, 0, 2000, 300), &[FULL_HD, (1920, 0, 1280, 1024)]), (1920, 0, 1280, 300));
    }

    #[test]
    fn clamp_with_no_monitors_returns_the_rect_unchanged() {
        assert_eq!(clamp_to_monitors((5000, -7, 300, 250), &[]), (5000, -7, 300, 250));
    }

    #[test]
    fn dragged_moves_by_the_pointer_delta() {
        assert_eq!(dragged((100, 200, 300, 250), (150, 210), (400, 160)), (350, 150, 300, 250));
        assert_eq!(dragged((100, 200, 300, 250), (150, 210), (150, 210)), (100, 200, 300, 250), "no move, no change");
        assert_eq!(dragged((10, 10, 30, 20), (0, 0), (-50, -60)), (-40, -50, 30, 20), "not clamped here");
    }

    #[test]
    fn a_resize_never_goes_below_the_minimum() {
        assert_eq!(at_least((100, 200, 80, 40), (200, 160)), (100, 200, 200, 160));
        assert_eq!(at_least((100, 200, 400, 40), (200, 160)), (100, 200, 400, 160));
        assert_eq!(at_least((100, 200, 400, 300), (200, 160)), (100, 200, 400, 300), "a bigger size is kept");
    }
}
