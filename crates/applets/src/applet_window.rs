//! The plain window of one applet: undecorated, at the saved or the manifest
//! size, holding a content slot for the built widget tree and an error label
//! that shows the latest error. No X11 hints or positioning yet (Tasks 10-11)
//! and the geometry is not saved yet.
//!
//! Ownership: the host's applet entry owns the [`AppletWindow`]; dropping it
//! destroys the GTK window, so the application no longer keeps it either.

use crate::manifest::Manifest;
use crate::state::AppletState;
use gtk4 as gtk;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use opusdm_ipc::err_log;

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Colour of the error text (red, as 16 bit pango channels).
const ERROR_RGB: (u16, u16, u16) = (0xd0d0, 0x1010, 0x1010);

/// Margin around the error label, in pixels.
const ERROR_MARGIN: i32 = 4;

/// The window of one applet and the two places the host fills.
pub struct AppletWindow {
    id: String,
    window: gtk::ApplicationWindow,
    /// Holds the root of the built widget tree, or nothing.
    content: gtk::Box,
    /// The latest error; hidden while empty.
    error: gtk::Label,
}

/// Build the window of applet `id` for `app`: title = manifest name, default
/// size = the saved `width` / `height`, else the manifest window size,
/// resizable as the manifest says. It is presented only when `present` is
/// true (tests build windows without ever showing them).
pub fn open(app: &gtk::Application, id: &str, manifest: &Manifest, state: &AppletState, present: bool) -> AppletWindow {
    let width = state.width.unwrap_or(manifest.window.width);
    let height = state.height.unwrap_or(manifest.window.height);
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(&manifest.name)
        .default_width(width)
        .default_height(height)
        .resizable(manifest.window.resizable)
        .decorated(false)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_vexpand(true);
    content.set_hexpand(true);
    let error = error_label();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&content);
    column.append(&error);
    window.set_child(Some(&column));
    // A window manager close must not leave a running applet without its
    // window: the applet is disabled from the Applet Browser instead.
    let applet = id.to_string();
    window.connect_close_request(move |_| {
        err_log!(APP_TAG, "applet {applet}: close request refused, disable the applet to close its window");
        glib::Propagation::Stop
    });
    if present { window.present(); }
    AppletWindow { id: id.to_string(), window, content, error }
}

/// The hidden, red, wrapping label that shows the latest error.
fn error_label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_wrap(true);
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label.set_margin_start(ERROR_MARGIN);
    label.set_margin_end(ERROR_MARGIN);
    label.set_margin_top(ERROR_MARGIN);
    label.set_margin_bottom(ERROR_MARGIN);
    let attrs = pango::AttrList::new();
    let (r, g, b) = ERROR_RGB;
    attrs.insert(pango::AttrColor::new_foreground(r, g, b));
    attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
    label.set_attributes(Some(&attrs));
    label.set_visible(false);
    label
}

impl AppletWindow {
    /// The GTK window.
    pub fn window(&self) -> &gtk::ApplicationWindow {
        &self.window
    }

    /// Put `root` in the content slot in place of what was there; `None`
    /// empties it.
    pub fn set_content(&self, root: Option<&gtk::Widget>) {
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        if let Some(root) = root { self.content.append(root); }
    }

    /// The widget in the content slot, if any.
    pub fn content(&self) -> Option<gtk::Widget> {
        self.content.first_child()
    }

    /// Show `message` as the latest error.
    pub fn show_error(&self, message: &str) {
        self.error.set_text(message);
        self.error.set_visible(!message.is_empty());
    }

    /// The error shown now, if any.
    pub fn error_text(&self) -> Option<String> {
        let text = self.error.text();
        if text.is_empty() { return None; }
        Some(text.to_string())
    }
}

impl Drop for AppletWindow {
    /// Destroy the GTK window: the application drops it from its list.
    fn drop(&mut self) {
        opusdm_ipc::debug_log!(APP_TAG, "applet {}: window destroyed", self.id);
        self.window.destroy();
    }
}
