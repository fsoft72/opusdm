//! The plain window of one applet: undecorated, at the saved or the manifest
//! size, holding a content slot for the built widget tree and an error label
//! that shows the latest error. On X11 it gets the applet window hints and
//! its position when realized and mapped (see [`crate::x11_window`]).
//!
//! Geometry (Task 11): Openbox moves (Alt + left drag) and resizes (Alt +
//! right drag) the window itself, and GTK sees no button event of those
//! drags, so the window reads its geometry back from X11 when `is-active`
//! changes and every [`READBACK_INTERVAL_MS`] while it is active, and hands a
//! changed geometry to `on_moved` (the host saves it). The geometry the
//! window should have (the "target") is shared by the realize and map
//! handlers, the placement retries and the read-back, so a re-map puts the
//! window where the user left it, not where it opened. The target is
//! clamped to the monitors at open and by [`AppletWindow::reclamp`].
//!
//! Ownership: the host's applet entry owns the [`AppletWindow`]; dropping it
//! destroys the GTK window, so the application no longer keeps it either.
//! A window-manager close is refused here and handed to `on_close` (the
//! host disables the applet and drops the window itself), so the host stays
//! the only owner. The signal handlers are owned by the window; the
//! placement retries and the read-back timer hold only a weak reference to
//! it and log when it is gone; the timer is removed when the window stops
//! being active and when it is destroyed.

use crate::manifest::Manifest;
use crate::placement::{self, Rect};
use crate::state::AppletState;
use crate::x11_window;
use gtk4 as gtk;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Colour of the error text (red, as 16 bit pango channels).
const ERROR_RGB: (u16, u16, u16) = (0xd0d0, 0x1010, 0x1010);

/// Margin around the error label, in pixels.
const ERROR_MARGIN: i32 = 4;

/// Monitor rect used when GDK lists no monitor.
const FALLBACK_MONITOR: Rect = (0, 0, 1280, 800);

/// How often the geometry is read back while the window is active, in
/// milliseconds (catches a move or resize that does not change the focus).
pub const READBACK_INTERVAL_MS: u64 = 500;

/// What the window calls with its new geometry after a move or a resize.
type OnMoved = Rc<dyn Fn(Rect)>;

/// The window of one applet and the two places the host fills.
pub struct AppletWindow {
    id: String,
    window: gtk::ApplicationWindow,
    /// Holds the root of the built widget tree, or nothing.
    content: gtk::Box,
    /// The latest error; hidden while empty.
    error: gtk::Label,
    /// The geometry the window should have: the open geometry, then the one
    /// last read back or set by [`AppletWindow::reclamp`].
    target: Rc<Cell<Rect>>,
}

/// Build the window of applet `id` for `app`: title = manifest name, default
/// size = the saved `width` / `height`, else the manifest window size,
/// resizable as the manifest says. It is presented only when `present` is
/// true (tests build windows without ever showing them). A close request
/// (Alt+F4, the window manager) calls `on_close` and is otherwise refused:
/// `on_close` must hold only weak references. On X11 the window is placed at
/// the saved `x` / `y`, else at a default spot in the top right area of the
/// first monitor, cascaded by the number of windows `app` already has. The
/// size is never below the manifest `min_width` / `min_height` (also a GTK
/// size request, so the window manager does not resize it below), and the
/// geometry is clamped to the current monitors. A move or resize by the
/// window manager calls `on_moved` with the new geometry, at most once per
/// read-back and only when it changed; it must hold only weak references.
pub fn open(app: &gtk::Application, id: &str, manifest: &Manifest, state: &AppletState, present: bool, on_close: impl Fn() + 'static, on_moved: impl Fn(Rect) + 'static) -> AppletWindow {
    let min = (manifest.window.min_width, manifest.window.min_height);
    let size = (state.width.unwrap_or(manifest.window.width), state.height.unwrap_or(manifest.window.height));
    let monitors = current_monitors();
    let first = monitors.first().copied().unwrap_or(FALLBACK_MONITOR);
    let (x, y) = match (state.x, state.y) {
        (Some(x), Some(y)) => (x, y),
        _ => x11_window::default_position(first, size, app.windows().len()),
    };
    let rect = placement::clamp_to_monitors(placement::at_least((x, y, size.0, size.1), min), &monitors);
    opusdm_ipc::debug_log!(APP_TAG, "applet {id}: opens at {rect:?} (saved x {:?} y {:?} w {:?} h {:?}, monitors {monitors:?})", state.x, state.y, state.width, state.height);
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(&manifest.name)
        .default_width(rect.2)
        .default_height(rect.3)
        .resizable(manifest.window.resizable)
        .decorated(false)
        .build();
    window.set_size_request(min.0, min.1);
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
    let target = Rc::new(Cell::new(rect));
    // False from each map until its last placement retry: a read-back in
    // that time would see the window before the window manager placed it.
    let settled = Rc::new(Cell::new(false));
    connect_x11_placement(&window, target.clone(), settled.clone());
    connect_readback(&window, id, target.clone(), settled, Rc::new(on_moved));
    if present { window.present(); }
    AppletWindow { id: id.to_string(), window, content, error, target }
}

/// Apply the X11 applet hints at the current `target` geometry when
/// `window` is realized, again on map, and on the retries after map (the
/// window manager adopts the window shortly after map and drops state
/// messages sent before that). `settled` goes false at each map and true
/// after its last retry. A window never presented (tests) is never realized,
/// so nothing is sent. The handlers hold the shared target; the retries hold
/// a weak reference to the window and log if it is gone.
fn connect_x11_placement(window: &gtk::ApplicationWindow, target: Rc<Cell<Rect>>, settled: Rc<Cell<bool>>) {
    let realize_target = target.clone();
    window.connect_realize(move |win| _prepare_at(win, realize_target.get()));
    window.connect_map(move |win| {
        settled.set(false);
        _prepare_at(win, target.get());
        let weak = win.downgrade();
        let (target, settled) = (target.clone(), settled.clone());
        let calls = Cell::new(0usize);
        opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || {
            calls.set(calls.get() + 1);
            let Some(win) = weak.upgrade() else {
                opusdm_ipc::err_log!(APP_TAG, "applet window gone before an X11 placement retry (closed right after it opened?)");
                return;
            };
            _prepare_at(&win, target.get());
            if calls.get() == opusdm_ui::PLACEMENT_RETRY_MS.len() { settled.set(true); }
        });
    });
}

/// [`x11_window::prepare`] at the geometry `rect`.
fn _prepare_at(window: &gtk::ApplicationWindow, rect: Rect) {
    let (x, y, w, h) = rect;
    x11_window::prepare(window, x, y, w, h);
}

/// Read the geometry of `window` back when `is-active` changes (Openbox
/// takes the focus away at the start of its move or resize grab and gives
/// it back at the end) and every [`READBACK_INTERVAL_MS`] while it is
/// active. The timer holds a weak reference to the window; it is removed
/// when the window stops being active and when it is destroyed.
fn connect_readback(window: &gtk::ApplicationWindow, id: &str, target: Rc<Cell<Rect>>, settled: Rc<Cell<bool>>, on_moved: OnMoved) {
    let id = id.to_string();
    let check: Rc<dyn Fn(&gtk::ApplicationWindow)> = Rc::new(move |win| _read_back(win, &id, &target, &settled, &on_moved));
    let tick: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let active_tick = tick.clone();
    window.connect_is_active_notify(move |win| {
        check(win);
        _stop_tick(&active_tick);
        if win.is_active() { _start_tick(win, &active_tick, check.clone()); }
    });
    window.connect_destroy(move |_| _stop_tick(&tick));
}

/// Start the read-back timer of the active `window` into `tick`. It stops
/// itself (and clears `tick`) when the window is gone (logged) or no longer
/// active.
fn _start_tick(window: &gtk::ApplicationWindow, tick: &Rc<RefCell<Option<glib::SourceId>>>, check: Rc<dyn Fn(&gtk::ApplicationWindow)>) {
    let weak = window.downgrade();
    let slot = tick.clone();
    let id = glib::timeout_add_local(Duration::from_millis(READBACK_INTERVAL_MS), move || {
        let Some(win) = weak.upgrade() else {
            opusdm_ipc::err_log!(APP_TAG, "applet window gone while its geometry read-back timer ran; timer stopped");
            slot.borrow_mut().take();
            return glib::ControlFlow::Break;
        };
        if !win.is_active() {
            slot.borrow_mut().take();
            return glib::ControlFlow::Break;
        }
        check(&win);
        glib::ControlFlow::Continue
    });
    *tick.borrow_mut() = Some(id);
}

/// Remove the read-back timer in `tick`, if one runs.
fn _stop_tick(tick: &Rc<RefCell<Option<glib::SourceId>>>) {
    if let Some(id) = tick.borrow_mut().take() { id.remove(); }
}

/// Read the geometry of `window` (applet `id`) from X11 and, when it
/// differs from `target`, make it the target and call `on_moved`. Skipped
/// (logged at debug level) while the window is not realized or its
/// placement has not settled after a map.
fn _read_back(window: &gtk::ApplicationWindow, id: &str, target: &Cell<Rect>, settled: &Cell<bool>, on_moved: &OnMoved) {
    if !window.is_realized() { opusdm_ipc::debug_log!(APP_TAG, "applet {id}: window not realized, geometry not read back"); return; }
    if !settled.get() { opusdm_ipc::debug_log!(APP_TAG, "applet {id}: placement not settled after map yet, geometry not read back"); return; }
    // `None` is logged by `current_geometry`.
    let Some(now) = x11_window::current_geometry(window) else { return };
    let before = target.get();
    if now == before { return; }
    opusdm_ipc::debug_log!(APP_TAG, "applet {id}: moved or resized by the window manager from {before:?} to {now:?}");
    target.set(now);
    on_moved(now);
}

/// The current monitor rects of the default display (none without one).
fn current_monitors() -> Vec<Rect> {
    gtk::gdk::Display::default().as_ref().map(opusdm_ui::list_monitors).unwrap_or_default()
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

    /// The geometry the window should have now (see the module doc).
    pub fn target(&self) -> Rect {
        self.target.get()
    }

    /// Clamp the target geometry to `monitors` (the monitors changed) and,
    /// when that changed it, move a realized window there. Returns the new
    /// target when it changed. The saved geometry is not touched: when the
    /// monitor comes back, the next start opens the applet where the user
    /// left it.
    pub fn reclamp(&self, monitors: &[Rect]) -> Option<Rect> {
        let before = self.target.get();
        let after = placement::clamp_to_monitors(before, monitors);
        if after == before { return None; }
        self.target.set(after);
        opusdm_ipc::debug_log!(APP_TAG, "applet {}: clamped from {before:?} to {after:?} for the monitors {monitors:?}", self.id);
        if self.window.is_realized() { _prepare_at(&self.window, after); }
        Some(after)
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
