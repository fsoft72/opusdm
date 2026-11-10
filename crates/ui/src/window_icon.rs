//! The per-application window icon. GTK4 does not write `_NET_WM_ICON` on
//! X11, so without this the task list and the window switcher show a generic
//! icon. Under Wayland the compositor finds the icon through the desktop entry
//! of the application id, so nothing is done there.

use crate::x11::{with_x11, x11_window_id};
use gtk4::prelude::*;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, PropMode};
use x11rb::wrapper::ConnectionExt as _;

/// Side, in pixels, of the icon written into `_NET_WM_ICON`.
const WINDOW_ICON_SIZE: i32 = 48;

/// Side declared by the SVGs of `assets/apps/icons` in their root element.
const DECLARED_SIDE: &str = "512";

/// `svg` with the declared `width` / `height` of the root element set to `side`.
fn sized_svg(svg: &[u8], side: i32) -> Vec<u8> {
    let text = String::from_utf8_lossy(svg);
    let text = text.replacen(&format!("width=\"{DECLARED_SIDE}\""), &format!("width=\"{side}\""), 1);
    text.replacen(&format!("height=\"{DECLARED_SIDE}\""), &format!("height=\"{side}\""), 1).into_bytes()
}

/// `svg` as `_NET_WM_ICON` words: width, height, then one `ARGB` word per
/// pixel (not premultiplied). Fails when the SVG cannot be rendered.
fn icon_words(svg: &[u8], side: i32) -> Result<Vec<u32>, String> {
    let bytes = gtk4::glib::Bytes::from_owned(sized_svg(svg, side));
    let texture = gtk4::gdk::Texture::from_bytes(&bytes).map_err(|e| e.to_string())?;
    let (w, h) = (texture.width() as usize, texture.height() as usize);
    if w == 0 || h == 0 { return Err("the icon SVG has no size".to_string()); }
    // Native download format: premultiplied ARGB, one little endian word per pixel.
    let mut data = vec![0u8; w * h * 4];
    texture.download(&mut data, w * 4);
    let mut words = vec![w as u32, h as u32];
    words.extend(data.as_chunks::<4>().0.iter().map(|px| {
        let word = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
        let a = word >> 24;
        if a == 0 || a == 255 { return word; }
        let un = |c: u32| ((c * 255 + a / 2) / a).min(255);
        (a << 24) | (un((word >> 16) & 0xff) << 16) | (un((word >> 8) & 0xff) << 8) | un(word & 0xff)
    }));
    Ok(words)
}

/// Write `words` into `_NET_WM_ICON` of the X11 window `xid`.
fn write_icon(xid: u32, words: &[u32]) -> Result<(), String> {
    with_x11(|x11| {
        let atom = x11.atom("_NET_WM_ICON")?;
        x11.conn().change_property32(PropMode::REPLACE, xid, atom, AtomEnum::CARDINAL, words)?;
        x11.conn().flush()?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

/// Give every window of `app` the icon drawn by `svg` (the bytes of a file of
/// `assets/apps/icons`, embedded with `include_bytes!`) in window lists. Call it
/// once, before the first window is created; each window gets its icon when
/// it is mapped (it needs its X11 id). Failures are logged under `name`.
pub fn set_app_window_icon(app: &gtk4::Application, name: &'static str, svg: &'static [u8]) {
    app.connect_window_added(move |_, window| {
        window.connect_map(move |window| {
            let Some(xid) = x11_window_id(window) else { return };
            let words = match icon_words(svg, WINDOW_ICON_SIZE) {
                Ok(words) => words,
                Err(e) => return opusdm_ipc::err_log!(name, "window icon: {e}"),
            };
            if let Err(e) = write_icon(xid, &words) { opusdm_ipc::err_log!(name, "window icon: {e}"); }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOT: &[u8] = include_bytes!("../../../assets/apps/icons/shot.svg");

    #[test]
    fn the_declared_side_is_replaced_once() {
        let svg = String::from_utf8(sized_svg(SHOT, 48)).unwrap();
        assert!(svg.contains("width=\"48\" height=\"48\""));
    }

    #[test]
    fn the_icon_has_the_requested_size_and_some_visible_pixels() {
        if gtk4::init().is_err() { return; }
        let words = icon_words(SHOT, 48).unwrap();
        assert_eq!((words[0], words[1]), (48, 48));
        assert_eq!(words.len(), 2 + 48 * 48);
        assert!(words[2..].iter().any(|w| w >> 24 != 0), "blank icon");
    }
}
