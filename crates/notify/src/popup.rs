//! The popups: one small undecorated window per notification, stacked from a
//! corner of the monitor under the pointer. At most [`MAX_VISIBLE`] are on
//! screen; the others wait for room. Critical ones never wait.
//!
//! The popups only report what the user did ([`PopupEvent`]); `main` decides
//! what it means for the store and the signals, then calls [`Popups::hide`].

use crate::markup;
use crate::model::{Image, Notification, Urgency};
use crate::placement::{self, Corner, Rect, MAX_VISIBLE, POPUP_WIDTH};
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

/// Size of the picture in pixels.
const ICON_SIZE: i32 = 48;
/// Action buttons shown at most.
const MAX_BUTTONS: usize = 3;
/// Lines of body text before the ellipsis.
const BODY_LINES: i32 = 4;
/// Icon shown when the notification names none.
const FALLBACK_ICON: &str = "dialog-information";
/// Space between the edge of a popup and its content: top, bottom, start, end.
const PADDING: (i32, i32, i32, i32) = (10, 10, 10, 8);
/// Fade-in: steps and time between them.
const FADE_STEPS: u32 = 8;
const FADE_STEP_MS: u64 = 18;
/// Stand-in monitor when nothing better is known.
const FALLBACK_AREA: Rect = (0, 0, 1280, 800);

/// Style of the popups.
pub const NOTIFY_CSS: &str = ".opusdm-notify { border: 1px solid alpha(currentColor, 0.25); border-radius: 6px; } \
     .opusdm-notify.critical { border: 2px solid #e5484d; } \
     .opusdm-notify-app, .opusdm-notify-time { font-size: 85%; opacity: 0.7; } \
     .opusdm-notify-summary { font-weight: bold; } \
     .opusdm-notify-close { min-width: 0; min-height: 0; padding: 0 4px; }";

/// What the user did with a popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupEvent {
    /// Its time ran out.
    Expired(u32),
    /// Closed with its button or a right click.
    Dismissed(u32),
    /// Clicked (the default action) or one of its buttons.
    Action { id: u32, key: String },
}

/// The widgets of one popup that change when the notification is replaced.
struct Popup {
    id: u32,
    window: gtk::Window,
    /// Running timer, cancelled while the pointer is over the popup.
    timer: Rc<RefCell<Option<glib::SourceId>>>,
    /// Popup time; `None` for ever.
    expire: Rc<Cell<Option<u32>>>,
    /// Whether the pointer is over the popup right now.
    hovered: Rc<Cell<bool>>,
}

impl Popup {
    /// Stop the timer.
    fn stop_timer(&self) {
        stop(&self.timer);
    }
}

/// Every popup on screen and the ones waiting.
pub struct Popups {
    app: gtk::Application,
    corner: Corner,
    /// Newest first: the first one sits in the corner.
    shown: RefCell<Vec<Popup>>,
    /// Oldest first.
    waiting: RefCell<VecDeque<(Notification, Option<u32>)>>,
    /// X11: the usable area the stack was started on.
    area: Cell<Option<Rect>>,
    on_event: Rc<dyn Fn(PopupEvent)>,
    /// Weak link to itself for the callbacks of the widgets.
    me: RefCell<std::rc::Weak<Popups>>,
}

impl Popups {
    /// No popup yet. `on_event` hears what the user does.
    pub fn new(app: &gtk::Application, corner: Corner, on_event: impl Fn(PopupEvent) + 'static) -> Rc<Self> {
        let popups = Rc::new(Popups {
            app: app.clone(),
            corner,
            shown: RefCell::new(Vec::new()),
            waiting: RefCell::new(VecDeque::new()),
            area: Cell::new(None),
            on_event: Rc::new(on_event),
            me: RefCell::new(std::rc::Weak::new()),
        });
        *popups.me.borrow_mut() = Rc::downgrade(&popups);
        popups
    }

    /// Show `n` for `expire` milliseconds (`None`: until closed), or update
    /// its popup when it is already up or waiting.
    pub fn show(&self, n: Notification, expire: Option<u32>) {
        let up = self.shown.borrow().iter().position(|p| p.id == n.id);
        if let Some(i) = up {
            let (window, expire_cell, timer, hovered) = {
                let shown = self.shown.borrow();
                (shown[i].window.clone(), shown[i].expire.clone(), shown[i].timer.clone(), shown[i].hovered.get())
            };
            expire_cell.set(expire);
            window.set_child(Some(&self.content(&n)));
            set_critical_class(&window, n.urgency);
            // While hovered the timer stays stopped; leaving starts it with
            // the new time.
            self.start_timer(n.id, &timer, timer_time(hovered, expire));
            self.relayout();
            return;
        }
        if let Some(slot) = self.waiting.borrow_mut().iter_mut().find(|(w, _)| w.id == n.id) {
            *slot = (n, expire);
            return;
        }
        let room = self.shown.borrow().len() < MAX_VISIBLE;
        if room || n.urgency == Urgency::Critical {
            self.open(n, expire);
        } else {
            self.waiting.borrow_mut().push_back((n, expire));
        }
    }

    /// Take the popup of `id` away (shown or waiting), and let a waiting one in.
    pub fn hide(&self, id: u32) {
        self.waiting.borrow_mut().retain(|(n, _)| n.id != id);
        let removed = {
            let mut shown = self.shown.borrow_mut();
            shown.iter().position(|p| p.id == id).map(|i| shown.remove(i))
        };
        let Some(popup) = removed else { return };
        popup.stop_timer();
        popup.window.destroy();
        while self.shown.borrow().len() < MAX_VISIBLE {
            let Some((n, expire)) = self.waiting.borrow_mut().pop_front() else { break };
            self.open(n, expire);
        }
        if self.shown.borrow().is_empty() {
            self.area.set(None);
        }
        self.relayout();
    }

    /// Build, place and show a new popup.
    fn open(&self, n: Notification, expire: Option<u32>) {
        if self.shown.borrow().is_empty() {
            self.area.set(None);
        }
        let window = gtk::Window::builder()
            .application(&self.app)
            .title("OpusDM Notification")
            .decorated(false)
            .resizable(false)
            .default_width(POPUP_WIDTH)
            .build();
        window.add_css_class("opusdm-notify");
        set_critical_class(&window, n.urgency);
        window.set_child(Some(&self.content(&n)));
        let wayland = opusdm_ui::layer::init(&window, &self.layer_spec(placement::MARGIN));
        if !wayland {
            window.connect_realize(set_x11_type);
            window.connect_map(add_x11_states);
        }
        let popup = Popup {
            id: n.id,
            window: window.clone(),
            timer: Rc::new(RefCell::new(None)),
            expire: Rc::new(Cell::new(expire)),
            hovered: Rc::new(Cell::new(false)),
        };
        self.watch_hover(&popup);
        self.start_timer(n.id, &popup.timer, expire);
        self.shown.borrow_mut().insert(0, popup);
        window.set_opacity(0.0);
        window.present();
        fade_in(&window);
        self.relayout();
        if !wayland {
            // The window manager places a new window after map; put it back.
            let me = self.me.borrow().clone();
            opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || {
                if let Some(me) = me.upgrade() { me.relayout(); }
            });
        }
    }

    /// The inside of a popup: picture, texts, buttons.
    fn content(&self, n: &Notification) -> gtk::Widget {
        let id = n.id;
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let (top, bottom, start, end) = PADDING;
        root.set_margin_top(top);
        root.set_margin_bottom(bottom);
        root.set_margin_start(start);
        root.set_margin_end(end);
        // The window is exactly POPUP_WIDTH wide, margins included.
        root.set_size_request(POPUP_WIDTH - start - end, -1);

        let image = picture(n);
        image.set_valign(gtk::Align::Start);
        root.append(&image);

        let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
        texts.set_hexpand(true);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let app = label(&n.app_name, "opusdm-notify-app");
        app.set_hexpand(true);
        header.append(&app);
        let time = label(&clock_time(n.time), "opusdm-notify-time");
        // Only the app name stretches: the time stays by the close button.
        time.set_hexpand(false);
        time.set_max_width_chars(-1);
        header.append(&time);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.add_css_class("opusdm-notify-close");
        close.set_tooltip_text(Some("Dismiss"));
        let on_event = self.on_event.clone();
        close.connect_clicked(move |_| on_event(PopupEvent::Dismissed(id)));
        header.append(&close);
        texts.append(&header);

        if !n.summary.is_empty() {
            let summary = label(&n.summary, "opusdm-notify-summary");
            summary.set_wrap(true);
            summary.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            texts.append(&summary);
        }
        if !n.body.is_empty() {
            texts.append(&body_label(&n.body));
        }
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_homogeneous(true);
        buttons.set_margin_top(4);
        for action in n.button_actions().take(MAX_BUTTONS) {
            let button = gtk::Button::with_label(&action.label);
            let (on_event, key) = (self.on_event.clone(), action.key.clone());
            button.connect_clicked(move |_| on_event(PopupEvent::Action { id, key: key.clone() }));
            buttons.append(&button);
        }
        if buttons.first_child().is_some() {
            texts.append(&buttons);
        }
        root.append(&texts);

        // Left click on the popup runs the default action (or just closes
        // it), right click dismisses it. Clicks on buttons stay theirs.
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let on_event = self.on_event.clone();
        let default = n.default_action().map(|a| a.key.clone());
        click.connect_released(move |gesture, _, _, _| {
            match (gesture.current_button(), &default) {
                (1, Some(key)) => on_event(PopupEvent::Action { id, key: key.clone() }),
                (1, None) | (3, _) => on_event(PopupEvent::Dismissed(id)),
                _ => {}
            }
        });
        root.add_controller(click);
        root.upcast()
    }

    /// Pause the timer while the pointer is over the popup.
    fn watch_hover(&self, popup: &Popup) {
        let motion = gtk::EventControllerMotion::new();
        let (timer, hovered) = (popup.timer.clone(), popup.hovered.clone());
        motion.connect_enter(move |_, _, _| {
            hovered.set(true);
            stop(&timer);
        });
        let (me, timer, expire, id) = (self.me.borrow().clone(), popup.timer.clone(), popup.expire.clone(), popup.id);
        let hovered = popup.hovered.clone();
        motion.connect_leave(move |_| {
            hovered.set(false);
            if let Some(me) = me.upgrade() { me.start_timer(id, &timer, expire.get()); }
        });
        popup.window.add_controller(motion);
    }

    /// (Re)start the timer of popup `id`.
    fn start_timer(&self, id: u32, timer: &Rc<RefCell<Option<glib::SourceId>>>, expire: Option<u32>) {
        stop(timer);
        let Some(ms) = expire else { return };
        let (on_event, slot) = (self.on_event.clone(), timer.clone());
        let source = glib::timeout_add_local_once(Duration::from_millis(u64::from(ms)), move || {
            slot.borrow_mut().take();
            on_event(PopupEvent::Expired(id));
        });
        timer.replace(Some(source));
    }

    /// Put every popup in its place in the stack.
    fn relayout(&self) {
        let shown = self.shown.borrow();
        let heights: Vec<i32> = shown.iter().map(|p| natural_height(&p.window)).collect();
        let offsets = placement::offsets(&heights);
        let area = if shown.iter().any(|p| !opusdm_ui::layer::is_layer(&p.window)) { Some(self.x11_area()) } else { None };
        for ((popup, height), offset) in shown.iter().zip(&heights).zip(&offsets) {
            if opusdm_ui::layer::is_layer(&popup.window) {
                opusdm_ui::layer::update(&popup.window, &self.layer_spec(*offset));
                continue;
            }
            let Some(area) = area else { continue };
            let (x, y) = placement::position(area, self.corner, POPUP_WIDTH, *height, *offset);
            place_x11(&popup.window, x, y, POPUP_WIDTH, *height);
        }
    }

    /// Wayland: a layer surface in the overlay layer, in the corner, `offset`
    /// pixels from its horizontal edge; never takes the keyboard. The
    /// compositor picks the monitor (the one in use).
    fn layer_spec(&self, offset: i32) -> opusdm_ui::layer::LayerSpec {
        use opusdm_ui::layer::{Anchors, KeyboardMode, Layer, LayerSpec, Margins};
        let corner = self.corner;
        let anchors = Anchors { top: corner.is_top(), bottom: !corner.is_top(), right: corner.is_right(), left: !corner.is_right() };
        let mut margins = Margins::default();
        if corner.is_top() { margins.top = offset } else { margins.bottom = offset }
        if corner.is_right() { margins.right = placement::MARGIN } else { margins.left = placement::MARGIN }
        LayerSpec::new(Layer::Overlay, "opusdm-notify").anchors(anchors).margins(margins).keyboard(KeyboardMode::None)
    }

    /// X11: the usable part of the monitor under the pointer, read when the
    /// stack starts and kept while it is up so the popups do not jump.
    fn x11_area(&self) -> Rect {
        if let Some(area) = self.area.get() { return area; }
        let area = read_x11_area().unwrap_or_else(|e| {
            opusdm_ipc::err_log!("opusdm-notify", "cannot read the work area: {e}");
            first_monitor()
        });
        self.area.set(Some(area));
        area
    }
}

/// How long a popup's timer should run from now: never while the pointer is
/// over it (`hovered`), else its `expire` time.
fn timer_time(hovered: bool, expire: Option<u32>) -> Option<u32> {
    if hovered { return None; }
    expire
}

/// Cancel the timer in `timer`, if any.
fn stop(timer: &Rc<RefCell<Option<glib::SourceId>>>) {
    if let Some(source) = timer.borrow_mut().take() {
        source.remove();
    }
}

/// Mark the window critical (red border) or not.
fn set_critical_class(window: &gtk::Window, urgency: Urgency) {
    if urgency == Urgency::Critical { window.add_css_class("critical") } else { window.remove_css_class("critical") }
}

/// A left aligned label with `class`, ellipsized at the end.
fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class(class);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    narrow(&label);
    label
}

/// Let the label take the width it is given instead of asking for the width
/// of its whole text, so the popup stays [`POPUP_WIDTH`] wide.
fn narrow(label: &gtk::Label) {
    label.set_max_width_chars(1);
    label.set_hexpand(true);
}

/// The body: whitelisted markup, wrapped, at most [`BODY_LINES`] lines.
/// Text that Pango still refuses is shown plain.
fn body_label(body: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_lines(BODY_LINES);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    narrow(&label);
    let markup = markup::sanitize(body);
    // Links are GtkLabel markup, not Pango's: check the rest with Pango.
    if gtk::pango::parse_markup(&markup::links_as_underline(&markup), '\0').is_ok() {
        label.set_markup(&markup);
    } else {
        label.set_text(&markup::plain_text(body));
    }
    // Links open with the desktop's handler; nothing else is clickable.
    label.connect_activate_link(|_, uri| {
        if let Err(e) = gtk::gio::AppInfo::launch_default_for_uri(uri, None::<&gtk::gio::AppLaunchContext>) {
            opusdm_ipc::err_log!("opusdm-notify", "cannot open {uri}: {e}");
        }
        glib::Propagation::Stop
    });
    label
}

/// `HH:MM` of `time` (seconds since the epoch), local time.
fn clock_time(time: i64) -> String {
    glib::DateTime::from_unix_local(time).and_then(|t| t.format("%H:%M")).map(|s| s.to_string()).unwrap_or_default()
}

/// The picture of `n`: its pixels, its image or icon, the icon of its
/// application, else a generic one.
fn picture(n: &Notification) -> gtk::Image {
    let image = match &n.image {
        Some(Image::Pixels(p)) => texture_image(p),
        Some(Image::Named(name)) => opusdm_apps::app_image(name),
        None => None,
    };
    let image = image
        .or_else(|| opusdm_apps::app_image(&n.app_icon))
        .or_else(|| n.desktop_entry.as_deref().and_then(opusdm_apps::app_image))
        .unwrap_or_else(|| gtk::Image::from_icon_name(FALLBACK_ICON));
    image.set_pixel_size(ICON_SIZE);
    image
}

/// An image of raw `image-data` pixels.
fn texture_image(p: &crate::model::Pixels) -> Option<gtk::Image> {
    let format = if p.has_alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
    let bytes = glib::Bytes::from(&p.data[..]);
    let texture = gdk::MemoryTexture::new(p.width, p.height, format, &bytes, p.stride as usize);
    Some(gtk::Image::from_paintable(Some(&texture)))
}

/// Height the window wants at [`POPUP_WIDTH`].
fn natural_height(window: &gtk::Window) -> i32 {
    let Some(child) = window.child() else { return 1 };
    let (_, natural, _, _) = child.measure(gtk::Orientation::Vertical, POPUP_WIDTH);
    natural.max(1)
}

/// Fade the window in over a few frames (needs a compositor; without one it
/// simply appears).
fn fade_in(window: &gtk::Window) {
    let window = window.downgrade();
    let step = Cell::new(0u32);
    glib::timeout_add_local(Duration::from_millis(FADE_STEP_MS), move || {
        let Some(window) = window.upgrade() else { return glib::ControlFlow::Break };
        step.set(step.get() + 1);
        window.set_opacity(f64::from(step.get()) / f64::from(FADE_STEPS));
        if step.get() >= FADE_STEPS { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
    });
}

/// States of a popup on X11.
const X11_STATES: [&str; 4] = ["_NET_WM_STATE_ABOVE", "_NET_WM_STATE_SKIP_TASKBAR", "_NET_WM_STATE_SKIP_PAGER", "_NET_WM_STATE_STICKY"];

/// X11, once mapped: ask the window manager for the popup states. GDK
/// rewrites `_NET_WM_STATE` when it maps the window, so the property set at
/// realize does not last; the EWMH messages do.
fn add_x11_states(window: &gtk::Window) {
    let Some(xid) = opusdm_ui::x11_window_id(window) else { return };
    let set = opusdm_ui::with_x11(|x11| {
        refuse_x11_input_focus(x11.conn(), xid)?;
        x11.add_wm_states(xid, &X11_STATES)
    });
    if let Err(e) = set {
        opusdm_ipc::err_log!("opusdm-notify", "cannot set the popup states: {e}");
    }
}

/// X11: make the popup unable to take the keyboard focus: `WM_HINTS` input
/// false (GDK writes it as true) and no `WM_TAKE_FOCUS` in `WM_PROTOCOLS`
/// (GTK adds it, and it makes the window "locally active", so the window
/// manager would still focus it).
fn refuse_x11_input_focus(conn: &impl x11rb::connection::Connection, xid: u32) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::properties::WmHints;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let mut hints = WmHints::get(conn, xid)?.reply()?.unwrap_or_default();
    hints.input = Some(false);
    hints.set(conn, xid)?;
    let protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let take_focus = conn.intern_atom(false, b"WM_TAKE_FOCUS")?.reply()?.atom;
    let current = opusdm_x11::property_cards(conn, xid, protocols).unwrap_or_default();
    let kept: Vec<u32> = current.into_iter().filter(|a| *a != take_focus).collect();
    conn.change_property32(PropMode::REPLACE, xid, protocols, AtomEnum::ATOM, &kept)?;
    Ok(())
}

/// X11: mark the window a notification, above the others, on every desktop
/// and out of the task bar and pager.
fn set_x11_type(window: &gtk::Window) {
    let Some(xid) = opusdm_ui::x11_window_id(window) else { return };
    let set = opusdm_ui::with_x11(|x11| {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{AtomEnum, PropMode};
        use x11rb::wrapper::ConnectionExt as _;
        let conn = x11.conn();
        let user_time = x11.atom("_NET_WM_USER_TIME")?;
        x11.set_type_and_states(xid, "_NET_WM_WINDOW_TYPE_NOTIFICATION", &X11_STATES)?;
        // A user time of 0 tells the window manager not to focus the window when it maps.
        conn.change_property32(PropMode::REPLACE, xid, user_time, AtomEnum::CARDINAL, &[0])?;
        refuse_x11_input_focus(x11.conn(), xid)?;
        conn.flush()?;
        Ok(())
    });
    if let Err(e) = set {
        opusdm_ipc::err_log!("opusdm-notify", "cannot set the popup window type: {e}");
    }
}

/// X11: put the window at `x, y` with size `w x h`, above the others.
fn place_x11(window: &gtk::Window, x: i32, y: i32, w: i32, h: i32) {
    let Some(xid) = opusdm_ui::x11_window_id(window) else { return };
    let placed = opusdm_ui::with_x11(|x11| {
        x11.pin_geometry(xid, x, y, w as u32, h as u32)?;
        x11.raise_above(xid)
    });
    if let Err(e) = placed {
        opusdm_ipc::err_log!("opusdm-notify", "cannot place a popup: {e}");
    }
}

/// X11: the usable area of the monitor under the pointer (panels and docks
/// left out).
fn read_x11_area() -> Result<Rect, Box<dyn std::error::Error>> {
    let display = opusdm_snap::x11::Display::connect()?;
    let (pointer, _) = display.pointer()?;
    let area = display.environment()?.area(pointer);
    if area.w <= 0 || area.h <= 0 { return Err("no monitor".into()); }
    Ok((area.x, area.y, area.w, area.h))
}

/// The first monitor GDK knows, else a stand-in.
fn first_monitor() -> Rect {
    gdk::Display::default().as_ref().map(opusdm_ui::list_monitors).and_then(|m| m.first().copied()).unwrap_or(FALLBACK_AREA)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hovered_popup_keeps_its_timer_stopped() {
        assert_eq!(timer_time(true, Some(5000)), None);
        assert_eq!(timer_time(true, None), None);
    }

    #[test]
    fn a_popup_not_hovered_runs_for_its_expire_time() {
        assert_eq!(timer_time(false, Some(5000)), Some(5000));
        assert_eq!(timer_time(false, None), None);
    }
}
