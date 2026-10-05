//! Icons of the control bar, drawn with cairo so they do not depend on the icon
//! theme and follow the text colour of the theme.

use gtk4 as gtk;
use gtk::cairo;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Side of an icon, in pixels.
pub const ICON_SIZE: i32 = 24;

/// Stroke width of the line icons.
const STROKE: f64 = 2.2;

/// Half width and length of an arrow head, in design pixels.
const HEAD_HALF: f64 = 3.5;
const HEAD_LEN: f64 = 5.0;

/// An icon of the control bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    /// Speaker with sound waves.
    Volume,
    /// Speaker crossed out.
    Muted,
    Loop,
}

/// A drawing area whose icon can be swapped, in the foreground colour of the widget.
#[derive(Clone)]
pub struct IconView {
    pub area: gtk::DrawingArea,
    icon: Rc<Cell<Icon>>,
}

impl IconView {
    /// A view showing `icon`.
    pub fn new(icon: Icon) -> Self {
        let cell = Rc::new(Cell::new(icon));
        let area = gtk::DrawingArea::new();
        area.set_content_width(ICON_SIZE);
        area.set_content_height(ICON_SIZE);
        let shown = cell.clone();
        area.set_draw_func(move |area, cr, w, h| {
            let c = area.color();
            let _ = paint(cr, shown.get(), (c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64), (w as f64, h as f64));
        });
        IconView { area, icon: cell }
    }

    /// Show another icon.
    pub fn set(&self, icon: Icon) {
        if self.icon.replace(icon) != icon { self.area.queue_draw(); }
    }
}

/// Draw `icon` in `rgba`, scaled from its 24 x 24 design to `size`.
pub fn paint(cr: &cairo::Context, icon: Icon, rgba: (f64, f64, f64, f64), size: (f64, f64)) -> Result<(), cairo::Error> {
    cr.scale(size.0 / ICON_SIZE as f64, size.1 / ICON_SIZE as f64);
    cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_width(STROKE);
    match icon {
        Icon::Play => {
            cr.move_to(7.0, 4.0);
            cr.line_to(19.0, 12.0);
            cr.line_to(7.0, 20.0);
            cr.close_path();
            cr.fill()
        }
        Icon::Pause => {
            cr.rectangle(6.0, 4.0, 4.0, 16.0);
            cr.rectangle(14.0, 4.0, 4.0, 16.0);
            cr.fill()
        }
        Icon::Volume => {
            speaker(cr)?;
            // Two sound waves.
            for radius in [5.0, 8.5] {
                cr.new_sub_path();
                cr.arc(12.5, 12.0, radius, -0.7, 0.7);
                cr.stroke()?;
            }
            Ok(())
        }
        Icon::Muted => {
            speaker(cr)?;
            cr.move_to(16.0, 9.0);
            cr.line_to(21.0, 15.0);
            cr.move_to(21.0, 9.0);
            cr.line_to(16.0, 15.0);
            cr.stroke()
        }
        Icon::Loop => loop_arrow(cr),
    }
}

/// The speaker body shared by the volume and muted icons.
fn speaker(cr: &cairo::Context) -> Result<(), cairo::Error> {
    cr.move_to(3.0, 9.0);
    for (x, y) in [(7.0, 9.0), (12.0, 5.0), (12.0, 19.0), (7.0, 15.0), (3.0, 15.0)] {
        cr.line_to(x, y);
    }
    cr.close_path();
    cr.fill()
}

/// A nearly closed circle with an arrow head at its end, the repeat symbol.
fn loop_arrow(cr: &cairo::Context) -> Result<(), cairo::Error> {
    let (cx, cy, radius) = (12.0, 12.0, 7.0);
    let (start, end) = (-60.0_f64.to_radians(), 210.0_f64.to_radians());
    cr.arc(cx, cy, radius, start, end);
    cr.stroke()?;
    let e = (cx + radius * end.cos(), cy + radius * end.sin());
    // Direction of travel at the end of a clockwise arc.
    let dir = (-end.sin(), end.cos());
    let perp = (-dir.1, dir.0);
    cr.move_to(e.0 + dir.0 * HEAD_LEN, e.1 + dir.1 * HEAD_LEN);
    cr.line_to(e.0 + perp.0 * HEAD_HALF, e.1 + perp.1 * HEAD_HALF);
    cr.line_to(e.0 - perp.0 * HEAD_HALF, e.1 - perp.1 * HEAD_HALF);
    cr.close_path();
    cr.fill()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 5] = [Icon::Play, Icon::Pause, Icon::Volume, Icon::Muted, Icon::Loop];

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
        for icon in ALL {
            assert!(painted(icon) > 0, "{icon:?} is blank");
        }
    }

    #[test]
    fn the_icons_differ() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(painted(*a), painted(*b), "{a:?} and {b:?} look the same");
            }
        }
    }
}
