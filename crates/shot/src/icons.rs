//! Icons of the tool bar, drawn with cairo so they do not depend on the icon
//! theme and follow the text colour of the theme.

use crate::annotations::Size;
use gtk4 as gtk;
use gtk::cairo;
use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, PropMode};
use x11rb::wrapper::ConnectionExt as _;

/// The camera drawing of `opusdm-shot` (outline, so it reads on a light and on
/// a dark theme), embedded at compile time. Its black is [`CAMERA_INK`]; the
/// widget swaps it for the foreground colour of the theme.
const SVG_CAMERA: &[u8] = include_bytes!("../../../assets/apps/icons/photo-camera-light.svg");

/// The colour the camera SVG is drawn in, replaced when tinting.
const CAMERA_INK: &str = "#231F20";

/// `svg` with every [`CAMERA_INK`] replaced by `rgba` as `#rrggbb`.
fn tint_camera(svg: &[u8], rgba: (f64, f64, f64, f64)) -> Vec<u8> {
    let channel = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let hex = format!("#{:02x}{:02x}{:02x}", channel(rgba.0), channel(rgba.1), channel(rgba.2));
    String::from_utf8_lossy(svg).replace(CAMERA_INK, &hex).into_bytes()
}

/// Side, in pixels, of the camera drawn into `_NET_WM_ICON`.
const WINDOW_ICON_SIZE: i32 = 48;

/// Side the camera SVG declares in its `width` / `height`.
const CAMERA_DECLARED_SIDE: &str = "800px";

/// The camera as `_NET_WM_ICON` words: width, height, then one `ARGB` word
/// per pixel (not premultiplied), drawn in `rgba`. `None` when the SVG cannot
/// be rendered (logged by the caller).
fn camera_icon_words(rgba: (f64, f64, f64, f64), side: i32) -> Result<Vec<u32>, String> {
    let svg = String::from_utf8_lossy(&tint_camera(SVG_CAMERA, rgba)).replace(CAMERA_DECLARED_SIDE, &format!("{side}px"));
    let texture = gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(svg.into_bytes())).map_err(|e| e.to_string())?;
    let (w, h) = (texture.width() as usize, texture.height() as usize);
    if w == 0 || h == 0 { return Err("the camera SVG has no size".to_string()); }
    // Native download format: premultiplied ARGB, one little endian word per pixel.
    let mut data = vec![0u8; w * h * 4];
    texture.download(&mut data, w * 4);
    let mut words = vec![w as u32, h as u32];
    words.extend(data.as_chunks::<4>().0.iter().map(|px| {
        let word = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
        let a = word >> 24;
        if a == 0 || a == 255 { return word; }
        let un = |c: u32| ((c * 255 + a / 2) / a).min(255);
        (a << 24) | (un((word >> 16) & 0xff) << 16) | (un((word >> 8) & 0xff) << 8) | un(word & 0xff)
    }));
    Ok(words)
}

/// Give `window` the camera as its icon in window lists. GTK4 does not write
/// `_NET_WM_ICON` on X11, so without this the panel shows a generic icon. Done
/// when the window is mapped (it needs its X11 id and its theme colour); under
/// Wayland nothing is done, the compositor finds the icon through the desktop
/// entry of the application id. Failures are logged.
pub fn set_window_icon(window: &gtk::ApplicationWindow) {
    window.connect_map(|window| {
        let Some(xid) = opusdm_ui::x11_window_id(window) else { return };
        let c = window.color();
        let words = match camera_icon_words((c.red() as f64, c.green() as f64, c.blue() as f64, 1.0), WINDOW_ICON_SIZE) {
            Ok(words) => words,
            Err(e) => return opusdm_ipc::err_log!("opusdm-shot", "window icon: {e}"),
        };
        let result = opusdm_ui::with_x11(|x11| {
            let atom = x11.atom("_NET_WM_ICON")?;
            x11.conn().change_property32(PropMode::REPLACE, xid, atom, AtomEnum::CARDINAL, &words)?;
            x11.conn().flush()?;
            Ok(())
        });
        if let Err(e) = result { opusdm_ipc::err_log!("opusdm-shot", "window icon: {e}"); }
    });
}

/// The camera as an image of `size` pixels, in the foreground colour of the
/// theme. The colour is read when the widget is mapped.
pub fn camera_image(size: i32) -> gtk::Image {
    let image = gtk::Image::new();
    image.set_pixel_size(size);
    image.connect_map(|image| {
        let c = image.color();
        let svg = tint_camera(SVG_CAMERA, (c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64));
        match gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(svg)) {
            Ok(texture) => image.set_paintable(Some(&texture)),
            Err(e) => opusdm_ipc::err_log!("opusdm-shot", "camera icon: {e}"),
        }
    });
    image
}

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
    Rect,
    Pixelate,
    Blur,
    Crop,
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
        Icon::Rect => {
            cr.rectangle(4.0, 6.0, 16.0, 12.0);
            cr.fill()
        }
        Icon::Pixelate => {
            // A 3 x 3 checkerboard of squares.
            for row in 0..3 {
                for col in 0..3 {
                    if (row + col) % 2 == 0 { cr.rectangle(4.0 + col as f64 * 5.0, 4.0 + row as f64 * 5.0, 5.0, 5.0); }
                }
            }
            cr.fill()
        }
        Icon::Blur => {
            // A disc fading out: three rings, fainter outwards.
            let (alpha_base, centre) = (rgba.3, 12.0);
            for (radius, fade) in [(3.5, 1.0), (7.0, 0.55), (10.0, 0.25)] {
                cr.set_source_rgba(rgba.0, rgba.1, rgba.2, alpha_base * fade);
                cr.arc(centre, centre, radius, 0.0, std::f64::consts::TAU);
                if fade == 1.0 { cr.fill()?; } else { cr.stroke()?; }
            }
            Ok(())
        }
        Icon::Crop => {
            // The two overlapping corners of a crop mark.
            cr.move_to(7.0, 3.0);
            cr.line_to(7.0, 17.0);
            cr.line_to(21.0, 17.0);
            cr.move_to(3.0, 7.0);
            cr.line_to(17.0, 7.0);
            cr.line_to(17.0, 21.0);
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
        data.as_chunks::<4>().0.iter().filter(|p| p[3] != 0).count()
    }

    #[test]
    fn the_camera_svg_loads_in_any_colour() {
        gtk::init().ok();
        for rgba in [(0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0, 1.0)] {
            let svg = tint_camera(SVG_CAMERA, rgba);
            assert!(!String::from_utf8_lossy(&svg).contains(CAMERA_INK));
            assert!(gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(svg)).is_ok());
        }
    }

    #[test]
    fn the_window_icon_has_the_declared_size_and_some_visible_pixels() {
        gtk::init().ok();
        let words = camera_icon_words((1.0, 1.0, 1.0, 1.0), 48).unwrap();
        assert_eq!((words[0], words[1]), (48, 48));
        assert_eq!(words.len(), 2 + 48 * 48);
        assert!(words[2..].iter().any(|w| w >> 24 != 0), "blank icon");
        assert!(words[2..].iter().any(|w| w >> 24 == 255 && w & 0x00ff_ffff == 0x00ff_ffff), "no opaque white pixel");
    }

    #[test]
    fn the_camera_is_tinted_with_the_given_colour() {
        let svg = String::from_utf8(tint_camera(SVG_CAMERA, (1.0, 0.5, 0.0, 1.0))).unwrap();
        assert!(svg.contains("#ff8000"));
    }

    #[test]
    fn every_icon_paints_something() {
        for icon in [Icon::Select, Icon::Line, Icon::Arrow, Icon::Text, Icon::Rect, Icon::Pixelate, Icon::Blur, Icon::Crop, Icon::Undo, Icon::Redo, Icon::Width(Size::Small)] {
            assert!(painted(icon) > 0, "{icon:?} is blank");
        }
    }

    #[test]
    fn the_size_icons_get_thicker() {
        let (s, m, l) = (painted(Icon::Width(Size::Small)), painted(Icon::Width(Size::Medium)), painted(Icon::Width(Size::Large)));
        assert!(s < m && m < l, "{s} {m} {l}");
    }
}
