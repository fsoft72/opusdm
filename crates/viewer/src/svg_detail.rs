//! Sharp detail of a zoomed SVG. The picture on screen is decoded once at its
//! own size; zoomed in past 100% it is only stretched, so the curves show their
//! pixels. This worker renders just a window of the drawing (the part on screen
//! and a margin) from its vectors at the pixels of the zoom, to be drawn over
//! the stretched picture, like the PDF detail. It keeps the parsed file, and a
//! newer request makes the older ones not worth rendering.

use crate::loader::RawImage;
use crate::pdf_detail::{DetailAnswer, DetailTile, MAX_TILE_SIDE};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;
use resvg::usvg::fontdb;
use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

/// A window of the SVG the main thread wants: tagged with the generation it was asked for.
pub struct SvgRequest {
    pub generation: u64,
    pub path: PathBuf,
    /// Size of the decoded picture on screen, before the zoom.
    pub picture: (u32, u32),
    /// Zoom of the picture on screen (screen pixels per pixel of the picture).
    pub zoom: f64,
    pub rotation: u16,
    /// Top-left corner of the window, in pixels of the unturned picture at this zoom.
    pub origin: (i32, i32),
    /// Size of the window in pixels.
    pub size: (u32, u32),
}

/// Start the SVG detail worker; it ends when the sender is dropped.
pub fn start_worker(answers: async_channel::Sender<DetailAnswer>) -> mpsc::Sender<SvgRequest> {
    let (tx, rx) = mpsc::channel::<SvgRequest>();
    std::thread::spawn(move || {
        // The system fonts take a while to list: only when an SVG is really zoomed.
        let fonts = OnceCell::new();
        let mut opened: Option<(PathBuf, Result<usvg::Tree, String>)> = None;
        while let Ok(first) = rx.recv() {
            let mut request = first;
            while let Ok(newer) = rx.try_recv() { request = newer; }
            if opened.as_ref().map(|(path, _)| path) != Some(&request.path) {
                let database = fonts.get_or_init(_system_fonts).clone();
                opened = Some((request.path.clone(), parse(&request.path, database)));
            }
            let result = match opened.as_ref().map(|(_, tree)| tree) {
                Some(Ok(tree)) => render_tile(tree, &request),
                Some(Err(e)) => Err(e.clone()),
                None => Err("no SVG was opened".to_string()),
            };
            if answers.send_blocking((request.generation, result)).is_err() {
                return opusdm_ipc::err_log!("viewer", "the SVG detail renderer stopped: the window is gone");
            }
        }
    });
    tx
}

/// The fonts of the system, for the text of the drawings.
fn _system_fonts() -> Arc<fontdb::Database> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    Arc::new(database)
}

/// Read and parse the SVG at `path`; relative images are looked up next to it.
pub fn parse(path: &Path, fonts: Arc<fontdb::Database>) -> Result<usvg::Tree, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let options = usvg::Options { resources_dir: path.parent().map(Path::to_path_buf), fontdb: fonts, ..Default::default() };
    usvg::Tree::from_data(&data, &options).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

/// Render the window of `request`, with straight alpha like the decoded pictures.
pub fn render_tile(tree: &usvg::Tree, request: &SvgRequest) -> Result<DetailTile, String> {
    let (w, h) = request.size;
    if w == 0 || h == 0 || w > MAX_TILE_SIDE || h > MAX_TILE_SIDE { return Err(format!("detail window of {w} x {h} pixels")); }
    let Some(mut pixmap) = Pixmap::new(w, h) else { return Err(format!("no memory for a window of {w} x {h} pixels")) };
    // Measured on the decoded picture: the two renderers may size a drawing differently.
    let drawing = tree.size();
    let scale_x = request.zoom * request.picture.0 as f64 / drawing.width() as f64;
    let scale_y = request.zoom * request.picture.1 as f64 / drawing.height() as f64;
    let transform = Transform::from_row(scale_x as f32, 0.0, 0.0, scale_y as f32, -request.origin.0 as f32, -request.origin.1 as f32);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    let bytes = pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect();
    let raw = RawImage { width: w, height: h, rowstride: (w * 4) as i32, has_alpha: true, bytes };
    Ok(DetailTile { page: 0, zoom: request.zoom, rotation: request.rotation, origin: request.origin, scale: request.zoom as f32, raw })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10 x 10 drawing: the left half has the fill `color`, the rest is transparent.
    fn tree(color: &str) -> usvg::Tree {
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="5" height="10" fill="{color}"/></svg>"#);
        usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap()
    }

    /// A request for a window of a picture decoded at `picture`.
    fn request(picture: (u32, u32), zoom: f64, origin: (i32, i32), size: (u32, u32)) -> SvgRequest {
        SvgRequest { generation: 1, path: PathBuf::new(), picture, zoom, rotation: 0, origin, size }
    }

    /// The RGBA bytes of one pixel.
    fn pixel(raw: &RawImage, x: u32, y: u32) -> &[u8] {
        let at = (y * raw.rowstride as u32 + x * 4) as usize;
        &raw.bytes[at..at + 4]
    }

    #[test]
    fn a_zoomed_edge_is_redrawn_from_the_vectors_not_stretched() {
        // At 1600% the edge of the rectangle is at pixel 80; stretched, it would be a ramp.
        let tile = render_tile(&tree("#ff0000"), &request((10, 10), 16.0, (70, 0), (20, 4))).unwrap();
        assert_eq!(pixel(&tile.raw, 9, 0), &[255, 0, 0, 255]);
        assert_eq!(pixel(&tile.raw, 10, 0)[3], 0);
    }

    #[test]
    fn the_scale_follows_the_size_of_the_decoded_picture() {
        // Decoded twice as big as the drawing says: at 200% the edge is at pixel 20.
        let tile = render_tile(&tree("#ff0000"), &request((20, 20), 2.0, (0, 0), (40, 1))).unwrap();
        assert_eq!(pixel(&tile.raw, 19, 0)[3], 255);
        assert_eq!(pixel(&tile.raw, 20, 0)[3], 0);
    }

    #[test]
    fn a_transparent_colour_keeps_its_straight_value() {
        let tile = render_tile(&tree("rgba(255,0,0,0.5)"), &request((10, 10), 2.0, (0, 0), (2, 2))).unwrap();
        let p = pixel(&tile.raw, 0, 0);
        assert_eq!(p[0], 255, "not premultiplied: {p:?}");
        assert!((126..=129).contains(&p[3]), "half transparent: {p:?}");
    }

    #[test]
    fn a_window_too_big_or_empty_is_an_error() {
        let t = tree("#000000");
        assert!(render_tile(&t, &request((10, 10), 2.0, (0, 0), (0, 10))).is_err());
        assert!(render_tile(&t, &request((10, 10), 2.0, (0, 0), (MAX_TILE_SIDE + 1, 10))).is_err());
    }

    #[test]
    fn a_damaged_or_missing_file_is_an_error_with_a_message() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.svg");
        std::fs::write(&bad, "not a drawing").unwrap();
        let fonts = Arc::new(fontdb::Database::new());
        assert!(parse(&bad, fonts.clone()).unwrap_err().contains("bad.svg"));
        assert!(parse(&dir.path().join("missing.svg"), fonts).unwrap_err().contains("missing.svg"));
    }

    #[test]
    fn the_worker_answers_with_the_generation_it_was_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.svg");
        std::fs::write(&file, r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#).unwrap();
        let (answer_tx, answer_rx) = async_channel::unbounded();
        let tx = start_worker(answer_tx);
        tx.send(SvgRequest { generation: 7, path: file, picture: (10, 10), zoom: 2.0, rotation: 0, origin: (0, 0), size: (4, 4) }).unwrap();
        let (generation, result) = answer_rx.recv_blocking().unwrap();
        assert_eq!(generation, 7);
        assert_eq!(pixel(&result.unwrap().raw, 0, 0), &[0, 0, 0, 255]);
    }
}
