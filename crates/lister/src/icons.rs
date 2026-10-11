//! Embedded SVG icons and helper functions to build iconified GTK4 widgets.

use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;

// Embed SVG assets at compile-time so the binary has zero runtime file dependencies.
pub const SVG_COPY: &[u8] = include_bytes!("../../../assets/icons/copy.svg");
pub const SVG_MOVE: &[u8] = include_bytes!("../../../assets/icons/file-arrow-right.svg");
pub const SVG_DELETE: &[u8] = include_bytes!("../../../assets/icons/trash.svg");
pub const SVG_MKDIR: &[u8] = include_bytes!("../../../assets/icons/folder-plus.svg");
pub const SVG_NEWFILE: &[u8] = include_bytes!("../../../assets/icons/file-plus.svg");
pub const SVG_RENAME: &[u8] = include_bytes!("../../../assets/icons/edit.svg");
pub const SVG_EYE: &[u8] = include_bytes!("../../../assets/icons/eye.svg");
pub const SVG_EYE_OFF: &[u8] = include_bytes!("../../../assets/icons/eye-off.svg");
pub const SVG_HOME: &[u8] = include_bytes!("../../../assets/icons/home.svg");
pub const SVG_PARENT: &[u8] = include_bytes!("../../../assets/icons/arrow-up.svg");
pub const SVG_BACK: &[u8] = include_bytes!("../../../assets/icons/arrow-left.svg");
pub const SVG_FORWARD: &[u8] = include_bytes!("../../../assets/icons/arrow-right.svg");
pub const SVG_HISTORY: &[u8] = include_bytes!("../../../assets/icons/chevron-down.svg");
pub const SVG_PIN: &[u8] = include_bytes!("../../../assets/icons/pin.svg");
pub const SVG_STAR: &[u8] = include_bytes!("../../../assets/icons/star.svg");

thread_local! {
    /// The colour `currentColor` is drawn in, `None` until the theme sets it
    /// (the SVG's own default, black, is used meanwhile).
    static ICON_COLOUR: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Every image built by [`svg_image`], so a theme change can re-tint it.
    static TINTED: RefCell<Vec<(glib::WeakRef<gtk::Image>, &'static [u8])>> = const { RefCell::new(Vec::new()) };
}

/// Whether `hex` is a plain `#rgb` / `#rrggbb` / `#rrggbbaa` colour, safe to
/// put into an SVG document.
fn is_hex_colour(hex: &str) -> bool {
    let Some(digits) = hex.strip_prefix('#') else { return false };
    matches!(digits.len(), 3 | 6 | 8) && digits.chars().all(|c| c.is_ascii_hexdigit())
}

/// `svg` with every `currentColor` replaced by `colour`. Unchanged when
/// `colour` is not a hex colour.
fn tint_svg(svg: &[u8], colour: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(svg);
    if !is_hex_colour(colour) {
        return text.into_owned().into_bytes();
    }
    text.replace("currentColor", colour).into_bytes()
}

/// The texture of `svg_bytes` drawn in the current icon colour.
fn tinted_texture(svg_bytes: &'static [u8]) -> gdk::Texture {
    let bytes = match ICON_COLOUR.with_borrow(|c| c.clone()) {
        Some(colour) => glib::Bytes::from_owned(tint_svg(svg_bytes, &colour)),
        None => glib::Bytes::from_static(svg_bytes),
    };
    gdk::Texture::from_bytes(&bytes).expect("valid SVG asset")
}

/// Draw every icon built by [`svg_image`] in `colour` from now on, the ones
/// already on screen included. Images that were destroyed are forgotten.
pub fn set_icon_colour(colour: &str) {
    if ICON_COLOUR.with_borrow(|c| c.as_deref() == Some(colour)) {
        return;
    }
    ICON_COLOUR.set(Some(colour.to_string()));
    TINTED.with_borrow_mut(|images| {
        images.retain(|(weak, svg)| {
            let Some(img) = weak.upgrade() else { return false };
            img.set_paintable(Some(&tinted_texture(svg)));
            true
        });
    });
}

/// Create a `gtk::Image` widget from raw SVG bytes with the desired pixel
/// size, drawn in the icon colour of the theme (see [`set_icon_colour`]).
pub fn svg_image(svg_bytes: &'static [u8], pixel_size: i32) -> gtk::Image {
    let img = gtk::Image::from_paintable(Some(&tinted_texture(svg_bytes)));
    img.set_pixel_size(pixel_size);
    TINTED.with_borrow_mut(|images| {
        images.retain(|(weak, _)| weak.upgrade().is_some());
        images.push((img.downgrade(), svg_bytes));
    });
    img
}

/// Create a `gtk::Button` displaying an SVG icon and a tooltip.
pub fn icon_button(svg_bytes: &'static [u8], tooltip: &str, pixel_size: i32) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.set_child(Some(&svg_image(svg_bytes, pixel_size)));
    btn.set_tooltip_text(Some(tooltip));
    btn.add_css_class("opus-btn");
    btn
}

/// Create a `gtk::ToggleButton` displaying an SVG icon and a tooltip.
pub fn icon_toggle_button(svg_bytes: &'static [u8], tooltip: &str, pixel_size: i32) -> gtk::ToggleButton {
    let btn = gtk::ToggleButton::new();
    btn.set_child(Some(&svg_image(svg_bytes, pixel_size)));
    btn.set_tooltip_text(Some(tooltip));
    btn.add_css_class("opus-btn");
    btn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_replaces_current_color() {
        let svg = br#"<svg stroke="currentColor"><path fill="currentColor"/></svg>"#;
        let out = String::from_utf8(tint_svg(svg, "#a1b2c3")).unwrap();
        assert_eq!(out, r##"<svg stroke="#a1b2c3"><path fill="#a1b2c3"/></svg>"##);
    }

    #[test]
    fn tint_ignores_anything_but_a_hex_colour() {
        let svg = br#"<svg stroke="currentColor"/>"#;
        for bad in ["red", "#12", "#zzzzzz", "\"/><script/>", ""] {
            assert_eq!(tint_svg(svg, bad), svg.to_vec(), "{bad}");
        }
    }

    #[test]
    fn every_toolbar_icon_uses_current_color() {
        for svg in [SVG_COPY, SVG_MOVE, SVG_DELETE, SVG_MKDIR, SVG_NEWFILE, SVG_RENAME, SVG_EYE, SVG_EYE_OFF,
                    SVG_HOME, SVG_PARENT, SVG_BACK, SVG_FORWARD, SVG_HISTORY, SVG_PIN, SVG_STAR] {
            assert!(String::from_utf8_lossy(svg).contains("currentColor"));
        }
    }
}
