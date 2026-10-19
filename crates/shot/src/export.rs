//! Encoding and saving of the final image as PNG, JPG or WebP.

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage, RgbaImage};
use std::fs;
use std::path::{Path, PathBuf};

/// Quality offered for JPG and WebP until the user changes it.
pub const DEFAULT_QUALITY: u8 = 90;

/// Best and worst lossy quality.
pub const MIN_QUALITY: u8 = 1;
pub const MAX_QUALITY: u8 = 100;

/// Smallest and largest scale of a saved image, in percent.
pub const MIN_SCALE_PERCENT: u32 = 1;
pub const MAX_SCALE_PERCENT: u32 = 400;

/// Folder of the quick save, under the home directory.
pub const QUICK_SAVE_DIR: &str = "Pictures/Screenshots";

/// File formats the tool can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpg,
    WebP,
}

impl Format {
    /// Every format, in the order the dialog lists them.
    pub const ALL: [Format; 3] = [Format::Png, Format::Jpg, Format::WebP];

    /// Lower-case extension, no dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpg => "jpg",
            Format::WebP => "webp",
        }
    }

    /// Name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpg => "JPG",
            Format::WebP => "WebP",
        }
    }

    /// Whether the quality setting applies.
    pub fn has_quality(self) -> bool {
        self != Format::Png
    }

    /// The format for a file name by its extension (case insensitive, `jpeg`
    /// counts as JPG). `None` when missing or unknown.
    pub fn from_path(path: &Path) -> Option<Format> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some(Format::Png),
            "jpg" | "jpeg" => Some(Format::Jpg),
            "webp" => Some(Format::WebP),
            _ => None,
        }
    }
}

/// How to encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub format: Format,
    /// 1-100, used by JPG and lossy WebP.
    pub quality: u8,
    /// WebP only: lossless instead of lossy.
    pub lossless: bool,
}

impl Options {
    /// The default options of a format.
    pub fn new(format: Format) -> Self {
        Options { format, quality: DEFAULT_QUALITY, lossless: false }
    }
}

/// Encode `image` as the format of `options`.
pub fn encode(image: &RgbaImage, options: &Options) -> Result<Vec<u8>, String> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 { return Err("the image is empty".to_string()); }
    let quality = options.quality.clamp(MIN_QUALITY, MAX_QUALITY);
    match options.format {
        Format::Png => {
            let mut out = Vec::new();
            PngEncoder::new(&mut out)
                .write_image(image.as_raw(), w, h, ExtendedColorType::Rgba8)
                .map_err(|e| format!("PNG encoding failed: {e}"))?;
            Ok(out)
        }
        Format::Jpg => {
            let mut out = Vec::new();
            let rgb = flatten_on_white(image);
            JpegEncoder::new_with_quality(&mut out, quality)
                .write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)
                .map_err(|e| format!("JPG encoding failed: {e}"))?;
            Ok(out)
        }
        Format::WebP => {
            let encoder = webp::Encoder::from_rgba(image.as_raw(), w, h);
            let memory = if options.lossless { encoder.encode_lossless() } else { encoder.encode(quality as f32) };
            Ok(memory.to_vec())
        }
    }
}

/// The size of a `width` x `height` image scaled by `percent` (clamped to the
/// allowed range): both sides use the same factor, so the aspect ratio is kept.
/// A side never gets smaller than one pixel.
pub fn scaled_size(width: u32, height: u32, percent: u32) -> (u32, u32) {
    let p = percent.clamp(MIN_SCALE_PERCENT, MAX_SCALE_PERCENT) as u64;
    let side = |v: u32| (((v as u64 * p) + 50) / 100).max(1) as u32;
    (side(width), side(height))
}

/// `image` scaled by `percent` keeping the aspect ratio. At 100% it is
/// returned untouched.
pub fn scale(image: RgbaImage, percent: u32) -> RgbaImage {
    let (w, h) = scaled_size(image.width(), image.height(), percent);
    if (w, h) == image.dimensions() { return image; }
    image::imageops::resize(&image, w, h, image::imageops::FilterType::Lanczos3)
}

/// `image` without alpha, blended over white (JPG has no transparency).
fn flatten_on_white(image: &RgbaImage) -> RgbImage {
    let mut out = RgbImage::new(image.width(), image.height());
    for (x, y, p) in image.enumerate_pixels() {
        let a = p[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        out.put_pixel(x, y, image::Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    out
}

/// Encode `image` and write it to `path`. The error names the path and the
/// cause.
pub fn save(image: &RgbaImage, path: &Path, options: &Options) -> Result<(), String> {
    let bytes = encode(image, options)?;
    fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// `dir/stem.ext`, or `dir/stem-2.ext`, `dir/stem-3.ext`... the first that does
/// not exist yet.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() { return first; }
    (2..).map(|n| dir.join(format!("{stem}-{n}.{ext}"))).find(|p| !p.exists()).expect("an unused name exists")
}

/// File name stem of a quick save for a local time (`shot-20261005-143015`).
pub fn quick_stem(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> String {
    format!("shot-{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

/// Create `dir` if needed and save `image` as a PNG in it under a unique name
/// made from `stem`. Returns the path written.
pub fn quick_save(image: &RgbaImage, dir: &Path, stem: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = unique_path(dir, stem, Format::Png.extension());
    save(image, &path, &Options::new(Format::Png))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn sample() -> RgbaImage {
        RgbaImage::from_fn(32, 24, |x, y| Rgba([(x * 8) as u8, (y * 10) as u8, 120, 255]))
    }

    #[test]
    fn format_from_path_by_extension() {
        assert_eq!(Format::from_path(Path::new("a.PNG")), Some(Format::Png));
        assert_eq!(Format::from_path(Path::new("a.jpeg")), Some(Format::Jpg));
        assert_eq!(Format::from_path(Path::new("a.webp")), Some(Format::WebP));
        assert_eq!(Format::from_path(Path::new("a.bmp")), None);
        assert_eq!(Format::from_path(Path::new("noext")), None);
    }

    #[test]
    fn png_round_trip_is_exact() {
        let img = sample();
        let bytes = encode(&img, &Options::new(Format::Png)).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        let back = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(back, img);
    }

    #[test]
    fn jpg_round_trip_keeps_size_and_is_close() {
        let img = RgbaImage::from_pixel(16, 16, Rgba([200, 40, 40, 255]));
        let bytes = encode(&img, &Options::new(Format::Jpg)).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
        let back = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (16, 16));
        let p = back.get_pixel(8, 8);
        assert!((p[0] as i32 - 200).abs() < 8 && (p[1] as i32 - 40).abs() < 8, "{p:?}");
    }

    #[test]
    fn jpg_flattens_alpha_on_white() {
        let img = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0]));
        let flat = flatten_on_white(&img);
        assert_eq!(*flat.get_pixel(0, 0), image::Rgb([255, 255, 255]));
    }

    #[test]
    fn webp_lossless_round_trip_is_exact_and_lossy_is_smaller() {
        let img = sample();
        let lossless = Options { format: Format::WebP, quality: 90, lossless: true };
        let bytes = encode(&img, &lossless).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        let decoded = webp::Decoder::new(&bytes).decode().unwrap().to_image().to_rgba8();
        assert_eq!(decoded, img);
        let lossy = encode(&img, &Options { format: Format::WebP, quality: 20, lossless: false }).unwrap();
        assert_eq!(&lossy[8..12], b"WEBP");
    }

    #[test]
    fn lower_quality_gives_a_smaller_jpg() {
        let img = RgbaImage::from_fn(64, 64, |x, y| Rgba([((x * 4) ^ (y * 3)) as u8, (x * y) as u8, (x + y) as u8, 255]));
        let high = encode(&img, &Options { format: Format::Jpg, quality: 95, lossless: false }).unwrap();
        let low = encode(&img, &Options { format: Format::Jpg, quality: 10, lossless: false }).unwrap();
        assert!(low.len() < high.len());
    }

    #[test]
    fn scaled_size_keeps_the_ratio_and_never_reaches_zero() {
        assert_eq!(scaled_size(1920, 1080, 50), (960, 540));
        assert_eq!(scaled_size(1920, 1080, 100), (1920, 1080));
        assert_eq!(scaled_size(100, 50, 200), (200, 100));
        assert_eq!(scaled_size(10, 3, 1), (1, 1));
        assert_eq!(scaled_size(100, 100, 9999), (400, 400));
    }

    #[test]
    fn scale_resizes_and_a_full_scale_is_the_same_image() {
        let img = sample();
        assert_eq!(scale(img.clone(), 100), img);
        assert_eq!(scale(img, 50).dimensions(), (16, 12));
    }

    #[test]
    fn an_empty_image_is_an_error() {
        let err = encode(&RgbaImage::new(0, 0), &Options::new(Format::Png)).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn unique_path_never_reuses_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let first = unique_path(dir.path(), "shot", "png");
        assert_eq!(first, dir.path().join("shot.png"));
        fs::write(&first, b"x").unwrap();
        let second = unique_path(dir.path(), "shot", "png");
        assert_eq!(second, dir.path().join("shot-2.png"));
        fs::write(&second, b"x").unwrap();
        assert_eq!(unique_path(dir.path(), "shot", "png"), dir.path().join("shot-3.png"));
    }

    #[test]
    fn quick_stem_is_zero_padded() {
        assert_eq!(quick_stem(2026, 3, 7, 4, 5, 9), "shot-20260307-040509");
    }

    #[test]
    fn quick_save_creates_the_folder_and_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a").join("b");
        let one = quick_save(&sample(), &target, "shot-1").unwrap();
        let two = quick_save(&sample(), &target, "shot-1").unwrap();
        assert_ne!(one, two);
        assert!(one.exists() && two.exists());
    }

    #[test]
    fn save_reports_the_path_on_failure() {
        let err = save(&sample(), Path::new("/nonexistent-dir/x.png"), &Options::new(Format::Png)).unwrap_err();
        assert!(err.contains("/nonexistent-dir/x.png"), "{err}");
    }
}
