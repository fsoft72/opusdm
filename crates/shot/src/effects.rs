//! Pixel effects of the redaction tools (pixelate and blur). Pure functions on
//! images, no GUI and no cairo.

use image::{imageops, Rgba, RgbaImage};

/// Replace every `block` x `block` square (aligned to the top left corner) with
/// its average colour. A block of 1 or less returns the image unchanged.
pub fn pixelate(img: &RgbaImage, block: u32) -> RgbaImage {
    if block <= 1 { return img.clone(); }
    let (w, h) = img.dimensions();
    let mut out = img.clone();
    for by in (0..h).step_by(block as usize) {
        for bx in (0..w).step_by(block as usize) {
            let (x1, y1) = ((bx + block).min(w), (by + block).min(h));
            let mut sum = [0u64; 4];
            for y in by..y1 {
                for x in bx..x1 {
                    for (s, c) in sum.iter_mut().zip(img.get_pixel(x, y).0) { *s += c as u64; }
                }
            }
            let n = ((x1 - bx) * (y1 - by)) as u64;
            let avg = Rgba(sum.map(|s| ((s + n / 2) / n) as u8));
            for y in by..y1 {
                for x in bx..x1 { out.put_pixel(x, y, avg); }
            }
        }
    }
    out
}

/// A gaussian blur of `img` with the given `sigma` (in pixels). A sigma of 0 or
/// less returns the image unchanged.
pub fn blur(img: &RgbaImage, sigma: f32) -> RgbaImage {
    if sigma <= 0.0 { return img.clone(); }
    imageops::blur(img, sigma)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A black and white checkerboard of 1 pixel squares.
    fn checker(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| if (x + y) % 2 == 0 { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) })
    }

    #[test]
    fn pixelate_makes_each_block_one_colour() {
        let out = pixelate(&checker(8, 8), 4);
        let first = *out.get_pixel(0, 0);
        assert!((0..4).all(|y| (0..4).all(|x| *out.get_pixel(x, y) == first)));
        assert_ne!(first, Rgba([0, 0, 0, 255]), "the checker is averaged to grey");
    }

    #[test]
    fn pixelate_handles_partial_blocks_and_keeps_the_size() {
        let out = pixelate(&checker(10, 7), 4);
        assert_eq!(out.dimensions(), (10, 7));
        assert_eq!(out.get_pixel(9, 6), out.get_pixel(8, 4));
    }

    #[test]
    fn pixelate_with_block_one_is_the_identity() {
        let img = checker(5, 5);
        assert_eq!(pixelate(&img, 1), img);
    }

    #[test]
    fn blur_smooths_hard_edges_and_keeps_the_size() {
        let img = checker(16, 16);
        let out = blur(&img, 3.0);
        assert_eq!(out.dimensions(), (16, 16));
        assert_ne!(out, img);
        let p = out.get_pixel(8, 8);
        assert!(p[0] > 60 && p[0] < 200, "centre is grey: {p:?}");
    }

    #[test]
    fn blur_with_no_sigma_is_the_identity() {
        let img = checker(5, 5);
        assert_eq!(blur(&img, 0.0), img);
    }
}
