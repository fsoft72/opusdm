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
const ACCENT: [u8; 3] = [0x35, 0x84, 0xe4];

/// Opacity of the fill at full visibility.
const FILL_ALPHA: f32 = 0.22;

/// Opacity of the border at full visibility.
const LINE_ALPHA: f32 = 0.85;

/// Border width in pixels.
const BORDER: i32 = 3;

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
}

impl Overlay {
    /// Create the (unmapped) preview window. `None` when the screen has no
    /// 32-bit ARGB visual or no compositing manager runs: without one the
    /// translucent window would be opaque.
    pub fn new(display: &Display) -> Result<Option<Overlay>, Box<dyn Error>> {
        let conn = &display.conn;
        let screen = &conn.setup().roots[display.screen_num];
        let Some((depth, visual)) = screen.allowed_depths.iter().filter(|d| d.depth == 32).find_map(|d| {
            d.visuals.iter().find(|v| v.class == VisualClass::TRUE_COLOR).map(|v| (d.depth, v.visual_id))
        }) else {
            return Ok(None);
        };
        if !display.compositor_running()? { return Ok(None); }

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
        Ok(Some(Overlay { window, gc, transition: Transition::hidden(), mapped: false, drawn: None }))
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

    /// Paint the fill (the window background) and the border at `rect`.
    fn draw(&mut self, display: &Display, rect: Rect, alpha: f32, fill: u32) -> Result<(), Box<dyn Error>> {
        let conn = &display.conn;
        let (w, h) = (rect.w.max(1), rect.h.max(1));
        conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().x(rect.x).y(rect.y).width(w as u32).height(h as u32).stack_mode(StackMode::ABOVE),
        )?;
        conn.change_window_attributes(self.window, &ChangeWindowAttributesAux::new().background_pixel(fill))?;
        if !self.mapped {
            conn.map_window(self.window)?;
            self.mapped = true;
        }
        conn.clear_area(false, self.window, 0, 0, 0, 0)?;
        conn.change_gc(self.gc, &ChangeGCAux::new().foreground(argb(ACCENT, alpha * LINE_ALPHA)))?;
        let b = BORDER.min(w / 2).min(h / 2).max(1);
        let side = |x: i32, y: i32, rw: i32, rh: i32| xproto::Rectangle { x: x as i16, y: y as i16, width: rw.max(1) as u16, height: rh.max(1) as u16 };
        conn.poly_fill_rectangle(
            self.window,
            self.gc,
            &[side(0, 0, w, b), side(0, h - b, w, b), side(0, b, b, h - 2 * b), side(w - b, b, b, h - 2 * b)],
        )?;
        conn.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: Rect = Rect { x: 0, y: 30, w: 960, h: 1050 };
    const RIGHT: Rect = Rect { x: 960, y: 30, w: 960, h: 1050 };

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
