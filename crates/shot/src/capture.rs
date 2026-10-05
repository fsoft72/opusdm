//! Screenshot of the X11 screen through `GetImage` on the root window.

use image::RgbaImage;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat, ImageOrder};

/// Bits per pixel of the only layout supported (BGRX, 8 bits a channel).
const SUPPORTED_BPP: u8 = 32;

/// Capture the part of the screen at `x`, `y` of size `w` x `h` (root window
/// coordinates). Fails with the X11 cause when the server refuses or the pixel
/// layout is not 32 bit little endian.
pub fn capture(x: i32, y: i32, w: u32, h: u32) -> Result<RgbaImage, String> {
    if w == 0 || h == 0 { return Err("cannot capture an empty area".to_string()); }
    let (conn, screen_num) = x11rb::connect(None).map_err(|e| format!("cannot connect to the X server: {e}"))?;
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let format = setup.pixmap_formats.iter().find(|f| f.depth == screen.root_depth)
        .ok_or_else(|| format!("no pixmap format for depth {}", screen.root_depth))?;
    if format.bits_per_pixel != SUPPORTED_BPP || setup.image_byte_order != ImageOrder::LSB_FIRST {
        return Err(format!("unsupported pixel layout: {} bpp, byte order {:?}", format.bits_per_pixel, setup.image_byte_order));
    }
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, screen.root, x as i16, y as i16, w as u16, h as u16, !0)
        .map_err(|e| format!("GetImage request failed: {e}"))?
        .reply()
        .map_err(|e| format!("GetImage was refused for {w}x{h} at {x},{y}: {e}"))?;
    bgrx_to_image(&reply.data, w, h)
}

/// Turn little endian BGRX pixels (the X server's 32 bit layout) into an
/// opaque RGBA image.
pub fn bgrx_to_image(data: &[u8], w: u32, h: u32) -> Result<RgbaImage, String> {
    let expected = w as usize * h as usize * 4;
    if data.len() != expected {
        return Err(format!("expected {expected} bytes for {w}x{h}, the server sent {}", data.len()));
    }
    let rgba: Vec<u8> = data.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
    RgbaImage::from_raw(w, h, rgba).ok_or_else(|| "pixel buffer does not fit the image".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgrx_becomes_opaque_rgba() {
        let img = bgrx_to_image(&[10, 20, 30, 0, 1, 2, 3, 7], 2, 1).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [30, 20, 10, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [3, 2, 1, 255]);
    }

    #[test]
    fn a_short_buffer_is_an_error() {
        assert!(bgrx_to_image(&[0; 7], 2, 1).unwrap_err().contains("expected 8 bytes"));
    }

    #[test]
    fn an_empty_area_is_an_error() {
        assert!(capture(0, 0, 0, 10).unwrap_err().contains("empty"));
    }
}
