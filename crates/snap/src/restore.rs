//! Where a snapped window goes when it is dragged away: back to its size from
//! before the snap, under the pointer.

use crate::zones::Rect;

/// Frame rectangle of a snapped window, `stored`, restored to its pre-snap
/// size while the pointer is at `pointer`. `snapped` is the rectangle the
/// window has now and `press` where the pointer was when the drag started.
/// The pointer keeps the same proportional horizontal position inside the
/// window and the same distance from its top, as in KDE. The result is clamped
/// into `area` so the title bar stays reachable.
pub fn restore_rect(stored: Rect, snapped: Rect, press: (i32, i32), pointer: (i32, i32), area: Rect) -> Rect {
    let ratio = if snapped.w > 0 { ((press.0 - snapped.x) as f64 / snapped.w as f64).clamp(0.0, 1.0) } else { 0.5 };
    let x = pointer.0 - (ratio * stored.w as f64).round() as i32;
    let y = pointer.1 - (press.1 - snapped.y);
    Rect::new(clamp_into(x, area.x, area.x + area.w - stored.w), clamp_into(y, area.y, area.y + area.h - stored.h), stored.w, stored.h)
}

/// `value` limited to `lo..=hi`; `lo` wins when the range is empty (a window
/// bigger than the area).
fn clamp_into(value: i32, lo: i32, hi: i32) -> i32 {
    value.min(hi).max(lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0, y: 30, w: 1920, h: 1050 };
    const SNAPPED: Rect = Rect { x: 0, y: 30, w: 960, h: 1050 };
    const STORED: Rect = Rect { x: 300, y: 200, w: 600, h: 400 };

    #[test]
    fn keeps_the_stored_size_and_the_pointer_proportion() {
        let r = restore_rect(STORED, SNAPPED, (480, 40), (480, 40), AREA);
        assert_eq!((r.w, r.h), (600, 400));
        assert_eq!((r.x, r.y), (180, 30));
    }

    #[test]
    fn follows_the_pointer_when_it_moves() {
        let r = restore_rect(STORED, SNAPPED, (480, 40), (700, 300), AREA);
        assert_eq!((r.x, r.y), (700 - 300, 300 - 10));
    }

    #[test]
    fn a_press_near_the_left_edge_keeps_the_pointer_near_the_left_edge() {
        let r = restore_rect(STORED, SNAPPED, (96, 40), (500, 100), AREA);
        assert_eq!(r.x, 500 - 60);
    }

    #[test]
    fn is_clamped_into_the_area() {
        assert_eq!(restore_rect(STORED, SNAPPED, (480, 40), (0, 40), AREA).x, 0);
        assert_eq!(restore_rect(STORED, SNAPPED, (480, 40), (1919, 40), AREA).x, 1320);
        assert_eq!(restore_rect(STORED, SNAPPED, (480, 40), (480, 31), AREA).y, 30);
        assert_eq!(restore_rect(STORED, SNAPPED, (480, 40), (480, 1079), AREA).y, 680);
    }

    #[test]
    fn a_window_bigger_than_the_area_sits_at_its_origin() {
        let huge = Rect::new(0, 0, 3000, 2000);
        let r = restore_rect(huge, SNAPPED, (480, 40), (480, 40), AREA);
        assert_eq!((r.x, r.y), (0, 30));
    }
}
