//! Icons of the tool bar, drawn with cairo so they do not depend on the icon
//! theme and follow the text colour of the theme.

use crate::annotations::Size;
use gtk4 as gtk;
use gtk::cairo;
use gtk::prelude::*;

/// Side of an icon, in pixels.
pub const ICON_SIZE: i32 = 24;

/// Stroke width of the tool icons.
const STROKE: f64 = 2.5;

/// Length of an arrow head in the icon, and its half angle in radians.
const HEAD_LEN: f64 = 8.0;
const HEAD_HALF_ANGLE: f64 = 0.5;

/// An icon of the tool bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Select,
    Line,
    Arrow,
    Text,
    Undo,
    Redo,
    /// A stroke as thick as the size it stands for.
    Width(Size),
}

/// A drawing area showing `icon` in the foreground colour of the widget.
pub fn widget(icon: Icon) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(ICON_SIZE);
    area.set_content_height(ICON_SIZE);
    area.set_draw_func(move |area, cr, w, h| {
        let c = area.color();
        let _ = paint(cr, icon, (c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64), (w as f64, h as f64));
    });
    area
}

/// Draw `icon` in `rgba`, scaled from its 24 x 24 design to `size`.
pub fn paint(cr: &cairo::Context, icon: Icon, rgba: (f64, f64, f64, f64), size: (f64, f64)) -> Result<(), cairo::Error> {
    let side = ICON_SIZE as f64;
    cr.scale(size.0 / side, size.1 / side);
    cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_width(STROKE);
    match icon {
        Icon::Select => {
            // The classic pointer.
            cr.move_to(6.0, 3.0);
            for (x, y) in [(6.0, 19.0), (10.5, 15.0), (13.5, 21.0), (16.0, 19.8), (13.0, 14.0), (18.5, 14.0)] {
                cr.line_to(x, y);
            }
            cr.close_path();
            cr.fill()
        }
        Icon::Line => {
            cr.move_to(5.0, 19.0);
            cr.line_to(19.0, 5.0);
            cr.stroke()
        }
        Icon::Arrow => {
            let (from, tip): ((f64, f64), (f64, f64)) = ((5.0, 19.0), (19.0, 5.0));
            let back = (from.1 - tip.1).atan2(from.0 - tip.0);
            let wing = |a: f64| (tip.0 + HEAD_LEN * (back + a).cos(), tip.1 + HEAD_LEN * (back + a).sin());
            let (w1, w2) = (wing(HEAD_HALF_ANGLE), wing(-HEAD_HALF_ANGLE));
            let base = ((w1.0 + w2.0) / 2.0, (w1.1 + w2.1) / 2.0);
            cr.move_to(from.0, from.1);
            cr.line_to(base.0, base.1);
            cr.stroke()?;
            cr.move_to(tip.0, tip.1);
            cr.line_to(w1.0, w1.1);
            cr.line_to(w2.0, w2.1);
            cr.close_path();
            cr.fill()
        }
        Icon::Text => {
            // A capital T.
            cr.move_to(5.0, 6.0);
            cr.line_to(19.0, 6.0);
            cr.move_to(12.0, 6.0);
            cr.line_to(12.0, 19.0);
            cr.stroke()
        }
        Icon::Undo => undo_arrow(cr),
        Icon::Redo => {
            // The mirror image of undo.
            cr.translate(side, 0.0);
            cr.scale(-1.0, 1.0);
            undo_arrow(cr)
        }
        Icon::Width(s) => {
            cr.set_line_width(s.line_width());
            cr.move_to(4.0, side / 2.0);
            cr.line_to(side - 4.0, side / 2.0);
            cr.stroke()
        }
    }
}

/// Centre and radius of the undo arc, in design pixels.
const UNDO_CENTER: (f64, f64) = (12.0, 13.0);
const UNDO_RADIUS: f64 = 6.5;

/// Half width and length of the head of the undo arrow.
const UNDO_HEAD_HALF: f64 = 3.5;
const UNDO_HEAD_LEN: f64 = 5.0;

/// A counter-clockwise arc from the lower right over the top to the left, with
/// the head pointing down at its end.
fn undo_arrow(cr: &cairo::Context) -> Result<(), cairo::Error> {
    let (cx, cy) = UNDO_CENTER;
    let (start, end) = (30.0_f64.to_radians(), 190.0_f64.to_radians());
    cr.arc_negative(cx, cy, UNDO_RADIUS, start, end);
    cr.stroke()?;
    let e = (cx + UNDO_RADIUS * end.cos(), cy + UNDO_RADIUS * end.sin());
    // Direction of travel at the end of a counter-clockwise arc.
    let dir = (end.sin(), -end.cos());
    let perp = (-dir.1, dir.0);
    cr.move_to(e.0 + dir.0 * UNDO_HEAD_LEN, e.1 + dir.1 * UNDO_HEAD_LEN);
    cr.line_to(e.0 + perp.0 * UNDO_HEAD_HALF, e.1 + perp.1 * UNDO_HEAD_HALF);
    cr.line_to(e.0 - perp.0 * UNDO_HEAD_HALF, e.1 - perp.1 * UNDO_HEAD_HALF);
    cr.close_path();
    cr.fill()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Number of pixels painted by `icon` on a transparent surface.
    fn painted(icon: Icon) -> usize {
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, ICON_SIZE, ICON_SIZE).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            paint(&cr, icon, (0.0, 0.0, 0.0, 1.0), (ICON_SIZE as f64, ICON_SIZE as f64)).unwrap();
        }
        surface.flush();
        let data = surface.data().unwrap();
        data.chunks_exact(4).filter(|p| p[3] != 0).count()
    }

    #[test]
    fn every_icon_paints_something() {
        for icon in [Icon::Select, Icon::Line, Icon::Arrow, Icon::Text, Icon::Undo, Icon::Redo, Icon::Width(Size::Small)] {
            assert!(painted(icon) > 0, "{icon:?} is blank");
        }
    }

    #[test]
    fn the_size_icons_get_thicker() {
        let (s, m, l) = (painted(Icon::Width(Size::Small)), painted(Icon::Width(Size::Medium)), painted(Icon::Width(Size::Large)));
        assert!(s < m && m < l, "{s} {m} {l}");
    }
}
