//! Drawing of the server side frame of a decorated window: the title bar
//! image (top border, title gradient, button glyphs, title text) drawn with
//! cairo and pango, and the rectangles of the other three borders, which
//! the backend fills with solid colour.
//!
//! Ownership: a `WindowDecor` (the cached title image and the border
//! buffers of one window) belongs to the backend that draws it (the nested
//! backend keeps one per decorated window id and drops the entries of the
//! windows that are no longer drawn).

use pangocairo::pango;
use smithay::backend::renderer::element::{memory::MemoryRenderBuffer, solid::SolidColorBuffer};

use crate::decor::geometry::{button_rects, insets, title_bar_rect, title_text_rect, Insets, TitleButton, TitleLayout, TITLE_HEIGHT};
use crate::decor::theme::DecorTheme;
use crate::draw::{components, fill_gradient, paint_buffer, set_rgb, RenderCache};
use crate::geometry::Rect;
use crate::menu::theme::Rgb;

/// Font of the title text (Openbox `window.active.label.text.font` of the
/// X11 session's rc.xml).
pub const TITLE_FONT: &str = "Sans Bold 9";

/// Space between a button's edges and its glyph, in pixels.
const GLYPH_INSET: f64 = 4.0;

/// Width of the strokes of the button glyphs, in pixels.
const GLYPH_LINE: f64 = 2.0;

/// What a title bar image shows: the cache key of the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleState {
    /// The window title (`xdg_toplevel.set_title`), may be empty.
    pub title: String,
    /// True for the active window (its colours), false for the others.
    pub active: bool,
    /// Width of the frame (and of the image), in pixels.
    pub width: i32,
    /// The button under the pointer, if any.
    pub hovered: Option<TitleButton>,
    /// The button pressed (and still under the pointer), if any.
    pub pressed: Option<TitleButton>,
}

/// Size of the title bar image of a frame `width` pixels wide: the frame
/// width by the top border plus `TITLE_HEIGHT`.
pub fn title_image_size(width: i32, theme: &DecorTheme) -> (i32, i32) {
    (width, TITLE_HEIGHT.saturating_add(insets(true, false, theme).left))
}

/// Draws the title bar of `state` with the buttons of `layout` and the
/// colours of `theme` at `scale` into an opaque ARGB8888 buffer of
/// `title_image_size` times `scale`: the top border and the side borders
/// beside the bar, the vertical title gradient, each button (its hover or
/// pressed background, its glyph) and the title centered between the
/// buttons, ellipsized.
pub fn render_title(state: &TitleState, layout: &TitleLayout, theme: &DecorTheme, scale: i32) -> Result<MemoryRenderBuffer, String> {
    paint_buffer("title bar", title_image_size(state.width, theme), scale, |cr| _paint_title(cr, state, layout, theme))
}

/// The rectangles of the left, right and bottom borders of `frame` with
/// `insets` (the top border and the title bar are the title image): the
/// side borders from under the title bar to above the bottom border, the
/// bottom border the whole frame width.
pub fn border_rects(frame: Rect, insets: Insets) -> [Rect; 3] {
    let side_h = frame.h.saturating_sub(insets.top).saturating_sub(insets.bottom).max(0);
    let y = frame.y.saturating_add(insets.top);
    [
        Rect { x: frame.x, y, w: insets.left, h: side_h },
        Rect { x: frame.x.saturating_add(frame.w).saturating_sub(insets.right), y, w: insets.right, h: side_h },
        Rect { x: frame.x, y: frame.y.saturating_add(frame.h).saturating_sub(insets.bottom), w: frame.w, h: insets.bottom },
    ]
}

/// The key of a cached title image: what it shows, the layout and the
/// theme it was drawn with, and the scale.
type TitleKey = (TitleState, TitleLayout, DecorTheme, i32);

/// The drawn frame of one window: its title image, drawn again only when
/// its `TitleState` (or layout, theme, scale) changed, and one solid
/// buffer per border (each keeps its id for the whole life of the window,
/// so the damage tracker only redraws what changed).
#[derive(Debug, Default)]
pub struct WindowDecor {
    /// The last title image and the state it was drawn for.
    title: RenderCache<TitleKey, MemoryRenderBuffer>,
    /// The left, right and bottom border buffers.
    borders: [SolidColorBuffer; 3],
}

impl WindowDecor {
    /// The title image of `state` (cached; a failed draw is cached too).
    pub fn title_image(&mut self, state: TitleState, layout: &TitleLayout, theme: &DecorTheme, scale: i32) -> Result<MemoryRenderBuffer, String> {
        let key = (state.clone(), layout.clone(), *theme, scale);
        self.title.get_or_draw(key, || render_title(&state, layout, theme, scale))
    }

    /// The border buffers of `frame` with `insets`, sized and coloured
    /// `colour`, with their rectangles (left, right, bottom).
    pub fn borders(&mut self, frame: Rect, insets: Insets, colour: Rgb) -> Vec<(SolidColorBuffer, Rect)> {
        let (r, g, b) = components(colour);
        let rgba = [r as f32, g as f32, b as f32, 1.0];
        self.borders
            .iter_mut()
            .zip(border_rects(frame, insets))
            .map(|(buffer, rect)| {
                buffer.update((rect.w, rect.h), rgba);
                (buffer.clone(), rect)
            })
            .collect()
    }

    /// How many title images were drawn (read by the tests).
    #[cfg(test)]
    pub fn title_builds(&self) -> u32 {
        self.title.builds()
    }
}

/// Paints the title image on `cr` in logical pixels (see `render_title`).
fn _paint_title(cr: &cairo::Context, state: &TitleState, layout: &TitleLayout, theme: &DecorTheme) -> Result<(), cairo::Error> {
    let (w, h) = title_image_size(state.width, theme);
    let border = if state.active { theme.active_border } else { theme.inactive_border };
    set_rgb(cr, border);
    cr.rectangle(0.0, 0.0, f64::from(w), f64::from(h));
    cr.fill()?;
    let frame = Rect { x: 0, y: 0, w, h };
    let frame_insets = insets(true, false, theme);
    let Some(bar) = title_bar_rect(frame, frame_insets).filter(|bar| bar.w > 0) else { return Ok(()) };
    let colours = if state.active { theme.active_title } else { theme.inactive_title };
    fill_gradient(cr, colours, f64::from(bar.x), f64::from(bar.y), f64::from(bar.w), f64::from(bar.h))?;
    cr.save()?;
    cr.rectangle(f64::from(bar.x), f64::from(bar.y), f64::from(bar.w), f64::from(bar.h));
    cr.clip();
    let drawn = _paint_bar(cr, state, layout, theme, frame, frame_insets);
    cr.restore()?;
    drawn
}

/// Paints the buttons and the title inside the title bar (clipped to it).
fn _paint_bar(
    cr: &cairo::Context,
    state: &TitleState,
    layout: &TitleLayout,
    theme: &DecorTheme,
    frame: Rect,
    frame_insets: Insets,
) -> Result<(), cairo::Error> {
    let glyph = if state.active { theme.active_button } else { theme.inactive_button };
    for (button, rect) in button_rects(frame, frame_insets, layout) {
        let (x, y, w, h) = (f64::from(rect.x), f64::from(rect.y), f64::from(rect.w), f64::from(rect.h));
        if state.pressed == Some(button) {
            fill_gradient(cr, theme.pressed_bg, x, y, w, h)?;
        } else if state.hovered == Some(button) && state.active {
            // OpusDMTheme: `window.inactive.button.*.hover.bg: parentrelative`, no hover colour.
            fill_gradient(cr, theme.hover_bg, x, y, w, h)?;
        }
        set_rgb(cr, glyph);
        _paint_glyph(cr, button, (x, y, w, h))?;
    }
    let Some(text) = title_text_rect(frame, frame_insets, layout).filter(|r| r.w > 0) else { return Ok(()) };
    let pango_layout = pangocairo::functions::create_layout(cr);
    pango_layout.set_font_description(Some(&pango::FontDescription::from_string(TITLE_FONT)));
    pango_layout.set_ellipsize(pango::EllipsizeMode::End);
    pango_layout.set_alignment(pango::Alignment::Center);
    pango_layout.set_single_paragraph_mode(true);
    pango_layout.set_width(text.w.saturating_mul(pango::SCALE));
    pango_layout.set_text(&state.title);
    let (_, th) = pango_layout.pixel_size();
    set_rgb(cr, if state.active { theme.active_label } else { theme.inactive_label });
    cr.move_to(f64::from(text.x), f64::from(text.y) + f64::from(text.h - th) / 2.0);
    pangocairo::functions::show_layout(cr, &pango_layout);
    Ok(())
}

/// Paints the glyph of `button` in the box `(x, y, w, h)` with the current
/// source: an X for close, a bar at the bottom for iconify, a box with a
/// thick top for maximize.
fn _paint_glyph(cr: &cairo::Context, button: TitleButton, (x, y, w, h): (f64, f64, f64, f64)) -> Result<(), cairo::Error> {
    let (x0, y0) = (x + GLYPH_INSET, y + GLYPH_INSET);
    let (x1, y1) = (x + w - GLYPH_INSET, y + h - GLYPH_INSET);
    if x1 <= x0 || y1 <= y0 { return Ok(()); }
    match button {
        TitleButton::Close => {
            cr.set_line_width(GLYPH_LINE);
            cr.move_to(x0, y0);
            cr.line_to(x1, y1);
            cr.move_to(x1, y0);
            cr.line_to(x0, y1);
            cr.stroke()
        }
        TitleButton::Iconify => {
            cr.rectangle(x0, y1 - GLYPH_LINE, x1 - x0, GLYPH_LINE);
            cr.fill()
        }
        TitleButton::Maximize => {
            cr.set_line_width(1.0);
            cr.rectangle(x0 + 0.5, y0 + 0.5, x1 - x0 - 1.0, y1 - y0 - 1.0);
            cr.stroke()?;
            cr.rectangle(x0, y0, x1 - x0, GLYPH_LINE);
            cr.fill()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::buffer_bytes;

    /// The ARGB8888 pixel `(x, y)` of `data`, an image `width` pixels wide,
    /// as `(r, g, b)`.
    fn pixel(data: &[u8], width: i32, x: i32, y: i32) -> (u8, u8, u8) {
        let i = ((y * width + x) * 4) as usize;
        (data[i + 2], data[i + 1], data[i])
    }

    /// A title state of a 400 pixels wide frame.
    fn state(active: bool) -> TitleState {
        TitleState { title: "foot".into(), active, width: 400, hovered: None, pressed: None }
    }

    /// The default title layout, `"close,iconify,max:"`.
    fn layout() -> TitleLayout {
        TitleLayout::parse("close,iconify,max:")
    }

    #[test]
    fn render_title_returns_a_frame_width_by_title_and_border_buffer() {
        let theme = DecorTheme::default();
        let mut buffer = render_title(&state(true), &layout(), &theme, 1).expect("rendered");
        assert_eq!(buffer_bytes(&mut buffer).len(), (400 * (24 + 4) * 4) as usize);
        let mut buffer = render_title(&state(true), &layout(), &theme, 2).expect("rendered at scale 2");
        assert_eq!(buffer_bytes(&mut buffer).len(), (800 * 56 * 4) as usize);
    }

    #[test]
    fn the_active_title_is_dark_and_the_inactive_one_light() {
        let theme = DecorTheme::default();
        let mut active = render_title(&state(true), &layout(), &theme, 1).expect("rendered");
        let data = buffer_bytes(&mut active);
        assert_eq!(pixel(&data, 400, 0, 0), (0x18, 0x18, 0x18), "active border at the corner");
        let (r, _, _) = pixel(&data, 400, 390, 6);
        assert!((0x20..=0x41).contains(&r), "dark active gradient near the top, got {r:#x}");
        let mut inactive = render_title(&state(false), &layout(), &theme, 1).expect("rendered");
        let data = buffer_bytes(&mut inactive);
        assert_eq!(pixel(&data, 400, 0, 0), (0x40, 0x40, 0x40), "inactive border");
        let (r, _, _) = pixel(&data, 400, 390, 6);
        assert!(r >= 0xde, "light inactive gradient, got {r:#x}");
    }

    #[test]
    fn a_hovered_button_has_the_hover_background_and_a_pressed_one_the_pressed_background() {
        let theme = DecorTheme::default();
        // The close button of a 400 wide frame: (8, 8) to (24, 24); (9, 9) is its background.
        let hovered = TitleState { hovered: Some(TitleButton::Close), ..state(true) };
        let data = buffer_bytes(&mut render_title(&hovered, &layout(), &theme, 1).expect("rendered"));
        let (r, g, b) = pixel(&data, 400, 9, 9);
        assert!(b > r && b > 0x80 && g > 0x60, "blue hover background, got {:?}", (r, g, b));
        let pressed = TitleState { pressed: Some(TitleButton::Close), ..state(true) };
        let data = buffer_bytes(&mut render_title(&pressed, &layout(), &theme, 1).expect("rendered"));
        let (r, g, b) = pixel(&data, 400, 9, 9);
        assert!(b > r && b < 0x80 && b > 0x40, "dark blue pressed background, got {:?}", (r, g, b));
        let inactive_hover = TitleState { hovered: Some(TitleButton::Close), ..state(false) };
        let data = buffer_bytes(&mut render_title(&inactive_hover, &layout(), &theme, 1).expect("rendered"));
        let (r, _, b) = pixel(&data, 400, 9, 9);
        assert!(r >= 0xd0 && b >= 0xd0, "no hover colour on an inactive window, got {:?}", (r, b));
    }

    #[test]
    fn the_close_glyph_is_drawn_in_the_button_colour() {
        let theme = DecorTheme::default();
        let data = buffer_bytes(&mut render_title(&state(true), &layout(), &theme, 1).expect("rendered"));
        // The X crosses at the centre of the close button (16, 16).
        let (r, _, _) = pixel(&data, 400, 16, 16);
        assert!(r >= 0xc0, "light glyph on the dark bar, got {r:#x}");
    }

    #[test]
    fn a_long_title_and_a_tiny_frame_stay_inside_the_image() {
        let theme = DecorTheme::default();
        let long = TitleState { title: "x".repeat(500), ..state(true) };
        let data = buffer_bytes(&mut render_title(&long, &layout(), &theme, 1).expect("rendered"));
        assert_eq!(pixel(&data, 400, 399, 10), (0x18, 0x18, 0x18), "the right border is intact");
        let tiny = TitleState { width: 6, ..state(true) };
        assert!(render_title(&tiny, &layout(), &theme, 1).is_ok(), "a frame narrower than its buttons still draws");
        let empty = TitleState { width: 0, ..state(true) };
        assert!(render_title(&empty, &layout(), &theme, 1).is_err(), "an empty image is an error");
    }

    #[test]
    fn the_borders_surround_the_client_under_the_title() {
        let frame = Rect { x: 100, y: 50, w: 408, h: 332 };
        let i = insets(true, false, &DecorTheme::default());
        assert_eq!(border_rects(frame, i), [
            Rect { x: 100, y: 78, w: 4, h: 300 },
            Rect { x: 504, y: 78, w: 4, h: 300 },
            Rect { x: 100, y: 378, w: 408, h: 4 },
        ]);
    }

    #[test]
    fn the_title_image_is_drawn_again_only_when_its_state_changes() {
        let theme = DecorTheme::default();
        let mut decor = WindowDecor::default();
        decor.title_image(state(true), &layout(), &theme, 1).expect("drawn");
        decor.title_image(state(true), &layout(), &theme, 1).expect("cached");
        assert_eq!(decor.title_builds(), 1);
        decor.title_image(state(false), &layout(), &theme, 1).expect("drawn inactive");
        let hovered = TitleState { hovered: Some(TitleButton::Maximize), ..state(false) };
        decor.title_image(hovered, &layout(), &theme, 1).expect("drawn hovered");
        decor.title_image(TitleState { title: "other".into(), ..state(false) }, &layout(), &theme, 1).expect("drawn retitled");
        assert_eq!(decor.title_builds(), 4);
    }
}
