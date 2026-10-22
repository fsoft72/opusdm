//! Sharp detail of a zoomed PDF page. The page on screen is rendered once at
//! `PAGE_SCALE`; zoomed in past 100% that picture is only stretched, so the
//! letters turn blurry. This worker renders just a window of the page (the part
//! on screen and a margin) at the pixels of the zoom, to be drawn over the
//! stretched page. It keeps the document open, like the page worker, and a
//! newer request makes the older ones not worth rendering.

use crate::loader::RawImage;
use crate::pdf::open;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{render_into, RenderCache, RenderSettings};
use std::path::PathBuf;
use std::sync::mpsc;

/// Longest side, in pixels, of a window (hayro sizes are u16).
pub const MAX_TILE_SIDE: u32 = 4096;

/// A window of a page the main thread wants: tagged with the generation it was asked for.
pub struct DetailRequest {
    pub generation: u64,
    pub path: PathBuf,
    /// Page index, 0-based.
    pub page: usize,
    /// Zoom of the picture on screen (screen pixels per pixel of the page picture).
    pub zoom: f64,
    pub rotation: u16,
    /// Top-left corner of the window, in pixels of the unturned page at this zoom.
    pub origin: (i32, i32),
    /// Size of the window in pixels.
    pub size: (u32, u32),
    /// Pixels per PDF point to render at.
    pub scale: f32,
}

/// A rendered window and what it was rendered for.
#[derive(Debug)]
pub struct DetailTile {
    pub page: usize,
    pub zoom: f64,
    pub rotation: u16,
    pub origin: (i32, i32),
    pub scale: f32,
    pub raw: RawImage,
}

/// Answer of the detail worker: the generation of the request and the window.
pub type DetailAnswer = (u64, Result<DetailTile, String>);

/// Start the detail worker; it ends when the sender is dropped.
pub fn start_worker(answers: async_channel::Sender<DetailAnswer>) -> mpsc::Sender<DetailRequest> {
    let (tx, rx) = mpsc::channel::<DetailRequest>();
    std::thread::spawn(move || {
        let mut pending = rx.recv().ok();
        while let Some(request) = pending.take() {
            pending = _serve_document(request, &rx, &answers);
        }
    });
    tx
}

/// The newest request waiting after `request`.
fn _newest(mut request: DetailRequest, rx: &mpsc::Receiver<DetailRequest>) -> DetailRequest {
    while let Ok(newer) = rx.try_recv() { request = newer; }
    request
}

/// Open the document of `request` and answer every request for it. Returns the
/// first request for another document, or `None` when the channels close.
fn _serve_document(request: DetailRequest, rx: &mpsc::Receiver<DetailRequest>, answers: &async_channel::Sender<DetailAnswer>) -> Option<DetailRequest> {
    let mut request = _newest(request, rx);
    let pdf = match open(&request.path) {
        Ok(pdf) => pdf,
        Err(e) => {
            answers.send_blocking((request.generation, Err(e))).ok()?;
            return rx.recv().ok();
        }
    };
    let cache = RenderCache::new();
    loop {
        let result = render_tile(&pdf, &request, &cache);
        answers.send_blocking((request.generation, result)).ok()?;
        let next = _newest(rx.recv().ok()?, rx);
        if next.path != request.path { return Some(next); }
        request = next;
    }
}

/// Render the window of `request`.
pub fn render_tile<'a>(pdf: &'a Pdf, request: &DetailRequest, cache: &RenderCache<'a>) -> Result<DetailTile, String> {
    let (w, h) = request.size;
    if w == 0 || h == 0 || w > MAX_TILE_SIDE || h > MAX_TILE_SIDE { return Err(format!("detail window of {w} x {h} pixels")); }
    let pages = pdf.pages();
    let Some(page) = pages.get(request.page) else { return Err(format!("page {} does not exist", request.page + 1)) };
    let transform = Affine::translate((-request.origin.0 as f64, -request.origin.1 as f64)) * Affine::scale(request.scale as f64) * page.initial_transform(true).to_kurbo();
    let mut ctx = RenderContext::new(w as u16, h as u16);
    render_into(page, cache, &InterpreterSettings::default(), &RenderSettings::default(), &mut ctx, transform);
    ctx.flush();
    let mut pixmap = Pixmap::new(ctx.width(), ctx.height());
    ctx.render_with(&mut pixmap, &mut Resources::default(), RasterizerSettings { target_init: TargetInit::Clear(WHITE), ..Default::default() });
    let raw = RawImage { width: w, height: h, rowstride: (w * 4) as i32, has_alpha: true, bytes: pixmap.take_rgba8(ImageAlphaType::Alpha) };
    Ok(DetailTile { page: request.page, zoom: request.zoom, rotation: request.rotation, origin: request.origin, scale: request.scale, raw })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::tests::pdf_with_contents;

    /// A request for the window at `origin` of page 0 at 2 pixels per point.
    fn request(origin: (i32, i32), size: (u32, u32)) -> DetailRequest {
        DetailRequest { generation: 1, path: PathBuf::new(), page: 0, zoom: 1.0, rotation: 0, origin, size, scale: 2.0 }
    }

    #[test]
    fn a_window_shows_the_part_of_the_page_it_was_asked_for() {
        // A black square from (0, 0) to (50, 50) points, bottom-left origin: it
        // is the lower left corner of a 100 x 100 points page.
        let pdf = Pdf::new(pdf_with_contents(&["0 0 0 rg 0 0 50 50 re f"])).unwrap();
        let cache = RenderCache::new();
        let page_size = pdf.pages().get(0).unwrap().render_dimensions();
        let height = (page_size.1 * 2.0) as i32;
        // 20 x 20 pixels inside the square, then the same size above it.
        let inside = render_tile(&pdf, &request((10, height - 30), (20, 20)), &cache).unwrap();
        let outside = render_tile(&pdf, &request((10, 10), (20, 20)), &cache).unwrap();
        assert_eq!((inside.raw.width, inside.raw.height), (20, 20));
        assert_eq!(&inside.raw.bytes[..3], &[0, 0, 0], "the square is black");
        assert_eq!(&outside.raw.bytes[..3], &[255, 255, 255], "the rest of the page is white");
    }

    #[test]
    fn a_window_too_big_or_empty_is_an_error() {
        let pdf = Pdf::new(pdf_with_contents(&[""])).unwrap();
        let cache = RenderCache::new();
        assert!(render_tile(&pdf, &request((0, 0), (0, 10)), &cache).is_err());
        assert!(render_tile(&pdf, &request((0, 0), (MAX_TILE_SIDE + 1, 10)), &cache).is_err());
    }
}
