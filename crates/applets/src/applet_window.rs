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
//!
//! Edit mode (Task 12): the content sits in a `gtk::Overlay` whose overlay
//! children are a dashed frame (takes no input) and, top right, a small box
//! with a drag handle and a settings gear. Outside edit mode both are hidden
//! (not drawn, not clickable) and the outer overlay has no
//! [`EDIT_CLASS`]. The window subscribes one listener to the shared
//! [`EditMode`] when it opens (holding only weak references to its widgets,
//! logged when gone) and unsubscribes it when dropped. A press on the handle
//! (no Alt, so GTK gets it) drags the window: the pointer is read in root
//! coordinates from X11 (the window moves under it), the window is moved
//! with `pin_geometry` and the target follows on every step, the read-back
//! is paused, and the geometry is handed to `on_moved` once, at the end.
//! The handle's gesture holds only a `Weak` of the drag state, owned by the
//! [`AppletWindow`]; a failed upgrade is logged.

use crate::edit_mode::{EditMode, ListenerId};
use crate::manifest::Manifest;
use crate::placement::{self, Rect};
use crate::state::AppletState;
use crate::x11_window;
use gtk4 as gtk;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
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

/// CSS class of the outer widget (the overlay) while edit mode is on.
pub const EDIT_CLASS: &str = "opusdm-applet-edit";

/// CSS class of the dashed frame drawn over the content in edit mode.
const FRAME_CLASS: &str = "opusdm-applet-edit-frame";

/// CSS class of the box with the handle and the gear.
const ICONS_CLASS: &str = "opusdm-applet-edit-icons";

/// The stylesheet of the edit widgets (see [`install_css`]).
const EDIT_CSS: &str = include_str!("applet_window.css");

/// Size of the handle and gear icons, in pixels.
const EDIT_ICON_SIZE: i32 = 16;

/// Icon names tried for the drag handle, first found wins.
const HANDLE_ICONS: [&str; 2] = ["list-drag-handle-symbolic", "object-move-symbolic"];

/// Icon names tried for the settings gear, first found wins.
const GEAR_ICONS: [&str; 2] = ["emblem-system-symbolic", "preferences-system-symbolic"];

/// Label of the handle when the icon theme has none of its icons.
const HANDLE_FALLBACK: &str = "Move";

/// Label of the gear when the icon theme has none of its icons.
const GEAR_FALLBACK: &str = "Set";

/// Tooltip of the drag handle.
const HANDLE_TOOLTIP: &str = "Move";

/// Tooltip of the settings gear.
const GEAR_TOOLTIP: &str = "Settings";

/// What the window calls with its new geometry after a move or a resize.
type OnMoved = Rc<dyn Fn(Rect)>;

/// What the host gives a window to call back. Every hook must hold only
/// weak references.
pub struct WindowHooks {
    /// A window-manager close request (Alt+F4); the close itself is refused.
    pub on_close: Box<dyn Fn()>,
    /// The new geometry after a move or resize by the window manager or a
    /// drag of the edit handle.
    pub on_moved: Rc<dyn Fn(Rect)>,
    /// A click on the edit gear (only while edit mode is on).
    pub on_settings: Rc<dyn Fn()>,
}

/// The window of one applet and the two places the host fills.
pub struct AppletWindow {
    id: String,
    window: gtk::ApplicationWindow,
    /// Holds the root of the built widget tree, or nothing.
    content: gtk::Box,
    /// The latest error; hidden while empty.
    error: gtk::Label,
    /// The geometry the window should have: the open geometry, then the one
    /// last read back or set by [`AppletWindow::reclamp`] or a handle drag.
    target: Rc<Cell<Rect>>,
    /// The overlays shown in edit mode.
    edit_widgets: EditWidgets,
    /// The handle and the gear (inside `edit_widgets.icons`).
    handle: gtk::Button,
    gear: gtk::Button,
    /// The shared edit mode and this window's listener on it.
    edit: EditMode,
    edit_listener: ListenerId,
    /// The handle drag; the gesture holds only a `Weak` of it.
    drag: Rc<Drag>,
}

/// The widgets edit mode shows and hides.
struct EditWidgets {
    /// The outer widget of the window; has [`EDIT_CLASS`] in edit mode.
    outer: gtk::Overlay,
    /// The dashed border, drawn over the content, never takes input.
    frame: gtk::Box,
    /// The handle and the gear, top right.
    icons: gtk::Box,
}

/// Where a handle drag started.
struct DragStart {
    rect: Rect,
    pointer: (i32, i32),
    monitors: Vec<Rect>,
}

/// The state of a handle drag of one window (see the module doc).
struct Drag {
    id: String,
    window: gtk::ApplicationWindow,
    target: Rc<Cell<Rect>>,
    /// True during a drag: the read-back is paused.
    dragging: Rc<Cell<bool>>,
    start: RefCell<Option<DragStart>>,
    on_moved: OnMoved,
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
/// read-back and only when it changed, and a handle drag calls it once at
/// its end. The window follows `edit` (on at open when `edit` is on); the
/// gear calls `on_settings` while edit mode is on. Every hook must hold only
/// weak references.
pub fn open(app: &gtk::Application, id: &str, manifest: &Manifest, state: &AppletState, present: bool, edit: &EditMode, hooks: WindowHooks) -> AppletWindow {
    let WindowHooks { on_close, on_moved, on_settings } = hooks;
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
    install_css(&WidgetExt::display(&window));
    let (handle, gear) = (edit_button(&HANDLE_ICONS, HANDLE_FALLBACK, HANDLE_TOOLTIP), edit_button(&GEAR_ICONS, GEAR_FALLBACK, GEAR_TOOLTIP));
    let edit_widgets = edit_overlay(&column, &handle, &gear);
    window.set_child(Some(&edit_widgets.outer));
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
    let dragging = Rc::new(Cell::new(false));
    connect_x11_placement(&window, target.clone(), settled.clone());
    connect_readback(&window, id, target.clone(), settled, dragging.clone(), on_moved.clone());
    let drag = Rc::new(Drag { id: id.to_string(), window: window.clone(), target: target.clone(), dragging, start: RefCell::new(None), on_moved });
    connect_handle_drag(&handle, &drag);
    connect_gear(&gear, id, edit.clone(), on_settings);
    edit_widgets.apply(edit.get());
    let edit_listener = subscribe_edit(edit, id, &edit_widgets);
    if present { window.present(); }
    AppletWindow { id: id.to_string(), window, content, error, target, edit_widgets, handle, gear, edit: edit.clone(), edit_listener, drag }
}

/// Load [`EDIT_CSS`] for `display`, once per display, at APPLICATION
/// priority. A parse error is logged.
fn install_css(display: &gtk::gdk::Display) {
    thread_local! {
        /// The displays that already have the edit stylesheet.
        static CSS_DISPLAYS: RefCell<Vec<gtk::gdk::Display>> = const { RefCell::new(Vec::new()) };
    }
    CSS_DISPLAYS.with(|displays| {
        if displays.borrow().contains(display) { return; }
        let provider = gtk::CssProvider::new();
        provider.connect_parsing_error(|_, section, error| opusdm_ipc::err_log!(APP_TAG, "applet_window.css: {} at {}", error.message(), section.to_str()));
        provider.load_from_data(EDIT_CSS);
        gtk::style_context_add_provider_for_display(display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        displays.borrow_mut().push(display.clone());
    });
}

/// A flat button with the first icon of `candidates` the icon theme has
/// ([`EDIT_ICON_SIZE`] px), else the text `fallback`, and the tooltip
/// `tooltip`.
fn edit_button(candidates: &[&'static str], fallback: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.set_focus_on_click(false);
    button.set_tooltip_text(Some(tooltip));
    match _first_icon(candidates) {
        Some(name) => {
            let image = gtk::Image::from_icon_name(name);
            image.set_pixel_size(EDIT_ICON_SIZE);
            button.set_child(Some(&image));
        }
        None => {
            opusdm_ipc::debug_log!(APP_TAG, "the icon theme has none of {candidates:?}: the edit button shows \"{fallback}\"");
            button.set_label(fallback);
        }
    }
    button
}

/// The first of `candidates` the icon theme of the default display has.
fn _first_icon(candidates: &[&'static str]) -> Option<&'static str> {
    let display = gtk::gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    candidates.iter().copied().find(|name| theme.has_icon(name))
}

/// The overlay around `content` with the dashed frame and, top right, the
/// box holding `handle` and `gear` (both hidden until edit mode is applied).
fn edit_overlay(content: &gtk::Box, handle: &gtk::Button, gear: &gtk::Button) -> EditWidgets {
    let outer = gtk::Overlay::new();
    outer.set_child(Some(content));
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class(FRAME_CLASS);
    frame.set_can_target(false);
    frame.set_halign(gtk::Align::Fill);
    frame.set_valign(gtk::Align::Fill);
    frame.set_visible(false);
    outer.add_overlay(&frame);
    let icons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    icons.add_css_class(ICONS_CLASS);
    icons.set_halign(gtk::Align::End);
    icons.set_valign(gtk::Align::Start);
    icons.append(handle);
    icons.append(gear);
    icons.set_visible(false);
    outer.add_overlay(&icons);
    EditWidgets { outer, frame, icons }
}

impl EditWidgets {
    /// Show (`on`) or hide the frame and the icons, and set or remove
    /// [`EDIT_CLASS`] on the outer widget.
    fn apply(&self, on: bool) {
        _apply_edit(&self.outer, &self.frame, &self.icons, on);
    }
}

/// See [`EditWidgets::apply`].
fn _apply_edit(outer: &gtk::Overlay, frame: &gtk::Box, icons: &gtk::Box, on: bool) {
    frame.set_visible(on);
    icons.set_visible(on);
    if on { outer.add_css_class(EDIT_CLASS); } else { outer.remove_css_class(EDIT_CLASS); }
}

/// Subscribe the edit widgets of applet `id` to `edit`. The listener holds
/// only weak references to the widgets; when they are gone it logs (the
/// window should have unsubscribed when it was dropped).
fn subscribe_edit(edit: &EditMode, id: &str, widgets: &EditWidgets) -> ListenerId {
    let (outer, frame, icons) = (widgets.outer.downgrade(), widgets.frame.downgrade(), widgets.icons.downgrade());
    let id = id.to_string();
    edit.subscribe(move |on| {
        let (Some(outer), Some(frame), Some(icons)) = (outer.upgrade(), frame.upgrade(), icons.upgrade()) else {
            opusdm_ipc::err_log!(APP_TAG, "applet {id}: edit mode {on}, but the window widgets are gone (listener not unsubscribed?)");
            return;
        };
        _apply_edit(&outer, &frame, &icons, on);
    })
}

/// A click on `gear` calls `on_settings` while `edit` is on; outside edit
/// mode (the gear is hidden, but `clicked` can still be emitted) it is
/// logged at debug level and ignored.
fn connect_gear(gear: &gtk::Button, id: &str, edit: EditMode, on_settings: Rc<dyn Fn()>) {
    let id = id.to_string();
    gear.connect_clicked(move |_| {
        if !edit.get() { opusdm_ipc::debug_log!(APP_TAG, "applet {id}: gear activated outside edit mode, ignored"); return; }
        on_settings();
    });
}

/// A left-button drag on `handle` moves the window (see [`Drag`]). The
/// gesture runs in the capture phase and claims the press, so the button
/// itself does not see a click. Its closures hold only a `Weak<Drag>`; a
/// failed upgrade is logged, a failed pointer query is logged by
/// [`x11_window::pointer_position`].
fn connect_handle_drag(handle: &gtk::Button, drag: &Rc<Drag>) {
    let gesture = gtk::GestureDrag::new();
    gesture.set_button(gtk::gdk::BUTTON_PRIMARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(drag);
    let id = drag.id.clone();
    gesture.connect_drag_begin(move |gesture, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        let Some(drag) = _upgrade_drag(&weak, &id, "begin") else { return };
        let Some(pointer) = x11_window::pointer_position() else { return };
        drag.begin(pointer, &current_monitors());
    });
    let weak = Rc::downgrade(drag);
    let id = drag.id.clone();
    gesture.connect_drag_update(move |_, _, _| {
        let Some(drag) = _upgrade_drag(&weak, &id, "step") else { return };
        let Some(pointer) = x11_window::pointer_position() else { return };
        drag.step(pointer);
    });
    let weak = Rc::downgrade(drag);
    let id = drag.id.clone();
    gesture.connect_drag_end(move |_, _, _| {
        let Some(drag) = _upgrade_drag(&weak, &id, "end") else { return };
        if let Some(pointer) = x11_window::pointer_position() { drag.step(pointer); }
        drag.end();
    });
    handle.add_controller(gesture);
}

/// Upgrade the drag state of applet `id` for the gesture phase `phase`;
/// logs when it is gone.
fn _upgrade_drag(weak: &Weak<Drag>, id: &str, phase: &str) -> Option<Rc<Drag>> {
    let drag = weak.upgrade();
    if drag.is_none() { opusdm_ipc::err_log!(APP_TAG, "applet {id}: handle drag {phase} after the window was dropped, ignored"); }
    drag
}

impl Drag {
    /// Start a drag at the root `pointer`, from the current target, kept on
    /// `monitors`. A drag already running restarts from here.
    fn begin(&self, pointer: (i32, i32), monitors: &[Rect]) {
        let rect = self.target.get();
        opusdm_ipc::debug_log!(APP_TAG, "applet {}: handle drag from {rect:?}, pointer {pointer:?}", self.id);
        *self.start.borrow_mut() = Some(DragStart { rect, pointer, monitors: monitors.to_vec() });
        self.dragging.set(true);
    }

    /// Move the window to follow the root `pointer`: the new target, also
    /// sent to X11 when the window is realized. `None` (logged at debug
    /// level) without a drag in progress.
    fn step(&self, pointer: (i32, i32)) -> Option<Rect> {
        let rect = {
            let start = self.start.borrow();
            let Some(start) = start.as_ref() else { opusdm_ipc::debug_log!(APP_TAG, "applet {}: handle drag step without a begin, ignored", self.id); return None };
            placement::drag_on_monitors(start.rect, start.pointer, pointer, &start.monitors)
        };
        if rect == self.target.get() { return Some(rect); }
        self.target.set(rect);
        if self.window.is_realized() {
            let (x, y, w, h) = rect;
            x11_window::move_to(&self.window, x, y, w, h);
        }
        Some(rect)
    }

    /// End the drag: hand the final target to `on_moved` once when it moved,
    /// and resume the read-back. `None` when no drag was in progress or the
    /// window did not move.
    fn end(&self) -> Option<Rect> {
        self.dragging.set(false);
        let start = self.start.borrow_mut().take()?;
        let rect = self.target.get();
        if rect == start.rect { opusdm_ipc::debug_log!(APP_TAG, "applet {}: handle pressed without a move, nothing saved", self.id); return None; }
        opusdm_ipc::debug_log!(APP_TAG, "applet {}: handle drag from {:?} to {rect:?}", self.id, start.rect);
        (self.on_moved)(rect);
        Some(rect)
    }
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
fn connect_readback(window: &gtk::ApplicationWindow, id: &str, target: Rc<Cell<Rect>>, settled: Rc<Cell<bool>>, dragging: Rc<Cell<bool>>, on_moved: OnMoved) {
    let id = id.to_string();
    let check: Rc<dyn Fn(&gtk::ApplicationWindow)> = Rc::new(move |win| _read_back(win, &id, &target, &settled, &dragging, &on_moved));
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
/// placement has not settled after a map, and during a handle drag (the
/// drag owns the target then; the window manager may lag behind it).
fn _read_back(window: &gtk::ApplicationWindow, id: &str, target: &Cell<Rect>, settled: &Cell<bool>, dragging: &Cell<bool>, on_moved: &OnMoved) {
    if !window.is_realized() { opusdm_ipc::debug_log!(APP_TAG, "applet {id}: window not realized, geometry not read back"); return; }
    if dragging.get() { opusdm_ipc::debug_log!(APP_TAG, "applet {id}: handle drag in progress, geometry not read back"); return; }
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

    /// The outer widget (an overlay); it has [`EDIT_CLASS`] in edit mode.
    pub fn edit_outer(&self) -> &gtk::Overlay {
        &self.edit_widgets.outer
    }

    /// The dashed frame shown in edit mode.
    pub fn edit_frame(&self) -> &gtk::Box {
        &self.edit_widgets.frame
    }

    /// The box with the handle and the gear, shown in edit mode.
    pub fn edit_icons(&self) -> &gtk::Box {
        &self.edit_widgets.icons
    }

    /// The drag handle.
    pub fn handle(&self) -> &gtk::Button {
        &self.handle
    }

    /// The settings gear.
    pub fn gear(&self) -> &gtk::Button {
        &self.gear
    }

    /// Start a handle drag at the root `pointer`, kept on `monitors` (what
    /// the handle's gesture calls on press).
    pub fn begin_drag(&self, pointer: (i32, i32), monitors: &[Rect]) {
        self.drag.begin(pointer, monitors);
    }

    /// One step of a handle drag to the root `pointer`: the new target, or
    /// `None` without a drag in progress.
    pub fn drag_to(&self, pointer: (i32, i32)) -> Option<Rect> {
        self.drag.step(pointer)
    }

    /// End a handle drag: the geometry handed to `on_moved` (once), or
    /// `None` when there was no drag or no move.
    pub fn end_drag(&self) -> Option<Rect> {
        self.drag.end()
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
    /// Unsubscribe the edit listener, then destroy the GTK window: the
    /// application drops it from its list.
    fn drop(&mut self) {
        if !self.edit.unsubscribe(self.edit_listener) { opusdm_ipc::err_log!(APP_TAG, "applet {}: its edit mode listener was already gone", self.id); }
        opusdm_ipc::debug_log!(APP_TAG, "applet {}: window destroyed", self.id);
        self.window.destroy();
    }
}
