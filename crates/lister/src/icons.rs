//! Embedded SVG icons and helper functions to build iconified GTK4 widgets.

use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

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
pub const SVG_PIN: &[u8] = include_bytes!("../../../assets/icons/pin.svg");

/// Create a `gtk::Image` widget from raw SVG bytes with the desired pixel size.
pub fn svg_image(svg_bytes: &'static [u8], pixel_size: i32) -> gtk::Image {
    let bytes = glib::Bytes::from_static(svg_bytes);
    let texture = gdk::Texture::from_bytes(&bytes).expect("valid SVG asset");
    let img = gtk::Image::from_paintable(Some(&texture));
    img.set_pixel_size(pixel_size);
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
