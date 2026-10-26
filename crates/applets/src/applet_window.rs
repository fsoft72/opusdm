//! The plain window of one applet: undecorated, at the saved or the manifest
//! size, holding a content slot for the built widget tree and an error label
//! that shows the latest error. On X11 it gets the applet window hints and
//! its position when realized and mapped (see [`crate::x11_window`]); the
//! geometry is not saved yet (Task 11).
//!
//! Ownership: the host's applet entry owns the [`AppletWindow`]; dropping it
//! destroys the GTK window, so the application no longer keeps it either.
//! A window-manager close is refused here and handed to `on_close` (the
//! host disables the applet and drops the window itself), so the host stays
//! the only owner.

use crate::manifest::Manifest;
use crate::state::AppletState;
use crate::x11_window;
use gtk4 as gtk;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Colour of the error text (red, as 16 bit pango channels).
const ERROR_RGB: (u16, u16, u16) = (0xd0d0, 0x1010, 0x1010);

/// Margin around the error label, in pixels.
const ERROR_MARGIN: i32 = 4;

/// Monitor rect used when GDK lists no monitor.
const FALLBACK_MONITOR: (i32, i32, i32, i32) = (0, 0, 1280, 800);

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
/// true (tests build windows without ever showing them). A close request
/// (Alt+F4, the window manager) calls `on_close` and is otherwise refused:
/// `on_close` must hold only weak references. On X11 the window is placed at
/// the saved `x` / `y`, else at a default spot in the top right area of the
/// first monitor, cascaded by the number of windows `app` already has.
pub fn open(app: &gtk::Application, id: &str, manifest: &Manifest, state: &AppletState, present: bool, on_close: impl Fn() + 'static) -> AppletWindow {
    let width = state.width.unwrap_or(manifest.window.width);
    let height = state.height.unwrap_or(manifest.window.height);
    let (x, y) = match (state.x, state.y) {
        (Some(x), Some(y)) => (x, y),
        _ => x11_window::default_position(first_monitor(), width, app.windows().len()),
    };
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
    // Stop: GTK must not destroy the window itself; `on_close` lets the
    // host disable the applet and drop (destroy) the window.
    window.connect_close_request(move |_| {
        on_close();
        glib::Propagation::Stop
    });
    connect_x11_placement(&window, (x, y, width, height));
    if present { window.present(); }
    AppletWindow { id: id.to_string(), window, content, error }
}

/// Apply the X11 applet hints and the geometry `(x, y, w, h)` when `window`
/// is realized, again on map, and on the retries after map (the window
/// manager adopts the window shortly after map and drops state messages sent
/// before that). A window never presented (tests) is never realized, so
/// nothing is sent. The handlers hold only the geometry; the retries hold a
/// weak reference to the window and log if it is gone.
fn connect_x11_placement(window: &gtk::ApplicationWindow, geometry: (i32, i32, i32, i32)) {
    let (x, y, w, h) = geometry;
    window.connect_realize(move |win| x11_window::prepare(win, x, y, w, h));
    window.connect_map(move |win| {
        x11_window::prepare(win, x, y, w, h);
        let weak = win.downgrade();
        opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || {
            let Some(win) = weak.upgrade() else {
                opusdm_ipc::err_log!(APP_TAG, "applet window gone before an X11 placement retry (closed right after it opened?)");
                return;
            };
            x11_window::prepare(&win, x, y, w, h);
        });
    });
}

/// Rect `(x, y, w, h)` of the first monitor, or a 1280x800 fallback.
fn first_monitor() -> (i32, i32, i32, i32) {
    gtk::gdk::Display::default()
        .as_ref()
        .map(opusdm_ui::list_monitors)
        .and_then(|rects| rects.first().copied())
        .unwrap_or(FALLBACK_MONITOR)
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
