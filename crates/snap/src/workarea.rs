//! The usable area of each monitor, from the struts of the docks and panels.
//!
//! `_NET_WORKAREA` is one rectangle for the whole screen, so a panel on one
//! monitor would also shrink the zones of the others. Each dock names the
//! band it reserves (`_NET_WM_STRUT_PARTIAL`), and only the monitors that band
//! touches lose that space. Pure, so it is unit tested without a server.

use crate::zones::Rect;

/// What a window reserves at the screen edges, in pixels, with the range
/// along each edge it covers (inclusive, in root coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Strut {
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
    pub left_range: (i32, i32),
    pub right_range: (i32, i32),
    pub top_range: (i32, i32),
    pub bottom_range: (i32, i32),
}

impl Strut {
    /// Read a `_NET_WM_STRUT_PARTIAL` value (12 numbers) or a plain
    /// `_NET_WM_STRUT` one (4 numbers: the whole edge is covered). `None` for
    /// any other length or when nothing is reserved.
    pub fn parse(values: &[u32]) -> Option<Strut> {
        let v = |i: usize| values[i] as i32;
        let strut = match values.len() {
            12 => Strut {
                left: v(0),
                right: v(1),
                top: v(2),
                bottom: v(3),
                left_range: (v(4), v(5)),
                right_range: (v(6), v(7)),
                top_range: (v(8), v(9)),
                bottom_range: (v(10), v(11)),
            },
            4 => {
                let whole = (0, i32::MAX);
                Strut { left: v(0), right: v(1), top: v(2), bottom: v(3), left_range: whole, right_range: whole, top_range: whole, bottom_range: whole }
            }
            _ => return None,
        };
        (strut.left > 0 || strut.right > 0 || strut.top > 0 || strut.bottom > 0).then_some(strut)
    }

    /// The bands this strut reserves on a screen of `screen` size, as
    /// rectangles in root coordinates, each with the side of a monitor it
    /// cuts into.
    fn bands(&self, screen: (i32, i32)) -> Vec<(Side, Rect)> {
        let along = |range: (i32, i32), limit: i32| (range.0.max(0), range.1.min(limit - 1));
        let mut bands = Vec::new();
        if self.left > 0 {
            let (a, b) = along(self.left_range, screen.1);
            bands.push((Side::Left, Rect::new(0, a, self.left, b - a + 1)));
        }
        if self.right > 0 {
            let (a, b) = along(self.right_range, screen.1);
            bands.push((Side::Right, Rect::new(screen.0 - self.right, a, self.right, b - a + 1)));
        }
        if self.top > 0 {
            let (a, b) = along(self.top_range, screen.0);
            bands.push((Side::Top, Rect::new(a, 0, b - a + 1, self.top)));
        }
        if self.bottom > 0 {
            let (a, b) = along(self.bottom_range, screen.0);
            bands.push((Side::Bottom, Rect::new(a, screen.1 - self.bottom, b - a + 1, self.bottom)));
        }
        bands
    }
}

/// The edge of a monitor a reserved band cuts into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// Whether two rectangles share at least one pixel.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.w > 0 && a.h > 0 && b.w > 0 && b.h > 0 && a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// The part of `monitor` left after every band of `struts` that touches it.
/// `screen` is the size of the whole screen (the struts are relative to its
/// edges). A band that covers all of the monitor is ignored.
pub fn monitor_area(monitor: Rect, struts: &[Strut], screen: (i32, i32)) -> Rect {
    let (mut left, mut top) = (monitor.x, monitor.y);
    let (mut right, mut bottom) = (monitor.x + monitor.w, monitor.y + monitor.h);
    for (side, band) in struts.iter().flat_map(|s| s.bands(screen)) {
        if !overlaps(monitor, band) { continue; }
        match side {
            Side::Left => left = left.max(band.x + band.w),
            Side::Right => right = right.min(band.x),
            Side::Top => top = top.max(band.y + band.h),
            Side::Bottom => bottom = bottom.min(band.y),
        }
    }
    if right <= left || bottom <= top { return monitor; }
    Rect::new(left, top, right - left, bottom - top)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i32, i32) = (3200, 1080);
    const FIRST: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };
    const SECOND: Rect = Rect { x: 1920, y: 0, w: 1280, h: 1024 };

    /// A top panel of `height` pixels over the x range `from..=to`.
    fn top_panel(height: u32, from: u32, to: u32) -> Strut {
        Strut::parse(&[0, 0, height, 0, 0, 0, 0, 0, from, to, 0, 0]).unwrap()
    }

    #[test]
    fn a_panel_on_the_first_monitor_leaves_the_second_alone() {
        let panel = top_panel(30, 0, 1919);
        assert_eq!(monitor_area(FIRST, &[panel], SCREEN), Rect::new(0, 30, 1920, 1050));
        assert_eq!(monitor_area(SECOND, &[panel], SCREEN), SECOND);
    }

    #[test]
    fn a_panel_over_both_monitors_shrinks_both() {
        let panel = top_panel(30, 0, 3199);
        assert_eq!(monitor_area(FIRST, &[panel], SCREEN), Rect::new(0, 30, 1920, 1050));
        assert_eq!(monitor_area(SECOND, &[panel], SCREEN), Rect::new(1920, 30, 1280, 994));
    }

    #[test]
    fn a_bottom_dock_and_a_top_panel_add_up() {
        let dock = Strut::parse(&[0, 0, 0, 48, 0, 0, 0, 0, 0, 0, 0, 1919]).unwrap();
        let area = monitor_area(FIRST, &[top_panel(30, 0, 1919), dock], SCREEN);
        assert_eq!(area, Rect::new(0, 30, 1920, 1002));
    }

    #[test]
    fn a_left_strut_covers_only_the_rows_it_names() {
        let dock = Strut::parse(&[60, 0, 0, 0, 0, 1079, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(monitor_area(FIRST, &[dock], SCREEN), Rect::new(60, 0, 1860, 1080));
        // The right monitor starts at x=1920: a strut at the left screen edge is not on it.
        assert_eq!(monitor_area(SECOND, &[dock], SCREEN), SECOND);
        let short = Strut::parse(&[60, 0, 0, 0, 2000, 2100, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(monitor_area(FIRST, &[short], SCREEN), FIRST, "its rows are below the monitor");
    }

    #[test]
    fn a_right_strut_sits_at_the_screen_edge() {
        let dock = Strut::parse(&[0, 40, 0, 0, 0, 0, 0, 1023, 0, 0, 0, 0]).unwrap();
        assert_eq!(monitor_area(SECOND, &[dock], SCREEN), Rect::new(1920, 0, 1240, 1024));
        assert_eq!(monitor_area(FIRST, &[dock], SCREEN), FIRST);
    }

    #[test]
    fn the_four_number_form_covers_the_whole_edge() {
        let panel = Strut::parse(&[0, 0, 24, 0]).unwrap();
        assert_eq!(monitor_area(FIRST, &[panel], SCREEN), Rect::new(0, 24, 1920, 1056));
        assert_eq!(monitor_area(SECOND, &[panel], SCREEN), Rect::new(1920, 24, 1280, 1000));
    }

    #[test]
    fn nothing_reserved_or_a_bad_length_is_no_strut() {
        assert_eq!(Strut::parse(&[0; 12]), None);
        assert_eq!(Strut::parse(&[0, 0, 0, 0]), None);
        assert_eq!(Strut::parse(&[1, 2, 3]), None);
        assert_eq!(Strut::parse(&[]), None);
    }

    #[test]
    fn a_band_that_would_leave_nothing_is_ignored() {
        let huge = Strut::parse(&[0, 0, 5000, 0]).unwrap();
        assert_eq!(monitor_area(FIRST, &[huge], SCREEN), FIRST);
    }

    #[test]
    fn no_struts_leave_the_monitor_whole() {
        assert_eq!(monitor_area(FIRST, &[], SCREEN), FIRST);
    }
}
