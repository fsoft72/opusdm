//! Screenshot of the screen: `GetImage` on the root window on X11, the
//! compositor's screen copy (`opusdm_wayland::screencopy`) on Wayland.

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
    if opusdm_ipc::session::is_wayland() {
        let outputs = opusdm_wayland::screencopy::capture_outputs()?;
        return compose(&outputs, x, y, w, h);
    }
    let opusdm_x11::Session { conn, screen_num, .. } =
        opusdm_x11::connect().map_err(|e| format!("cannot connect to the X server: {e}"))?;
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

/// The area at `x`, `y` of size `w` x `h` out of the output pictures. An
/// output placed exactly there is returned whole, at its own resolution (a
/// scaled HiDPI output has more pixels than its area); otherwise the
/// outputs are pasted at their places, one pixel per unit.
pub fn compose(outputs: &[opusdm_wayland::screencopy::OutputImage], x: i32, y: i32, w: u32, h: u32) -> Result<RgbaImage, String> {
    let image_of = |o: &opusdm_wayland::screencopy::OutputImage| {
        RgbaImage::from_raw(o.width, o.height, o.rgba.clone()).ok_or_else(|| format!("bad picture of output {}", o.name))
    };
    if let Some(exact) = outputs.iter().find(|o| (o.x, o.y) == (x, y) && o.width >= w && o.height >= h) {
        return image_of(exact);
    }
    let mut out = RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255]));
    let mut covered = false;
    for output in outputs {
        let (dx, dy) = (i64::from(output.x) - i64::from(x), i64::from(output.y) - i64::from(y));
        if dx >= i64::from(w) || dy >= i64::from(h) || dx + i64::from(output.width) <= 0 || dy + i64::from(output.height) <= 0 { continue; }
        image::imageops::replace(&mut out, &image_of(output)?, dx, dy);
        covered = true;
    }
    if !covered { return Err(format!("no output covers {w}x{h} at {x},{y}")); }
    Ok(out)
}

/// Turn little endian BGRX pixels (the X server's 32 bit layout) into an
/// opaque RGBA image.
pub fn bgrx_to_image(data: &[u8], w: u32, h: u32) -> Result<RgbaImage, String> {
    let expected = w as usize * h as usize * 4;
    if data.len() != expected {
        return Err(format!("expected {expected} bytes for {w}x{h}, the server sent {}", data.len()));
    }
    let rgba: Vec<u8> = data.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
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

    fn output(name: &str, x: i32, y: i32, w: u32, h: u32, value: u8) -> opusdm_wayland::screencopy::OutputImage {
        opusdm_wayland::screencopy::OutputImage { name: name.into(), x, y, width: w, height: h, rgba: vec![value; (w * h * 4) as usize] }
    }

    #[test]
    fn an_output_at_the_area_is_taken_whole() {
        let hidpi = output("eDP-1", 0, 0, 4, 2, 7);
        let img = compose(&[hidpi], 0, 0, 2, 1).unwrap();
        assert_eq!(img.dimensions(), (4, 2), "the full resolution is kept");
    }

    #[test]
    fn outputs_are_pasted_at_their_places() {
        let left = output("A", 0, 0, 2, 1, 10);
        let right = output("B", 2, 0, 2, 1, 20);
        let img = compose(&[left, right], 1, 0, 2, 1).unwrap();
        assert_eq!(img.get_pixel(0, 0).0[0], 10);
        assert_eq!(img.get_pixel(1, 0).0[0], 20);
        assert!(compose(&[output("A", 0, 0, 2, 1, 1)], 50, 50, 2, 1).is_err());
    }

    #[test]
    fn an_empty_area_is_an_error() {
        assert!(capture(0, 0, 0, 10).unwrap_err().contains("empty"));
    }
}
