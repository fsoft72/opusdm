//! Shared cairo helpers of the images the compositor draws itself (the
//! window menu and the window switcher): colours, gradients, the cairo
//! surface to `MemoryRenderBuffer` copy, and a one-entry cache keyed by the
//! state an image was drawn for.
//!
//! Ownership: a `RenderCache` belongs to the open menu or switcher it draws
//! (`WindowMenu::images`, `WindowSwitcher::images`) and is dropped with it.

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Buffer, Rectangle, Transform},
};

use crate::menu::theme::Rgb;

/// Buffer pixels per logical pixel of every image the compositor draws and
/// of their placement on the output: the nested output is drawn at scale 1
/// (its `OutputDamageTracker` and the element locations use this value).
pub const DRAW_SCALE: i32 = 1;

/// The last image drawn and the state (`K`, the scale included) it was
/// drawn for. A failed draw is cached too: the same state is not drawn
/// again every frame, a new state is.
#[derive(Debug, Clone)]
pub struct RenderCache<K, V> {
    /// The state and the result of the last draw.
    entry: Option<(K, Result<V, String>)>,
    /// How many times an image was drawn (read by the tests).
    builds: u32,
}

impl<K, V> Default for RenderCache<K, V> {
    /// An empty cache.
    fn default() -> Self {
        RenderCache { entry: None, builds: 0 }
    }
}

impl<K: PartialEq, V: Clone> RenderCache<K, V> {
    /// The cached result while `key` is the state of the last draw, else
    /// the result of `draw` (stored for `key`, an error included).
    pub fn get_or_draw(&mut self, key: K, draw: impl FnOnce() -> Result<V, String>) -> Result<V, String> {
        if let Some((cached, result)) = &self.entry {
            if *cached == key { return result.clone(); }
        }
        let result = draw();
        self.builds = self.builds.saturating_add(1);
        self.entry = Some((key, result.clone()));
        result
    }

    /// How many times an image was drawn (a failed draw included).
    #[cfg(test)]
    pub fn builds(&self) -> u32 {
        self.builds
    }
}

/// Draws an image of `size` logical pixels at `scale` (buffer pixels per
/// logical pixel) with `paint`, which works in logical pixels, into an
/// opaque ARGB8888 buffer. `what` names the image in the errors.
pub fn paint_buffer(
    what: &str,
    size: (i32, i32),
    scale: i32,
    paint: impl FnOnce(&cairo::Context) -> Result<(), cairo::Error>,
) -> Result<MemoryRenderBuffer, String> {
    if scale < 1 { return Err(format!("{what}: bad scale {scale}")); }
    let (w, h) = size;
    if w <= 0 || h <= 0 { return Err(format!("{what}: empty image {w}x{h}")); }
    let (pw, ph) = (w * scale, h * scale);
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, pw, ph)
        .map_err(|e| format!("{what}: cairo surface {pw}x{ph}: {e}"))?;
    {
        let cr = cairo::Context::new(&surface).map_err(|e| format!("{what}: cairo context: {e}"))?;
        cr.scale(f64::from(scale), f64::from(scale));
        paint(&cr).map_err(|e| format!("{what}: cairo drawing: {e}"))?;
    }
    surface.flush();
    let stride = surface.stride();
    if stride != pw * 4 { return Err(format!("{what}: cairo stride {stride} for width {pw}")); }
    let data = surface.data().map_err(|e| format!("{what}: cairo surface data: {e}"))?;
    let opaque: Vec<Rectangle<i32, Buffer>> = vec![Rectangle::from_size((pw, ph).into())];
    Ok(MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (pw, ph), scale, Transform::Normal, Some(opaque)))
}

/// Fills `(x, y, w, h)` with the vertical gradient `colours.0` (top) to
/// `colours.1` (bottom).
pub fn fill_gradient(cr: &cairo::Context, colours: (Rgb, Rgb), x: f64, y: f64, w: f64, h: f64) -> Result<(), cairo::Error> {
    let gradient = cairo::LinearGradient::new(0.0, y, 0.0, y + h);
    for (offset, rgb) in [(0.0, colours.0), (1.0, colours.1)] {
        let (r, g, b) = components(rgb);
        gradient.add_color_stop_rgb(offset, r, g, b);
    }
    cr.set_source(&gradient)?;
    cr.rectangle(x, y, w, h);
    cr.fill()
}

/// Sets the source of `cr` to the solid colour `rgb`.
pub fn set_rgb(cr: &cairo::Context, rgb: Rgb) {
    let (r, g, b) = components(rgb);
    cr.set_source_rgb(r, g, b);
}

/// The components of `rgb` in `0.0..=1.0`.
pub fn components(rgb: Rgb) -> (f64, f64, f64) {
    (f64::from(rgb.0) / 255.0, f64::from(rgb.1) / 255.0, f64::from(rgb.2) / 255.0)
}

/// The bytes of `buffer` (for the tests of the images).
#[cfg(test)]
pub fn buffer_bytes(buffer: &mut MemoryRenderBuffer) -> Vec<u8> {
    let mut out = Vec::new();
    buffer
        .render()
        .draw(|mem| -> Result<_, ()> {
            out = mem.to_vec();
            Ok(Vec::new())
        })
        .unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_state_is_drawn_once_and_a_new_state_again() {
        let mut cache: RenderCache<u32, u32> = RenderCache::default();
        assert_eq!(cache.get_or_draw(1, || Ok(10)), Ok(10));
        assert_eq!(cache.get_or_draw(1, || Ok(99)), Ok(10), "cached");
        assert_eq!(cache.builds(), 1);
        assert_eq!(cache.get_or_draw(2, || Ok(20)), Ok(20));
        assert_eq!(cache.builds(), 2);
    }

    #[test]
    fn a_failed_draw_is_not_retried_for_the_same_state() {
        let mut cache: RenderCache<u32, u32> = RenderCache::default();
        assert!(cache.get_or_draw(1, || Err("boom".into())).is_err());
        assert_eq!(cache.get_or_draw(1, || Ok(10)), Err("boom".to_string()), "the failure is cached");
        assert_eq!(cache.builds(), 1);
        assert_eq!(cache.get_or_draw(2, || Ok(20)), Ok(20), "a new state is drawn");
    }

    #[test]
    fn paint_buffer_refuses_a_bad_scale_and_an_empty_size() {
        assert!(paint_buffer("t", (10, 10), 0, |_| Ok(())).is_err());
        assert!(paint_buffer("t", (0, 10), 1, |_| Ok(())).is_err());
        let mut buffer = paint_buffer("t", (10, 5), 2, |_| Ok(())).expect("drawn");
        assert_eq!(buffer_bytes(&mut buffer).len(), 20 * 10 * 4);
    }
}
