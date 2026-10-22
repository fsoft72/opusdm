//! Comic book archives (`.cbz` zip, `.cbr` rar): the pictures inside, in
//! natural name order, are the pages. The archive is read in one pass through
//! libarchive; the pages are decoded from memory like any picture.

use crate::gallery::natural_cmp;
use crate::loader::{upright, RawImage};
use crate::pdf::RenderedPage;
use gtk4::gdk_pixbuf::Pixbuf;
use gtk4::{gio, glib};
use std::path::Path;

/// Extensions of the archive entries that are pages (lower case).
const PAGE_EXTS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];
/// Folder of resource forks a Mac zip tool adds; its files are not pages.
const MAC_FORKS: &str = "__MACOSX/";

/// The pages of one comic, compressed as they are stored, in reading order.
pub struct Comic {
    pages: Vec<Vec<u8>>,
}

impl Comic {
    /// Number of pages.
    pub fn count(&self) -> usize {
        self.pages.len()
    }
}

/// True for an archive entry that is a page: a picture, not hidden, not a Mac resource fork.
pub fn is_page_name(name: &str) -> bool {
    if name.starts_with(MAC_FORKS) { return false; }
    let base = name.rsplit('/').next().unwrap_or(name);
    if base.starts_with('.') { return false; }
    let Some((_, ext)) = base.rsplit_once('.') else { return false };
    PAGE_EXTS.contains(&ext.to_lowercase().as_str())
}

/// Read every page of the comic at `path`. Blocking: call it off the GTK thread.
pub fn open(path: &Path) -> Result<Comic, String> {
    let mut files = opusdm_archive::read_files(path, &is_page_name).map_err(|e| e.to_string())?;
    if files.is_empty() { return Err("the comic has no pages".to_string()); }
    files.sort_by(|a, b| natural_cmp(&a.0, &b.0));
    Ok(Comic { pages: files.into_iter().map(|(_, data)| data).collect() })
}

/// Page `index` (0-based) of `comic`, at its own size.
pub fn render_page(comic: &Comic, index: usize) -> Result<RenderedPage, String> {
    let count = comic.count();
    let Some(data) = comic.pages.get(index) else { return Err(format!("page {} does not exist ({count} pages)", index + 1)) };
    let raw = decode(data, None)?;
    Ok(RenderedPage { raw, count, scale: 1.0, links: Vec::new(), text: Default::default() })
}

/// Page `index` of `comic` scaled to `width` pixels wide, for the page panel.
pub fn render_page_thumb(comic: &Comic, index: usize, width: i32) -> Result<RawImage, String> {
    let data = comic.pages.get(index).ok_or_else(|| format!("page {} does not exist", index + 1))?;
    decode(data, Some((width, -1)))
}

/// The first page scaled to fit a `px` x `px` box, for the thumbnail strip.
/// Only that page is extracted.
pub fn render_thumb(path: &Path, px: i32) -> Result<RawImage, String> {
    let entries = opusdm_archive::list(path).map_err(|e| e.to_string())?;
    let first = entries
        .iter()
        .filter(|e| e.kind == opusdm_archive::EntryKind::File && is_page_name(&e.path))
        .min_by(|a, b| natural_cmp(&a.path, &b.path))
        .ok_or("the comic has no pages")?;
    let mut data = Vec::new();
    opusdm_archive::extract_entry(path, &first.path, &mut data).map_err(|e| e.to_string())?;
    decode(&data, Some((px, px)))
}

/// Decode a stored picture, scaled to fit `fit` (`-1` leaves a side free)
/// when given, turned as its EXIF orientation says. Safe on any thread.
fn decode(data: &[u8], fit: Option<(i32, i32)>) -> Result<RawImage, String> {
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from(data));
    let pixbuf = match fit {
        Some((w, h)) => Pixbuf::from_stream_at_scale(&stream, w, h, true, gio::Cancellable::NONE),
        None => Pixbuf::from_stream(&stream, gio::Cancellable::NONE),
    };
    let pixbuf = pixbuf.map_err(|e| format!("a page cannot be decoded: {}", e.message()))?;
    Ok(RawImage::from_pixbuf(&upright(pixbuf)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gtk4::gdk_pixbuf::Colorspace;
    use std::io::Write;

    /// PNG bytes of a `w` x `h` picture.
    fn png(w: i32, h: i32) -> Vec<u8> {
        let pixbuf = Pixbuf::new(Colorspace::Rgb, false, 8, w, h).unwrap();
        pixbuf.fill(0x00ff00ff);
        pixbuf.save_to_bufferv("png", &[]).unwrap()
    }

    /// Write a `.cbz` holding `entries` (name, bytes) in that order.
    pub(crate) fn write_cbz(path: &Path, entries: &[(&str, Vec<u8>)]) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    /// A `.cbz` whose pages are 10, 20 and 30 pixels wide (stored out of order).
    pub(crate) fn sample_cbz(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("comic.cbz");
        write_cbz(&path, &[("p10.png", png(30, 6)), ("p2.png", png(20, 6)), ("p1.png", png(10, 6)), ("ComicInfo.xml", b"<x/>".to_vec()), ("__MACOSX/._p1.png", b"fork".to_vec())]);
        path
    }

    #[test]
    fn pages_are_pictures_only() {
        assert!(is_page_name("001.jpg"));
        assert!(is_page_name("Chapter 1/Page 02.JPEG"));
        assert!(is_page_name("a.webp"));
        assert!(!is_page_name("ComicInfo.xml"));
        assert!(!is_page_name("__MACOSX/._001.jpg"));
        assert!(!is_page_name("sub/.hidden.png"));
        assert!(!is_page_name("noext"));
    }

    #[test]
    fn pages_come_in_natural_order() {
        let dir = tempfile::tempdir().unwrap();
        let comic = open(&sample_cbz(dir.path())).unwrap();
        assert_eq!(comic.count(), 3);
        let widths: Vec<u32> = (0..3).map(|i| render_page(&comic, i).unwrap().raw.width).collect();
        assert_eq!(widths, vec![10, 20, 30], "p1, p2, p10");
        let page = render_page(&comic, 0).unwrap();
        assert_eq!((page.count, page.scale), (3, 1.0));
        assert!(render_page(&comic, 3).is_err());
    }

    #[test]
    fn thumbnails_fit_their_box() {
        let dir = tempfile::tempdir().unwrap();
        let path = sample_cbz(dir.path());
        let thumb = render_thumb(&path, 5).unwrap();
        assert_eq!((thumb.width, thumb.height), (5, 3), "the first page, 10 x 6, fits a 5 x 5 box");
        let comic = open(&path).unwrap();
        assert_eq!(render_page_thumb(&comic, 2, 15).unwrap().width, 15);
    }

    #[test]
    fn a_comic_without_pictures_or_a_damaged_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.cbz");
        write_cbz(&empty, &[("readme.txt", b"hi".to_vec())]);
        assert!(open(&empty).err().expect("no pages is an error").contains("no pages"));
        assert!(render_thumb(&empty, 10).is_err());
        let bad = dir.path().join("bad.cbz");
        std::fs::write(&bad, b"not an archive").unwrap();
        assert!(open(&bad).is_err());
        let broken = dir.path().join("broken.cbz");
        write_cbz(&broken, &[("1.png", b"not a png".to_vec())]);
        let comic = open(&broken).unwrap();
        assert!(render_page(&comic, 0).unwrap_err().contains("cannot be decoded"));
    }
}
