//! The zone preview: a translucent rectangle with a border, drawn on a
//! click-through override-redirect ARGB window (so it needs a compositor).
//! It fades in and slides between zones. The animation maths is pure and unit
//! tested; the X11 part only applies what the maths says.

use crate::x11::Display;
use crate::zones::Rect;
use std::error::Error;
use x11rb::connection::Connection;
use x11rb::protocol::shape::{self, SK, SO};
use x11rb::protocol::xproto::{
    self, ChangeGCAux, ChangeWindowAttributesAux, ColormapAlloc, ConfigureWindowAux, ConnectionExt, CreateGCAux,
    CreateWindowAux, StackMode, VisualClass, Window, WindowClass,
};

/// Accent colour of the preview (red, green, blue).
pub const ACCENT: [u8; 3] = [0x35, 0x84, 0xe4];

/// Opacity of the fill at full visibility.
pub const FILL_ALPHA: f32 = 0.22;

/// Opacity of the border at full visibility.
pub const LINE_ALPHA: f32 = 0.85;

/// Border width in pixels.
pub const BORDER: i32 = 3;

/// Radius in pixels of the rounded corners of the preview.
pub const CORNER_RADIUS: i32 = 10;

/// Duration in milliseconds of the slide from one zone to another.
const RECT_MS: u64 = 120;

/// Duration in milliseconds of the fade in and out.
const FADE_MS: u64 = 100;

/// Slows down towards the end: `t` in `0..=1` maps to `0..=1`.
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// `a` to `b` at `t`, rounded.
fn lerp(a: i32, b: i32, t: f32) -> i32 {
    (a as f32 + (b - a) as f32 * t).round() as i32
}

/// The rectangle `0, 0, w, h` with its four corners rounded by `radius`, as a
/// list of rectangles (one per run of rows that share a corner inset) that
/// together cover it. The radius is limited to half the shorter side. Used as
/// a Shape region, which has hard edges: it cannot be antialiased.
pub fn rounded_spans(w: i32, h: i32, radius: i32) -> Vec<Rect> {
    if w <= 0 || h <= 0 { return Vec::new(); }
    let r = radius.min(w / 2).min(h / 2).max(0);
    // How far row `y` (counted from the top of a corner) is inset from the side.
    let inset = |y: i32| {
        let dy = r as f32 - (y as f32 + 0.5);
        (r as f32 - ((r * r) as f32 - dy * dy).max(0.0).sqrt()).round() as i32
    };
    let mut spans: Vec<Rect> = Vec::new();
    let mut push = |y: i32, rows: i32, inset: i32| match spans.last_mut() {
        Some(last) if last.x == inset && last.y + last.h == y => last.h += rows,
        _ => spans.push(Rect::new(inset, y, w - 2 * inset, rows)),
    };
    for y in 0..r {
        push(y, 1, inset(y));
    }
    push(r, h - 2 * r, 0);
    for y in 0..r {
        push(h - r + y, 1, inset(r - 1 - y));
    }
    spans
}

/// A pixel for a 32-bit ARGB window: `color` at `alpha`, premultiplied.
pub fn argb(color: [u8; 3], alpha: f32) -> u32 {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
    let premultiplied = |c: u8| (u32::from(c) * a + 127) / 255;
    (a << 24) | (premultiplied(color[0]) << 16) | (premultiplied(color[1]) << 8) | premultiplied(color[2])
}

/// Where the preview is and how visible, over time.
#[derive(Debug, Clone, Copy)]
pub struct Transition {
    from: Rect,
    to: Rect,
    alpha_from: f32,
    alpha_to: f32,
    start_ms: u64,
}

impl Transition {
    /// A preview that is not shown.
    pub fn hidden() -> Self {
        Self { from: Rect::default(), to: Rect::default(), alpha_from: 0.0, alpha_to: 0.0, start_ms: 0 }
    }

    /// Rectangle and opacity (`0..=1`) at `now_ms`.
    pub fn current(&self, now_ms: u64) -> (Rect, f32) {
        let elapsed = now_ms.saturating_sub(self.start_ms) as f32;
        let slide = ease_out(elapsed / RECT_MS as f32);
        let fade = ease_out(elapsed / FADE_MS as f32);
        let rect = Rect::new(
            lerp(self.from.x, self.to.x, slide),
            lerp(self.from.y, self.to.y, slide),
            lerp(self.from.w, self.to.w, slide),
            lerp(self.from.h, self.to.h, slide),
        );
        (rect, self.alpha_from + (self.alpha_to - self.alpha_from) * fade)
    }

    /// Show the preview on `rect`: it fades in where it is, or slides there if
    /// it is already visible. Does nothing when it is already heading there.
    pub fn show(&mut self, rect: Rect, now_ms: u64) {
        if self.alpha_to >= 1.0 && self.to == rect { return; }
        let (current, alpha) = self.current(now_ms);
        self.from = if alpha <= 0.0 { rect } else { current };
        self.to = rect;
        self.alpha_from = alpha;
        self.alpha_to = 1.0;
        self.start_ms = now_ms;
    }

    /// Fade the preview out where it is.
    pub fn hide(&mut self, now_ms: u64) {
        if self.alpha_to <= 0.0 { return; }
        let (current, alpha) = self.current(now_ms);
        self.from = current;
        self.to = current;
        self.alpha_from = alpha;
        self.alpha_to = 0.0;
        self.start_ms = now_ms;
    }

    /// Whether something still changes after `now_ms`.
    pub fn is_animating(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.start_ms) < RECT_MS.max(FADE_MS)
    }
}

/// The preview window.
pub struct Overlay {
    window: Window,
    gc: xproto::Gcontext,
    transition: Transition,
    mapped: bool,
    drawn: Option<(Rect, u32)>,
    /// The size the rounded Shape region was made for.
    shaped: Option<(i32, i32)>,
}

impl Overlay {
    /// Create the (unmapped) preview window. `None` when the screen has no
    /// 32-bit ARGB visual or no compositing manager runs: without one the
    /// translucent window would be opaque.
    pub fn new(display: &Display) -> Result<Option<Overlay>, Box<dyn Error>> {
        Self::create(display, true)
    }

    /// Like [`Overlay::new`], but with `require_compositor` false the preview
    /// is also made when no compositing manager runs (it then draws opaque).
    /// For tests, which have no compositor to run.
    pub fn create(display: &Display, require_compositor: bool) -> Result<Option<Overlay>, Box<dyn Error>> {
        let conn = &display.conn;
        let screen = &conn.setup().roots[display.screen_num];
        let Some((depth, visual)) = screen.allowed_depths.iter().filter(|d| d.depth == 32).find_map(|d| {
            d.visuals.iter().find(|v| v.class == VisualClass::TRUE_COLOR).map(|v| (d.depth, v.visual_id))
        }) else {
            return Ok(None);
        };
        if require_compositor && !display.compositor_running()? { return Ok(None); }

        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, display.root, visual)?;
        let window = conn.generate_id()?;
        conn.create_window(
            depth,
            window,
            display.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new().background_pixel(0).border_pixel(0).override_redirect(1).colormap(colormap),
        )?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new())?;
        // An empty input region: the preview never catches the pointer.
        if let Err(e) = shape::rectangles(conn, SO::SET, SK::INPUT, xproto::ClipOrdering::UNSORTED, window, 0, 0, &[]) {
            opusdm_ipc::err_log!("opusdm-snap", "cannot make the overlay click-through: {e}");
        }
        conn.flush()?;
        Ok(Some(Overlay { window, gc, transition: Transition::hidden(), mapped: false, drawn: None, shaped: None }))
    }

    /// The preview window, for tests that look at what was drawn.
    pub fn window(&self) -> Window {
        self.window
    }

    /// Show the preview on `rect`, in root coordinates.
    pub fn show(&mut self, rect: Rect, now_ms: u64) {
        self.transition.show(rect, now_ms);
    }

    /// Fade the preview out.
    pub fn hide(&mut self, now_ms: u64) {
        self.transition.hide(now_ms);
    }

    /// Draw the state at `now_ms`. Returns whether it still animates, so the
    /// caller keeps ticking.
    pub fn tick(&mut self, display: &Display, now_ms: u64) -> Result<bool, Box<dyn Error>> {
        let conn = &display.conn;
        let (rect, alpha) = self.transition.current(now_ms);
        if alpha <= 0.0 {
            if self.mapped {
                conn.unmap_window(self.window)?;
                conn.flush()?;
                self.mapped = false;
                self.drawn = None;
            }
            return Ok(false);
        }
        let fill = argb(ACCENT, alpha * FILL_ALPHA);
        if self.drawn != Some((rect, fill)) {
            self.draw(display, rect, alpha, fill)?;
            self.drawn = Some((rect, fill));
        }
        Ok(self.transition.is_animating(now_ms))
    }

    /// Paint the preview at `rect`: the border colour over the whole window,
    /// the translucent fill over the inside, both with rounded corners.
    fn draw(&mut self, display: &Display, rect: Rect, alpha: f32, fill: u32) -> Result<(), Box<dyn Error>> {
        let conn = &display.conn;
        let (w, h) = (rect.w.max(1), rect.h.max(1));
        conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().x(rect.x).y(rect.y).width(w as u32).height(h as u32).stack_mode(StackMode::ABOVE),
        )?;
        if self.shaped != Some((w, h)) {
            self.round_corners(conn, w, h);
            self.shaped = Some((w, h));
        }
        conn.change_window_attributes(self.window, &ChangeWindowAttributesAux::new().background_pixel(argb(ACCENT, alpha * LINE_ALPHA)))?;
        if !self.mapped {
            conn.map_window(self.window)?;
            self.mapped = true;
        }
        conn.clear_area(false, self.window, 0, 0, 0, 0)?;
        conn.change_gc(self.gc, &ChangeGCAux::new().foreground(fill))?;
        let b = BORDER.min(w / 2).min(h / 2).max(1);
        let inner: Vec<xproto::Rectangle> = rounded_spans(w - 2 * b, h - 2 * b, (CORNER_RADIUS - b).max(0))
            .iter()
            .map(|r| xproto::Rectangle { x: (r.x + b) as i16, y: (r.y + b) as i16, width: r.w as u16, height: r.h as u16 })
            .collect();
        conn.poly_fill_rectangle(self.window, self.gc, &inner)?;
        conn.flush()?;
        Ok(())
    }

    /// Clip the window to a rounded rectangle of size `w` x `h` (the Shape
    /// bounding region). A failure is logged and the corners stay square.
    fn round_corners(&self, conn: &impl Connection, w: i32, h: i32) {
        let rects: Vec<xproto::Rectangle> = rounded_spans(w, h, CORNER_RADIUS)
            .iter()
            .map(|r| xproto::Rectangle { x: r.x as i16, y: r.y as i16, width: r.w as u16, height: r.h as u16 })
            .collect();
        if let Err(e) = shape::rectangles(conn, SO::SET, SK::BOUNDING, xproto::ClipOrdering::YX_BANDED, self.window, 0, 0, &rects) {
            opusdm_ipc::err_log!("opusdm-snap", "cannot round the corners of the overlay: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: Rect = Rect { x: 0, y: 30, w: 960, h: 1050 };
    const RIGHT: Rect = Rect { x: 960, y: 30, w: 960, h: 1050 };

    /// Whether the pixel `(x, y)` is inside the union of `spans`.
    fn covers(spans: &[Rect], x: i32, y: i32) -> bool {
        spans.iter().any(|s| s.contains((x, y)))
    }

    #[test]
    fn the_corners_are_cut_and_the_middle_is_kept() {
        let spans = rounded_spans(100, 60, 10);
        for corner in [(0, 0), (99, 0), (0, 59), (99, 59)] {
            assert!(!covers(&spans, corner.0, corner.1), "corner {corner:?} must be cut");
        }
        for inside in [(10, 0), (89, 0), (0, 10), (99, 49), (50, 30), (10, 59), (89, 59)] {
            assert!(covers(&spans, inside.0, inside.1), "{inside:?} must be kept");
        }
    }

    #[test]
    fn the_spans_do_not_overlap_and_the_area_matches_a_rounded_rectangle() {
        let (w, h, r) = (200, 120, 10);
        let spans = rounded_spans(w, h, r);
        let area: i32 = spans.iter().map(|s| s.w * s.h).sum();
        let mut covered = std::collections::HashSet::new();
        for s in &spans {
            for y in s.y..s.y + s.h {
                for x in s.x..s.x + s.w {
                    assert!(covered.insert((x, y)), "({x}, {y}) is covered twice");
                }
            }
        }
        // A quarter of a circle cut off each corner: r^2 * (1 - pi/4) pixels, four times.
        let cut = 4.0 * (r * r) as f32 * (1.0 - std::f32::consts::FRAC_PI_4);
        assert!((((w * h) as f32 - cut) - area as f32).abs() < 12.0, "area {area}");
    }

    #[test]
    fn the_shape_is_symmetric() {
        let spans = rounded_spans(80, 50, 10);
        for y in 0..50 {
            for x in 0..80 {
                assert_eq!(covers(&spans, x, y), covers(&spans, 79 - x, y), "left/right at ({x}, {y})");
                assert_eq!(covers(&spans, x, y), covers(&spans, x, 49 - y), "top/bottom at ({x}, {y})");
            }
        }
    }

    #[test]
    fn the_radius_is_limited_to_half_the_shorter_side() {
        let spans = rounded_spans(40, 8, 10);
        assert!(covers(&spans, 20, 4), "the middle is kept");
        assert!(!covers(&spans, 0, 0));
        assert!(spans.iter().all(|s| s.y >= 0 && s.y + s.h <= 8 && s.x >= 0 && s.x + s.w <= 40), "inside the window: {spans:?}");
    }

    #[test]
    fn no_radius_or_an_empty_size_is_plain() {
        assert_eq!(rounded_spans(30, 20, 0), vec![Rect::new(0, 0, 30, 20)]);
        assert!(rounded_spans(0, 20, 10).is_empty());
        assert!(rounded_spans(30, -1, 10).is_empty());
    }

    #[test]
    fn argb_is_premultiplied() {
        assert_eq!(argb([255, 255, 255], 1.0), 0xffff_ffff);
        assert_eq!(argb([255, 255, 255], 0.0), 0);
        assert_eq!(argb([0x80, 0x00, 0xff], 0.5), 0x8040_0080);
    }

    #[test]
    fn a_hidden_preview_is_invisible() {
        let t = Transition::hidden();
        assert_eq!(t.current(1_000).1, 0.0);
        assert!(!t.is_animating(1_000));
    }

    #[test]
    fn showing_fades_in_where_it_appears() {
        let mut t = Transition::hidden();
        t.show(LEFT, 1_000);
        let (rect, alpha) = t.current(1_000);
        assert_eq!((rect, alpha), (LEFT, 0.0));
        let (rect, alpha) = t.current(1_000 + FADE_MS / 2);
        assert_eq!(rect, LEFT, "no slide when it appears");
        assert!(alpha > 0.0 && alpha < 1.0);
        assert_eq!(t.current(1_000 + FADE_MS), (LEFT, 1.0));
        assert!(t.is_animating(1_000 + 10));
        assert!(!t.is_animating(1_000 + RECT_MS + 1));
    }

    #[test]
    fn moving_to_another_zone_slides_at_full_opacity() {
        let mut t = Transition::hidden();
        t.show(LEFT, 0);
        t.show(RIGHT, 1_000);
        let (rect, alpha) = t.current(1_000 + RECT_MS / 2);
        assert_eq!(alpha, 1.0);
        assert!(rect.x > LEFT.x && rect.x < RIGHT.x, "halfway: {rect:?}");
        assert_eq!(t.current(1_000 + RECT_MS), (RIGHT, 1.0));
    }

    #[test]
    fn showing_the_same_zone_again_does_not_restart() {
        let mut t = Transition::hidden();
        t.show(LEFT, 0);
        t.show(LEFT, 50);
        assert_eq!(t.current(FADE_MS).1, 1.0);
    }

    #[test]
    fn hiding_fades_out_in_place_and_can_be_shown_again() {
        let mut t = Transition::hidden();
        t.show(LEFT, 0);
        t.hide(1_000);
        assert_eq!(t.current(1_000).1, 1.0);
        let (rect, alpha) = t.current(1_000 + FADE_MS / 2);
        assert_eq!(rect, LEFT);
        assert!(alpha > 0.0 && alpha < 1.0);
        assert_eq!(t.current(1_000 + FADE_MS).1, 0.0);
        t.show(RIGHT, 2_000);
        assert_eq!(t.current(2_000).0, RIGHT, "a faded-out preview reappears in place");
    }
}
