//! Drawing of the window switcher with cairo and pango into a memory
//! buffer the nested backend shows above the window menu.
//!
//! The box is `SWITCHER_WIDTH` wide; inside its border and padding one row
//! of `SWITCHER_ROW_HEIGHT` per entry: the title (ellipsized) on the left,
//! the app id dimmed on the right, the selected row in the active colours
//! of the window menu (`MenuTheme`).

use pangocairo::pango;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;

use crate::draw::{fill_gradient, paint_buffer, set_rgb};
use crate::geometry::Rect;
use crate::menu::place_menu;
use crate::menu::theme::MenuTheme;

/// Width of the switcher box, in pixels.
pub const SWITCHER_WIDTH: i32 = 480;

/// Height of one switcher row, in pixels.
pub const SWITCHER_ROW_HEIGHT: i32 = 28;

/// Font of the switcher rows.
const SWITCHER_FONT: &str = "Sans 10";

/// Title shown for a window that has none.
const NO_TITLE: &str = "(no title)";

/// The app id takes at most this share of the row width.
const APP_ID_MAX_SHARE: f64 = 0.35;

/// Room between the title and the app id, in pixels.
const COLUMN_GAP: f64 = 12.0;

/// What one row of the switcher shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherRow {
    /// The window title (`xdg_toplevel.set_title`), may be empty.
    pub title: String,
    /// The app id (`xdg_toplevel.set_app_id`), may be empty.
    pub app_id: String,
}

/// Size of a switcher of `rows` rows with `theme`: `SWITCHER_WIDTH` x
/// (`rows` * `SWITCHER_ROW_HEIGHT` + 2 * padding + 2 * border).
pub fn switcher_size(rows: usize, theme: &MenuTheme) -> (i32, i32) {
    let rows = i32::try_from(rows).unwrap_or(i32::MAX / SWITCHER_ROW_HEIGHT);
    let frame = 2 * _metric(theme.padding) + 2 * _metric(theme.border_width);
    (SWITCHER_WIDTH, rows.saturating_mul(SWITCHER_ROW_HEIGHT).saturating_add(frame))
}

/// A theme metric (`u32`) as `i32`, clamped.
fn _metric(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX / 4)
}

/// Where a switcher of `size` goes on `output`: centred, kept inside (a
/// box taller than the output sticks to its top edge).
pub fn switcher_rect(size: (i32, i32), output: Rect) -> Rect {
    let anchor = (output.x + (output.w - size.0) / 2, output.y + (output.h - size.1) / 2);
    let (x, y) = place_menu(anchor, size, output);
    Rect { x, y, w: size.0, h: size.1 }
}

/// Draws the switcher rows `rows` (the row `selected` highlighted) at
/// `scale` into an ARGB8888 buffer of `switcher_size` times `scale`.
pub fn render_switcher(rows: &[SwitcherRow], selected: usize, theme: &MenuTheme, scale: i32) -> Result<MemoryRenderBuffer, String> {
    paint_buffer("window switcher", switcher_size(rows.len(), theme), scale, |cr| _paint(cr, rows, selected, theme))
}

/// Paints the switcher on `cr` in logical pixels: the border, the items
/// background gradient inside it, then each row clipped to its box (the
/// active gradient behind `selected`, the title, the app id dimmed).
fn _paint(cr: &cairo::Context, rows: &[SwitcherRow], selected: usize, theme: &MenuTheme) -> Result<(), cairo::Error> {
    let (w, h) = switcher_size(rows.len(), theme);
    let (w, h) = (f64::from(w), f64::from(h));
    let bw = f64::from(theme.border_width).min(w / 2.0).min(h / 2.0);
    set_rgb(cr, theme.border);
    cr.rectangle(0.0, 0.0, w, h);
    cr.fill()?;
    if w - 2.0 * bw <= 0.0 || h - 2.0 * bw <= 0.0 { return Ok(()); }
    fill_gradient(cr, theme.item_bg, bw, bw, w - 2.0 * bw, h - 2.0 * bw)?;
    let pad = f64::from(theme.padding);
    let (x0, x1) = (bw + pad, w - bw - pad);
    if x1 <= x0 { return Ok(()); }
    let layout = pangocairo::functions::create_layout(cr);
    layout.set_font_description(Some(&pango::FontDescription::from_string(SWITCHER_FONT)));
    layout.set_ellipsize(pango::EllipsizeMode::End);
    let row_h = f64::from(SWITCHER_ROW_HEIGHT);
    for (i, row) in rows.iter().enumerate() {
        let y = bw + pad + row_h * i as f64;
        cr.save()?;
        cr.rectangle(x0, y, x1 - x0, row_h);
        cr.clip();
        let drawn = _paint_row(cr, &layout, row, i == selected, theme, (x0, y, x1 - x0, row_h));
        cr.restore()?;
        drawn?;
    }
    Ok(())
}

/// Paints one row inside the box `(x, y, w, h)`: the app id on the right
/// (dimmed, at most `APP_ID_MAX_SHARE` of the width), the title on the left
/// in the rest, both ellipsized.
fn _paint_row(
    cr: &cairo::Context,
    layout: &pango::Layout,
    row: &SwitcherRow,
    active: bool,
    theme: &MenuTheme,
    (x, y, w, h): (f64, f64, f64, f64),
) -> Result<(), cairo::Error> {
    if active {
        fill_gradient(cr, theme.active_bg, x, y, w, h)?;
    }
    let pad = f64::from(theme.padding).max(4.0);
    let inner = (w - 2.0 * pad).max(0.0);
    let mut title_width = inner;
    if !row.app_id.is_empty() {
        layout.set_width(((inner * APP_ID_MAX_SHARE) * f64::from(pango::SCALE)) as i32);
        layout.set_text(&row.app_id);
        let (aw, ah) = layout.pixel_size();
        let aw = f64::from(aw);
        set_rgb(cr, theme.disabled_text);
        cr.move_to(x + w - pad - aw, y + (h - f64::from(ah)) / 2.0);
        pangocairo::functions::show_layout(cr, layout);
        title_width = (inner - aw - COLUMN_GAP).max(0.0);
    }
    set_rgb(cr, if active { theme.active_text } else { theme.item_text });
    layout.set_width((title_width * f64::from(pango::SCALE)) as i32);
    layout.set_text(if row.title.is_empty() { NO_TITLE } else { &row.title });
    let (_, th) = layout.pixel_size();
    cr.move_to(x + pad, y + (h - f64::from(th)) / 2.0);
    pangocairo::functions::show_layout(cr, layout);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::buffer_bytes;

    /// `n` rows with short titles.
    fn rows(n: usize) -> Vec<SwitcherRow> {
        (0..n).map(|i| SwitcherRow { title: format!("window {i}"), app_id: "foot".into() }).collect()
    }

    /// The ARGB8888 pixel `(x, y)` of `data`, an image `width` pixels wide,
    /// as `(r, g, b)`.
    fn pixel(data: &[u8], width: i32, x: i32, y: i32) -> (u8, u8, u8) {
        let i = ((y * width + x) * 4) as usize;
        (data[i + 2], data[i + 1], data[i])
    }

    #[test]
    fn render_switcher_returns_a_buffer_of_the_switcher_size() {
        let theme = MenuTheme::default();
        let pad = theme.padding as i32;
        let border = theme.border_width as i32;
        let height = 3 * SWITCHER_ROW_HEIGHT + 2 * pad + 2 * border;
        assert_eq!(switcher_size(3, &theme), (SWITCHER_WIDTH, height));
        let mut buffer = render_switcher(&rows(3), 1, &theme, 1).expect("rendered");
        assert_eq!(buffer_bytes(&mut buffer).len(), (SWITCHER_WIDTH * height * 4) as usize);
        let mut buffer = render_switcher(&rows(3), 1, &theme, 2).expect("rendered at scale 2");
        assert_eq!(buffer_bytes(&mut buffer).len(), (SWITCHER_WIDTH * 2 * height * 2 * 4) as usize);
    }

    #[test]
    fn the_border_is_drawn_and_the_selected_row_is_highlighted() {
        let theme = MenuTheme::default();
        let top = (theme.border_width + theme.padding) as i32;
        let mut buffer = render_switcher(&rows(3), 1, &theme, 1).expect("rendered");
        let data = buffer_bytes(&mut buffer);
        assert_eq!(pixel(&data, SWITCHER_WIDTH, 0, 0), (0x18, 0x18, 0x18), "border at the corner");
        let x = SWITCHER_WIDTH - top - 2;
        let (r, g, b) = pixel(&data, SWITCHER_WIDTH, x, top + SWITCHER_ROW_HEIGHT + 2);
        assert!(b > r && b > 0x80, "the selected second row has the blue active gradient, got {:?}", (r, g, b));
        let (r, g, b) = pixel(&data, SWITCHER_WIDTH, x, top + 2);
        assert!(r == g && g == b && r < 0x40, "the first row has the dark items gradient, got {:?}", (r, g, b));
    }

    #[test]
    fn a_long_title_is_drawn_inside_the_box() {
        let theme = MenuTheme::default();
        let long = vec![SwitcherRow { title: "x".repeat(500), app_id: "a.very.long.application.id".into() }];
        let mut buffer = render_switcher(&long, 0, &theme, 1).expect("rendered");
        let data = buffer_bytes(&mut buffer);
        assert_eq!(pixel(&data, SWITCHER_WIDTH, SWITCHER_WIDTH - 1, 10), (0x18, 0x18, 0x18), "the right border is intact");
    }

    #[test]
    fn the_switcher_is_centred_and_kept_inside_the_output() {
        let output = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        assert_eq!(switcher_rect((480, 100), output), Rect { x: 720, y: 490, w: 480, h: 100 });
        assert_eq!(switcher_rect((480, 2000), output), Rect { x: 720, y: 0, w: 480, h: 2000 });
        let shifted = Rect { x: 100, y: 50, w: 800, h: 600 };
        assert_eq!(switcher_rect((480, 100), shifted), Rect { x: 260, y: 300, w: 480, h: 100 });
    }
}
