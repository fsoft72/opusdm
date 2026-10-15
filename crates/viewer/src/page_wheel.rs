//! Wheel scrolling over a PDF page: inside the page the wheel scrolls it; past
//! the top or the bottom edge it turns the page, so the pages read like one
//! long picture.

/// Distance (pixels) from an edge that still counts as being on it.
const EDGE_SLACK: f64 = 0.5;

/// What one wheel step does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelMove {
    /// Scroll the page to this value.
    Scroll(f64),
    /// Turn to the next (`true`) or previous (`false`) page.
    Turn(bool),
    /// On an edge, not pushed far enough yet to turn.
    Hold,
}

/// Scroll step, in pixels, of one wheel notch in a view `page` pixels high
/// (the step GTK's scrolled window uses).
pub fn notch_pixels(page: f64) -> f64 {
    page.max(1.0).powf(2.0 / 3.0)
}

/// The move for a wheel step of `delta` pixels (down is positive) with the
/// scroll at `value` of a content `upper` long in a view `page` long.
/// `overscroll` adds up the pushes past an edge; the page turns when it
/// reaches `threshold` pixels.
pub fn wheel_move(value: f64, upper: f64, page: f64, delta: f64, overscroll: &mut f64, threshold: f64) -> WheelMove {
    if delta == 0.0 { return WheelMove::Hold; }
    let max = (upper - page).max(0.0);
    let forward = delta > 0.0;
    let on_edge = if forward { value >= max - EDGE_SLACK } else { value <= EDGE_SLACK };
    if !on_edge {
        *overscroll = 0.0;
        return WheelMove::Scroll((value + delta).clamp(0.0, max));
    }
    if (*overscroll > 0.0) != forward { *overscroll = 0.0; }
    *overscroll += delta;
    if overscroll.abs() < threshold { return WheelMove::Hold; }
    *overscroll = 0.0;
    WheelMove::Turn(forward)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_the_page_the_wheel_scrolls_and_stops_at_the_edge() {
        let mut over = 0.0;
        assert_eq!(wheel_move(100.0, 1000.0, 400.0, 50.0, &mut over, 30.0), WheelMove::Scroll(150.0));
        assert_eq!(wheel_move(580.0, 1000.0, 400.0, 50.0, &mut over, 30.0), WheelMove::Scroll(600.0));
        assert_eq!(wheel_move(20.0, 1000.0, 400.0, -50.0, &mut over, 30.0), WheelMove::Scroll(0.0));
        assert_eq!(over, 0.0);
    }

    #[test]
    fn past_the_bottom_turns_forward_past_the_top_turns_back() {
        let mut over = 0.0;
        assert_eq!(wheel_move(600.0, 1000.0, 400.0, 50.0, &mut over, 30.0), WheelMove::Turn(true));
        assert_eq!(wheel_move(0.0, 1000.0, 400.0, -50.0, &mut over, 30.0), WheelMove::Turn(false));
    }

    #[test]
    fn a_page_that_fits_turns_on_every_notch() {
        let mut over = 0.0;
        assert_eq!(wheel_move(0.0, 400.0, 400.0, 30.0, &mut over, 30.0), WheelMove::Turn(true));
        assert_eq!(wheel_move(0.0, 400.0, 400.0, -30.0, &mut over, 30.0), WheelMove::Turn(false));
    }

    #[test]
    fn small_pushes_add_up_and_a_change_of_direction_starts_again() {
        let mut over = 0.0;
        assert_eq!(wheel_move(600.0, 1000.0, 400.0, 10.0, &mut over, 30.0), WheelMove::Hold);
        assert_eq!(wheel_move(600.0, 1000.0, 400.0, 10.0, &mut over, 30.0), WheelMove::Hold);
        assert_eq!(wheel_move(0.0, 400.0, 400.0, -10.0, &mut over, 30.0), WheelMove::Hold);
        assert_eq!(over, -10.0);
        assert_eq!(wheel_move(0.0, 400.0, 400.0, -25.0, &mut over, 30.0), WheelMove::Turn(false));
        assert_eq!(over, 0.0);
    }

    #[test]
    fn notch_follows_the_view_height() {
        assert!((notch_pixels(1000.0) - 100.0).abs() < 1e-9);
        assert_eq!(notch_pixels(0.0), 1.0);
    }
}
