//! Picture decoding. The decoding runs on a worker thread and hands back plain
//! bytes (`RawImage`), which cross threads; the pixbuf and the texture are built
//! on the main thread.

use gtk4::gdk;
use gtk4::gdk_pixbuf::{Colorspace, Pixbuf, PixbufRotation};
use gtk4::glib;
use std::path::Path;

/// A box `(x0, y0, x1, y1)` in pixels and the colour it is tinted with.
pub type Mark = ((f64, f64, f64, f64), [u8; 3]);

/// Decoded pixels of one picture.
#[derive(Debug, Clone)]
pub struct RawImage {
    pub width: u32,
    pub height: u32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub bytes: Vec<u8>,
}

/// Decode the whole picture, turned as its EXIF orientation says. Safe to
/// call from any thread.
pub fn load_raw(path: &Path) -> Result<RawImage, String> {
    let pixbuf = Pixbuf::from_file(path).map_err(|e| e.message().to_string())?;
    Ok(RawImage::from_pixbuf(&upright(pixbuf)))
}

/// Decode the picture scaled to fit a `px` x `px` box, turned as its EXIF
/// orientation says. Safe to call from any thread.
pub fn load_thumb(path: &Path, px: i32) -> Result<RawImage, String> {
    let pixbuf = Pixbuf::from_file_at_scale(path, px, px, true).map_err(|e| e.message().to_string())?;
    Ok(RawImage::from_pixbuf(&upright(pixbuf)))
}

/// Decode the picture scaled down to fit a `px` x `px` box, or at its own
/// size when it already fits (never scaled up), turned as its EXIF
/// orientation says. Safe to call from any thread.
pub fn load_within(path: &Path, px: i32) -> Result<RawImage, String> {
    let (_, w, h) = Pixbuf::file_info(path).ok_or_else(|| format!("{} is not a picture this viewer can read", path.display()))?;
    if w <= px && h <= px { return load_raw(path); }
    load_thumb(path, px)
}

/// Decode a picture held in memory, scaled down to fit a `px` x `px` box
/// (never scaled up), turned as its EXIF orientation says. Safe to call from
/// any thread.
pub fn decode_within(data: &glib::Bytes, px: i32) -> Result<RawImage, String> {
    let stream = gtk4::gio::MemoryInputStream::from_bytes(data);
    let pixbuf = upright(Pixbuf::from_stream(&stream, gtk4::gio::Cancellable::NONE).map_err(|e| e.message().to_string())?);
    let (w, h) = (pixbuf.width(), pixbuf.height());
    if w <= px && h <= px { return Ok(RawImage::from_pixbuf(&pixbuf)); }
    let scale = px as f64 / w.max(h) as f64;
    let (sw, sh) = (((w as f64 * scale).round() as i32).max(1), ((h as f64 * scale).round() as i32).max(1));
    let scaled = pixbuf.scale_simple(sw, sh, gtk4::gdk_pixbuf::InterpType::Bilinear).ok_or("the picture cannot be scaled")?;
    Ok(RawImage::from_pixbuf(&scaled))
}

/// `pixbuf` with the EXIF orientation the loader found applied (phone
/// pictures are stored sideways), or as it is when there is none.
pub(crate) fn upright(pixbuf: Pixbuf) -> Pixbuf {
    pixbuf.apply_embedded_orientation().unwrap_or(pixbuf)
}

impl RawImage {
    /// Copy the pixels out of a pixbuf.
    pub fn from_pixbuf(pixbuf: &Pixbuf) -> RawImage {
        RawImage {
            width: pixbuf.width().max(0) as u32,
            height: pixbuf.height().max(0) as u32,
            rowstride: pixbuf.rowstride(),
            has_alpha: pixbuf.has_alpha(),
            bytes: pixbuf.read_pixel_bytes().to_vec(),
        }
    }

    /// A copy with each box `(x0, y0, x1, y1)` (pixels) tinted with its colour,
    /// multiplied so that dark text stays readable. Boxes are clipped to the picture.
    pub fn highlighted(&self, marks: &[Mark]) -> RawImage {
        let mut out = self.clone();
        let channels = if self.has_alpha { 4 } else { 3 };
        for &((x0, y0, x1, y1), color) in marks {
            let clip = |v: f64, max: u32| v.max(0.0).min(max as f64) as usize;
            let (x0, x1) = (clip(x0.floor(), self.width), clip(x1.ceil(), self.width));
            let (y0, y1) = (clip(y0.floor(), self.height), clip(y1.ceil(), self.height));
            for y in y0..y1 {
                let row = y * self.rowstride as usize;
                for x in x0..x1 {
                    let at = row + x * channels;
                    for (c, tint) in color.iter().enumerate() {
                        out.bytes[at + c] = (out.bytes[at + c] as u16 * *tint as u16 / 255) as u8;
                    }
                }
            }
        }
        out
    }

    /// Build a pixbuf over a copy of the pixels (main thread).
    pub fn to_pixbuf(&self) -> Pixbuf {
        let bytes = glib::Bytes::from(&self.bytes[..]);
        Pixbuf::from_bytes(&bytes, Colorspace::Rgb, self.has_alpha, 8, self.width as i32, self.height as i32, self.rowstride)
    }

    /// The pixbuf turned clockwise by `rotation` degrees (0, 90, 180 or 270).
    pub fn rotated_pixbuf(&self, rotation: u16) -> Pixbuf {
        let pixbuf = self.to_pixbuf();
        let turn = match rotation % 360 {
            90 => PixbufRotation::Clockwise,
            180 => PixbufRotation::Upsidedown,
            270 => PixbufRotation::Counterclockwise,
            _ => return pixbuf,
        };
        pixbuf.rotate_simple(turn).unwrap_or(pixbuf)
    }

    /// The texture to show for `rotation`.
    pub fn texture(&self, rotation: u16) -> gdk::Texture {
        gdk::Texture::for_pixbuf(&self.rotated_pixbuf(rotation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a `w` x `h` PNG whose top-left pixel is red and the rest blue.
    fn write_png(path: &Path, w: i32, h: i32) {
        let pixbuf = Pixbuf::new(Colorspace::Rgb, false, 8, w, h).unwrap();
        pixbuf.fill(0x0000ffff);
        pixbuf.put_pixel(0, 0, 255, 0, 0, 255);
        pixbuf.savev(path, "png", &[]).unwrap();
    }

    #[test]
    fn loads_a_png() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        write_png(&file, 4, 2);
        let raw = load_raw(&file).unwrap();
        assert_eq!((raw.width, raw.height), (4, 2));
        assert!(!raw.has_alpha);
        assert!(raw.bytes.len() >= (raw.rowstride * 2) as usize - raw.rowstride as usize + 12);
    }

    #[test]
    fn thumbnail_keeps_the_aspect_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        write_png(&file, 40, 20);
        let thumb = load_thumb(&file, 10).unwrap();
        assert_eq!((thumb.width, thumb.height), (10, 5));
    }

    #[test]
    fn load_within_scales_big_pictures_down_but_never_up() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.png");
        write_png(&big, 40, 20);
        let raw = load_within(&big, 10).unwrap();
        assert_eq!((raw.width, raw.height), (10, 5));
        let small = dir.path().join("small.png");
        write_png(&small, 4, 2);
        let raw = load_within(&small, 10).unwrap();
        assert_eq!((raw.width, raw.height), (4, 2));
        assert!(load_within(&dir.path().join("missing.png"), 10).is_err());
    }

    #[test]
    fn decode_within_reads_memory_and_scales_big_pictures_down_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        write_png(&file, 40, 20);
        let bytes = glib::Bytes::from(&std::fs::read(&file).unwrap()[..]);
        let raw = decode_within(&bytes, 10).unwrap();
        assert_eq!((raw.width, raw.height), (10, 5));
        let raw = decode_within(&bytes, 100).unwrap();
        assert_eq!((raw.width, raw.height), (40, 20));
        assert!(decode_within(&glib::Bytes::from(b"junk"), 10).is_err());
    }

    #[test]
    fn rotation_swaps_the_sides_and_moves_the_red_pixel() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        write_png(&file, 4, 2);
        let raw = load_raw(&file).unwrap();
        let cw = raw.rotated_pixbuf(90);
        assert_eq!((cw.width(), cw.height()), (2, 4));
        // Clockwise: the top-left pixel ends in the top-right corner.
        assert_eq!(cw.pixel_bytes_at(1, 0), [255, 0, 0]);
        let ccw = raw.rotated_pixbuf(270);
        assert_eq!((ccw.width(), ccw.height()), (2, 4));
        assert_eq!(ccw.pixel_bytes_at(0, 3), [255, 0, 0]);
        let half = raw.rotated_pixbuf(180);
        assert_eq!((half.width(), half.height()), (4, 2));
        assert_eq!(half.pixel_bytes_at(3, 1), [255, 0, 0]);
        let none = raw.rotated_pixbuf(0);
        assert_eq!(none.pixel_bytes_at(0, 0), [255, 0, 0]);
    }

    /// Write a 16 x 8 JPEG, left half red and right half blue, tagged with
    /// the EXIF `orientation` (an APP1 segment spliced in after SOI).
    fn write_jpeg_with_orientation(path: &Path, orientation: u8) {
        let pixbuf = Pixbuf::new(Colorspace::Rgb, false, 8, 16, 8).unwrap();
        pixbuf.fill(0x0000ffff);
        for y in 0..8 {
            for x in 0..8 {
                pixbuf.put_pixel(x, y, 255, 0, 0, 255);
            }
        }
        let jpeg = pixbuf.save_to_bufferv("jpeg", &[("quality", "95")]).unwrap();
        // TIFF little endian, one IFD entry: Orientation (0x0112), SHORT, 1.
        let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        exif.extend_from_slice(&[orientation, 0, 0, 0, 0, 0, 0, 0]);
        let len = (exif.len() + 2) as u16;
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&exif);
        out.extend_from_slice(&jpeg[2..]);
        std::fs::write(path, out).unwrap();
    }

    /// Whether the RGB pixel at `at` of `raw` is mostly red.
    fn is_red(raw: &RawImage, (x, y): (u32, u32)) -> bool {
        let channels = if raw.has_alpha { 4 } else { 3 };
        let offset = (y * raw.rowstride as u32 + x * channels) as usize;
        raw.bytes[offset] > 200 && raw.bytes[offset + 2] < 60
    }

    #[test]
    fn exif_orientation_is_applied_to_full_pictures_and_thumbnails() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("phone.jpg");
        // 6: the camera was turned, show the picture turned 90 degrees clockwise.
        write_jpeg_with_orientation(&file, 6);
        let raw = load_raw(&file).unwrap();
        assert_eq!((raw.width, raw.height), (8, 16));
        // The red left half ends at the top.
        assert!(is_red(&raw, (4, 2)));
        assert!(!is_red(&raw, (4, 13)));
        let thumb = load_thumb(&file, 8).unwrap();
        assert_eq!((thumb.width, thumb.height), (4, 8));
    }

    #[test]
    fn highlight_multiplies_the_colour_inside_the_clipped_box() {
        let raw = RawImage { width: 3, height: 2, rowstride: 12, has_alpha: true, bytes: vec![255; 24] };
        let marked = raw.highlighted(&[((-5.0, 0.0, 1.0, 1.0), [255, 128, 0]), ((2.0, 1.0, 99.0, 99.0), [0, 0, 0])]);
        assert_eq!(&marked.bytes[0..4], &[255, 128, 0, 255]);
        assert_eq!(&marked.bytes[4..8], &[255, 255, 255, 255]);
        assert_eq!(&marked.bytes[20..24], &[0, 0, 0, 255]);
        assert_eq!(raw.bytes, vec![255; 24], "the original is not touched");
    }

    #[test]
    fn a_damaged_file_is_an_error_with_a_message() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.png");
        std::fs::write(&file, b"not a picture").unwrap();
        let err = load_raw(&file).unwrap_err();
        assert!(!err.is_empty());
        assert!(load_raw(&dir.path().join("missing.png")).is_err());
    }

    /// Test helper: the RGB bytes of one pixel.
    trait PixelAt {
        fn pixel_bytes_at(&self, x: i32, y: i32) -> [u8; 3];
    }

    impl PixelAt for Pixbuf {
        fn pixel_bytes_at(&self, x: i32, y: i32) -> [u8; 3] {
            let channels = self.n_channels();
            let offset = (y * self.rowstride() + x * channels) as usize;
            let bytes = self.read_pixel_bytes();
            [bytes[offset], bytes[offset + 1], bytes[offset + 2]]
        }
    }
}
