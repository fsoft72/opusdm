//! PDF pages rendered to pictures with hayro (pure Rust, no system library).
//! A worker thread keeps the open document, so turning a page does not parse
//! the file again; it answers with plain bytes (`RawImage`), like the picture loader.

use crate::comic;
use crate::kind::{kind_of, MediaKind};
use crate::loader::RawImage;
use crate::pdf_links::{page_links, Link};
use crate::pdf_text::{extract, PageText};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// Pixels per PDF point for the page on screen (2.0 = 144 dpi): sharp in the
/// fit mode on most screens, and zooming in still has detail.
pub const PAGE_SCALE: f32 = 2.0;
/// Longest edge, in pixels, a rendered page may have (hayro sizes are u16 and
/// a poster-sized page would take gigabytes).
const MAX_EDGE: f32 = 8192.0;

/// One rendered page, the number of pages of its document, the pixels per
/// point it was rendered at, its links and its text (only the page worker
/// fills it; empty for a thumbnail or a comic).
#[derive(Debug)]
pub struct RenderedPage {
    pub raw: RawImage,
    pub count: usize,
    pub scale: f32,
    pub links: Vec<Link>,
    pub text: PageText,
}

/// A page the window wants: tagged with the generation it was asked for.
pub struct PageRequest {
    pub generation: u64,
    pub path: PathBuf,
    pub index: usize,
}

/// Answer of the page worker: the generation of the request and the page.
pub type PageAnswer = (u64, Result<RenderedPage, String>);

/// Open and parse a PDF file.
pub fn open(path: &Path) -> Result<Pdf, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    Pdf::new(data).map_err(|e| match e {
        LoadPdfError::Decryption(_) => "password protected PDF files are not supported".to_string(),
        LoadPdfError::Invalid => "not a valid PDF file".to_string(),
    })
}

/// Render page `index` (0-based) of `pdf` at `scale` pixels per point.
pub fn render_page(pdf: &Pdf, index: usize, scale: f32) -> Result<RenderedPage, String> {
    render_page_cached(pdf, index, scale, &RenderCache::new())
}

/// [`render_page`] reusing `cache`, which keeps the fonts and outlines decoded
/// for the earlier pages of the same document.
pub fn render_page_cached<'a>(pdf: &'a Pdf, index: usize, scale: f32, cache: &RenderCache<'a>) -> Result<RenderedPage, String> {
    let pages = pdf.pages();
    let count = pages.len();
    if count == 0 { return Err("the document has no pages".to_string()); }
    let Some(page) = pages.get(index) else { return Err(format!("page {} does not exist ({count} pages)", index + 1)) };
    let (width, height) = page.render_dimensions();
    let scale = _clamped_scale(width, height, scale);
    let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
    let pixmap = render(page, cache, &InterpreterSettings::default(), &RenderSettings::default(), &settings);
    let (w, h) = (pixmap.width() as u32, pixmap.height() as u32);
    if w == 0 || h == 0 { return Err(format!("page {} is empty", index + 1)); }
    let raw = RawImage { width: w, height: h, rowstride: (w * 4) as i32, has_alpha: true, bytes: pixmap.take_rgba8(ImageAlphaType::Alpha) };
    Ok(RenderedPage { raw, count, scale, links: page_links(pdf, index), text: PageText::default() })
}

/// The first page scaled to fit a `px` x `px` box, for the thumbnail strip.
pub fn render_thumb(path: &Path, px: i32) -> Result<RawImage, String> {
    let pdf = open(path)?;
    let first = pdf.pages().first().ok_or("the document has no pages")?;
    let (width, height) = first.render_dimensions();
    let scale = px as f32 / width.max(height).max(1.0);
    render_page(&pdf, 0, scale).map(|page| page.raw)
}

/// `scale` lowered so the longest edge stays within `MAX_EDGE` pixels.
fn _clamped_scale(width: f32, height: f32, scale: f32) -> f32 {
    let edge = width.max(height).max(1.0);
    scale.min(MAX_EDGE / edge)
}

/// Start the page worker. It keeps the last opened document and its render
/// cache, skips requests that a newer one already replaced, and stops when
/// the sender is dropped.
pub fn start_worker(answers: async_channel::Sender<PageAnswer>) -> mpsc::Sender<PageRequest> {
    let (tx, rx) = mpsc::channel::<PageRequest>();
    std::thread::spawn(move || {
        let mut next = rx.recv().ok();
        while let Some(request) = next.take() {
            next = _serve_document(request, &rx, &answers);
        }
    });
    tx
}

/// The newest request waiting after `request`: only it matters, the older
/// pages are no longer on screen.
fn _newest(mut request: PageRequest, rx: &mpsc::Receiver<PageRequest>) -> PageRequest {
    while let Ok(newer) = rx.try_recv() { request = newer; }
    request
}

/// Open the document of `request` and answer every request for it, sharing
/// one render cache. Returns the first request for another document, or
/// `None` when the channels close.
fn _serve_document(request: PageRequest, rx: &mpsc::Receiver<PageRequest>, answers: &async_channel::Sender<PageAnswer>) -> Option<PageRequest> {
    let mut request = _newest(request, rx);
    if kind_of(&request.path) == Some(MediaKind::Comic) { return _serve_comic(request, rx, answers); }
    let pdf = match open(&request.path) {
        Ok(pdf) => pdf,
        Err(e) => {
            answers.send_blocking((request.generation, Err(e))).ok()?;
            return rx.recv().ok();
        }
    };
    let cache = RenderCache::new();
    loop {
        let mut result = render_page_cached(&pdf, request.index, PAGE_SCALE, &cache);
        if let Ok(page) = result.as_mut() { page.text = extract(&pdf, request.index); }
        answers.send_blocking((request.generation, result)).ok()?;
        let next = _newest(rx.recv().ok()?, rx);
        if next.path != request.path { return Some(next); }
        request = next;
    }
}

/// [`_serve_document`] for a comic book: its pages are read once and kept.
fn _serve_comic(mut request: PageRequest, rx: &mpsc::Receiver<PageRequest>, answers: &async_channel::Sender<PageAnswer>) -> Option<PageRequest> {
    let comic = match comic::open(&request.path) {
        Ok(comic) => comic,
        Err(e) => {
            answers.send_blocking((request.generation, Err(e))).ok()?;
            return rx.recv().ok();
        }
    };
    loop {
        answers.send_blocking((request.generation, comic::render_page(&comic, request.index))).ok()?;
        let next = _newest(rx.recv().ok()?, rx);
        if next.path != request.path { return Some(next); }
        request = next;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Object number of page `index` (0-based) in the PDFs of [`build_pdf`].
    pub(crate) fn page_obj(index: usize) -> usize {
        4 + index * 2
    }

    /// A PDF with one 200 x 100 point page per content stream. Object 3 is the
    /// font `/F1` (Helvetica), page `i` is object [`page_obj`]`(i)`; `annots[i]`
    /// is put as is in the `/Annots` array of page `i` and `catalog_extra` in the catalog.
    pub(crate) fn build_pdf(contents: &[&str], annots: &[&str], catalog_extra: &str) -> Vec<u8> {
        let kids: Vec<String> = (0..contents.len()).map(|i| format!("{} 0 R", page_obj(i))).collect();
        let mut objects = vec![
            format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
            format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), contents.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        for (i, content) in contents.iter().enumerate() {
            let annots = annots.get(i).map(|a| format!("/Annots [{a}]")).unwrap_or_default();
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R {annots} >>", page_obj(i) + 1));
            objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{object}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }

    /// A PDF with one page per content stream and nothing else.
    pub(crate) fn pdf_with_contents(contents: &[&str]) -> Vec<u8> {
        build_pdf(contents, &[], "")
    }

    /// A PDF of `pages` pages, each with a 10 x 10 black square in the bottom-left corner.
    fn make_pdf(pages: usize) -> Vec<u8> {
        pdf_with_contents(&vec!["0 0 0 rg 0 0 10 10 re f"; pages])
    }

    /// Write a test PDF into `dir` and return its path.
    fn write_pdf(dir: &Path, pages: usize) -> PathBuf {
        let path = dir.join("doc.pdf");
        std::fs::write(&path, make_pdf(pages)).unwrap();
        path
    }

    /// RGBA of one pixel.
    fn pixel(raw: &RawImage, x: u32, y: u32) -> [u8; 4] {
        let at = (y as i32 * raw.rowstride + x as i32 * 4) as usize;
        [raw.bytes[at], raw.bytes[at + 1], raw.bytes[at + 2], raw.bytes[at + 3]]
    }

    #[test]
    fn renders_a_page_at_the_scale_with_the_page_count() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = open(&write_pdf(dir.path(), 3)).unwrap();
        let page = render_page(&pdf, 1, 2.0).unwrap();
        assert_eq!(page.count, 3);
        assert_eq!((page.raw.width, page.raw.height), (400, 200));
        assert_eq!(page.raw.bytes.len(), 400 * 200 * 4);
        // PDF y grows upwards: the square is in the bottom-left corner of the picture.
        assert_eq!(pixel(&page.raw, 2, 197), [0, 0, 0, 255]);
        assert_eq!(pixel(&page.raw, 399, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn a_missing_page_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = open(&write_pdf(dir.path(), 2)).unwrap();
        assert!(render_page(&pdf, 2, 1.0).unwrap_err().contains("page 3"));
    }

    #[test]
    fn a_damaged_or_missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.pdf");
        std::fs::write(&bad, b"not a pdf").unwrap();
        assert!(open(&bad).is_err());
        assert!(open(&dir.path().join("missing.pdf")).is_err());
    }

    #[test]
    fn thumbnail_fits_the_box() {
        let dir = tempfile::tempdir().unwrap();
        let thumb = render_thumb(&write_pdf(dir.path(), 1), 100).unwrap();
        assert_eq!((thumb.width, thumb.height), (100, 50));
    }

    #[test]
    fn the_worker_serves_comics_and_goes_back_to_pdf_files() {
        let dir = tempfile::tempdir().unwrap();
        let comic = crate::comic::tests::sample_cbz(dir.path());
        let pdf = write_pdf(dir.path(), 2);
        let (answer_tx, answer_rx) = async_channel::unbounded();
        let requests = start_worker(answer_tx);
        for (generation, path, index, count, width) in [(1, &comic, 1, 3, Some(20)), (2, &comic, 2, 3, Some(30)), (3, &pdf, 0, 2, None), (4, &comic, 0, 3, Some(10))] {
            requests.send(PageRequest { generation, path: path.clone(), index }).unwrap();
            let (got, result) = answer_rx.recv_blocking().unwrap();
            let page = result.unwrap();
            assert_eq!((got, page.count), (generation, count));
            if let Some(width) = width { assert_eq!(page.raw.width, width); }
        }
        requests.send(PageRequest { generation: 5, path: dir.path().join("missing.cbz"), index: 0 }).unwrap();
        assert!(answer_rx.recv_blocking().unwrap().1.is_err());
    }

    #[test]
    fn huge_pages_are_scaled_down() {
        assert_eq!(_clamped_scale(200.0, 100.0, 2.0), 2.0);
        assert_eq!(_clamped_scale(8192.0, 100.0, 2.0), 1.0);
        assert_eq!(_clamped_scale(100.0, 16384.0, 2.0), 0.5);
    }

    #[test]
    fn the_worker_answers_with_the_request_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_pdf(dir.path(), 2);
        let (answer_tx, answer_rx) = async_channel::unbounded();
        let requests = start_worker(answer_tx);
        requests.send(PageRequest { generation: 7, path: path.clone(), index: 1 }).unwrap();
        let (generation, result) = answer_rx.recv_blocking().unwrap();
        assert_eq!(generation, 7);
        assert_eq!(result.unwrap().count, 2);
        requests.send(PageRequest { generation: 8, path: dir.path().join("missing.pdf"), index: 0 }).unwrap();
        let (generation, result) = answer_rx.recv_blocking().unwrap();
        assert_eq!(generation, 8);
        assert!(result.is_err());
        // Back to the first document, then another one: each is opened anew.
        let other = dir.path().join("other.pdf");
        std::fs::write(&other, make_pdf(3)).unwrap();
        for (generation, path, count) in [(9, &path, 2), (10, &path, 2), (11, &other, 3)] {
            requests.send(PageRequest { generation, path: path.clone(), index: 0 }).unwrap();
            let (got, result) = answer_rx.recv_blocking().unwrap();
            assert_eq!((got, result.unwrap().count), (generation, count));
        }
    }

    #[test]
    fn a_shared_cache_renders_the_same_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = open(&write_pdf(dir.path(), 2)).unwrap();
        let cache = RenderCache::new();
        for index in [0, 1, 0] {
            let cached = render_page_cached(&pdf, index, 1.0, &cache).unwrap();
            let fresh = render_page(&pdf, index, 1.0).unwrap();
            assert_eq!(cached.raw.bytes, fresh.raw.bytes, "page {index}");
        }
    }
}
