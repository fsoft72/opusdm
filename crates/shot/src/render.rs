//! Drawing of the annotations with cairo. The editor canvas and the export use
//! the same [`draw`], so what the user sees is what gets saved.

use crate::annotations::{arrow_head, Annotation, Shape, LINE_HEIGHT_FACTOR};
use gtk4::cairo;
use image::RgbaImage;

/// Font family of the texts.
const FONT_FAMILY: &str = "sans-serif";

/// Draw `items` on `cr`, in image pixel coordinates, bottom item first.
pub fn draw(cr: &cairo::Context, items: &[Annotation]) -> Result<(), cairo::Error> {
    for item in items {
        cr.set_source_rgb(item.color.r, item.color.g, item.color.b);
        match &item.shape {
            Shape::Line { from, to } => stroke(cr, *from, *to, item.size.line_width())?,
            Shape::Arrow { from, to } => draw_arrow(cr, *from, *to, item.size.line_width())?,
            Shape::Text { at, text } => draw_text(cr, *at, text, item.size.font_size())?,
        }
    }
    Ok(())
}

/// A straight stroke with round ends.
fn stroke(cr: &cairo::Context, from: (f64, f64), to: (f64, f64), width: f64) -> Result<(), cairo::Error> {
    cr.set_line_width(width);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.move_to(from.0, from.1);
    cr.line_to(to.0, to.1);
    cr.stroke()
}

/// A line ending in a filled head; the stroke stops at the base of the head so
/// its round cap does not poke out of the tip.
fn draw_arrow(cr: &cairo::Context, from: (f64, f64), to: (f64, f64), width: f64) -> Result<(), cairo::Error> {
    let Some([tip, w1, w2]) = arrow_head(from, to, width) else { return Ok(()) };
    let base = ((w1.0 + w2.0) / 2.0, (w1.1 + w2.1) / 2.0);
    stroke(cr, from, base, width)?;
    cr.move_to(tip.0, tip.1);
    cr.line_to(w1.0, w1.1);
    cr.line_to(w2.0, w2.1);
    cr.close_path();
    cr.fill()
}

/// Text with its top left corner at `at`, one line per `\n`.
fn draw_text(cr: &cairo::Context, at: (f64, f64), text: &str, font_size: f64) -> Result<(), cairo::Error> {
    cr.select_font_face(FONT_FAMILY, cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(font_size);
    let ascent = cr.font_extents()?.ascent();
    for (i, line) in text.split('\n').enumerate() {
        cr.move_to(at.0, at.1 + ascent + i as f64 * font_size * LINE_HEIGHT_FACTOR);
        cr.show_text(line)?;
    }
    Ok(())
}

/// A cairo surface holding `base` (cairo wants premultiplied BGRA).
pub fn surface_from(base: &RgbaImage) -> Result<cairo::ImageSurface, String> {
    let (w, h) = (base.width() as i32, base.height() as i32);
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h)
        .map_err(|e| format!("cannot create a {w}x{h} surface: {e}"))?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface.data().map_err(|e| format!("cannot write the surface: {e}"))?;
        for (y, row) in base.rows().enumerate() {
            for (x, p) in row.enumerate() {
                let a = p[3] as u32;
                let premul = |c: u8| ((c as u32 * a + 127) / 255) as u8;
                let o = y * stride + x * 4;
                data[o..o + 4].copy_from_slice(&[premul(p[2]), premul(p[1]), premul(p[0]), p[3]]);
            }
        }
    }
    Ok(surface)
}

/// `base` with `items` drawn over it, at full resolution.
pub fn render(base: &RgbaImage, items: &[Annotation]) -> Result<RgbaImage, String> {
    let mut surface = surface_from(base)?;
    {
        let cr = cairo::Context::new(&surface).map_err(|e| format!("cannot draw: {e}"))?;
        draw(&cr, items).map_err(|e| format!("cannot draw the annotations: {e}"))?;
    }
    surface.flush();
    let (w, h) = (base.width(), base.height());
    let stride = surface.stride() as usize;
    let data = surface.data().map_err(|e| format!("cannot read the surface: {e}"))?;
    let mut out = RgbaImage::new(w, h);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let o = y * stride + x * 4;
            let a = data[o + 3];
            let unmul = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
            out.put_pixel(x as u32, y as u32, image::Rgba([unmul(data[o + 2]), unmul(data[o + 1]), unmul(data[o]), a]));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::{Annotation, Shape};
    use image::Rgba;

    const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);

    fn white(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, WHITE)
    }

    #[test]
    fn no_annotations_keeps_the_image() {
        let base = white(8, 8);
        assert_eq!(render(&base, &[]).unwrap(), base);
    }

    #[test]
    fn a_line_colours_its_pixels_only() {
        let line = Annotation::new(Shape::Line { from: (2.0, 20.0), to: (38.0, 20.0) });
        let out = render(&white(40, 40), &[line]).unwrap();
        let hit = out.get_pixel(20, 20);
        assert!(hit[0] > 200 && hit[1] < 80 && hit[2] < 80, "centre of the line is red: {hit:?}");
        assert_eq!(*out.get_pixel(20, 2), WHITE);
        assert_eq!(*out.get_pixel(0, 39), WHITE);
    }

    #[test]
    fn an_arrow_has_a_head_wider_than_its_shaft() {
        let arrow = Annotation::new(Shape::Arrow { from: (4.0, 30.0), to: (76.0, 30.0) });
        let out = render(&white(80, 60), &[arrow]).unwrap();
        let painted = |x: u32| (0..60).filter(|&y| *out.get_pixel(x, y) != WHITE).count();
        assert!(painted(66) > painted(20), "head {} vs shaft {}", painted(66), painted(20));
    }

    #[test]
    fn a_text_paints_something() {
        let text = Annotation::new(Shape::Text { at: (4.0, 4.0), text: "Hi".into() });
        let out = render(&white(80, 40), &[text]).unwrap();
        assert!(out.pixels().any(|p| *p != WHITE));
    }

    #[test]
    fn the_result_is_opaque_over_an_opaque_base() {
        let line = Annotation::new(Shape::Line { from: (0.0, 5.0), to: (20.0, 5.0) });
        let out = render(&white(20, 10), &[line]).unwrap();
        assert!(out.pixels().all(|p| p[3] == 255));
    }
}
