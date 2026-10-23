//! The Bluetooth icon and its popover, in the right side of the panel: the
//! on / off switch, the paired devices with Connect / Disconnect, and the
//! way to the Bluetooth manager. Hidden while there is no adapter.

use crate::bluetooth_center::BluetoothCenter;
use crate::bluetooth_model::{self, Look};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_bluetooth::{Device, Request};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

/// The program that manages Bluetooth, started by "Add device..." and "Configure...".
const MANAGER_BIN: &str = "opusdm-bluetooth-manager";
/// Its flag that opens "Add device" at once.
const PAIR_FLAG: &str = "--pair";
/// Size of the panel icon in pixels.
const ICON_SIZE: i32 = 20;
/// Width of the popover, in pixels.
const POPOVER_WIDTH: i32 = 300;
/// The device list scrolls above this height, in pixels.
const LIST_MAX_HEIGHT: i32 = 300;
/// Size of a device icon in the list, in pixels.
const ROW_ICON_SIZE: i32 = 20;
/// Opacity of the symbol while Bluetooth is off.
const OFF_ALPHA: f64 = 0.4;
/// Colour of the dot drawn while a device is connected.
const CONNECTED_DOT: (f64, f64, f64) = (0.208, 0.518, 0.894);

/// The button and its popover.
pub struct BluetoothButton {
    root: gtk::Box,
    icon: gtk::DrawingArea,
    shown: Rc<Cell<Look>>,
    button: gtk::MenuButton,
    popover: gtk::Popover,
    message: gtk::Label,
    switch: gtk::Switch,
    /// Set while `refresh` changes the switch, so it does not send a request.
    updating: Rc<Cell<bool>>,
    status: gtk::Label,
    list: gtk::Box,
}

impl BluetoothButton {
    /// Build the button; `refresh` fills it.
    pub fn new(center: &Rc<BluetoothCenter>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        root.set_visible(false);
        let shown = Rc::new(Cell::new(Look::Hidden));
        let icon = gtk::DrawingArea::new();
        icon.set_content_width(ICON_SIZE);
        icon.set_content_height(ICON_SIZE);
        {
            let shown = shown.clone();
            icon.set_draw_func(move |area, cr, w, h| draw_bluetooth(area, cr, w, h, shown.get()));
        }
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_width_request(POPOVER_WIDTH);
        let message = gtk::Label::new(None);
        message.add_css_class("error");
        message.set_visible(false);
        message.set_xalign(0.0);
        message.set_wrap(true);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(Some("Bluetooth"));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.add_css_class("heading");
        let switch = gtk::Switch::new();
        switch.set_valign(gtk::Align::Center);
        let updating = Rc::new(Cell::new(false));
        {
            let (center, updating) = (center.clone(), updating.clone());
            switch.connect_active_notify(move |s| {
                if !updating.get() { center.send(Request::SetPowered(s.is_active())); }
            });
        }
        head.append(&title);
        head.append(&switch);
        let status = gtk::Label::new(None);
        status.add_css_class("dim-label");
        status.set_xalign(0.0);
        status.set_wrap(true);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(LIST_MAX_HEIGHT);
        scroll.set_child(Some(&list));

        let add = gtk::Button::with_label("Add device...");
        add.add_css_class("flat");
        let configure = gtk::Button::with_label("Configure...");
        configure.add_css_class("flat");
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        actions.set_homogeneous(true);
        actions.append(&add);
        actions.append(&configure);

        content.append(&message);
        content.append(&head);
        content.append(&status);
        content.append(&scroll);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&actions);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        button.set_popover(Some(&popover));
        for (b, pair) in [(&add, true), (&configure, false)] {
            let popover = popover.clone();
            b.connect_clicked(move |_| {
                popover.popdown();
                open_manager(pair);
            });
        }
        root.append(&button);
        Rc::new(Self { root, icon, shown, button, popover, message, switch, updating, status, list })
    }

    /// The widget to put in the bar.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Whether the widget is still inside a bar window.
    pub fn is_attached(&self) -> bool {
        self.root.parent().is_some()
    }

    /// Show the shared state.
    pub fn refresh(&self, center: &Rc<BluetoothCenter>) {
        let look = center.look();
        let state = center.state();
        self.root.set_visible(look != Look::Hidden);
        if look == Look::Hidden { self.popover.popdown(); }
        self.shown.set(look);
        self.icon.queue_draw();
        self.button.set_tooltip_text(Some(&bluetooth_model::tooltip(&state)));
        let msg = center.message();
        self.message.set_visible(msg.is_some());
        self.message.set_text(msg.as_deref().unwrap_or(""));
        self.updating.set(true);
        self.switch.set_active(state.is_on());
        self.updating.set(false);
        self.switch.set_sensitive(!state.rfkill.hard);
        let paired = state.paired();
        let status = if state.rfkill.hard {
            "Blocked by a hardware switch"
        } else if !state.is_on() {
            "Bluetooth is off"
        } else if paired.is_empty() {
            "No paired devices"
        } else {
            ""
        };
        self.status.set_text(status);
        self.status.set_visible(!status.is_empty());
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        if !state.is_on() { return; }
        for d in paired {
            self.list.append(&device_row(center, d));
        }
    }
}

/// One paired device: icon, name, state, and Connect / Disconnect.
fn device_row(center: &Rc<BluetoothCenter>, d: &Device) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon = gtk::Image::from_icon_name(d.kind.icon_name());
    icon.set_pixel_size(ROW_ICON_SIZE);
    line.append(&icon);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&d.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&name);
    let mut detail = if d.connected { "Connected".to_string() } else { "Not connected".to_string() };
    if let Some(b) = d.battery { detail.push_str(&format!(", battery {b}%")); }
    let detail = gtk::Label::new(Some(&detail));
    detail.set_xalign(0.0);
    detail.add_css_class("dim-label");
    text.append(&detail);
    line.append(&text);
    let (label, request) = if d.connected { ("Disconnect", Request::Disconnect(d.path.clone())) } else { ("Connect", Request::Connect(d.path.clone())) };
    let b = gtk::Button::with_label(label);
    b.add_css_class("flat");
    b.set_valign(gtk::Align::Center);
    b.set_sensitive(!d.blocked);
    {
        let center = center.clone();
        b.connect_clicked(move |_| center.send(request.clone()));
    }
    line.append(&b);
    line
}

/// Start the Bluetooth manager, from the folder of the panel first; with
/// `pair`, it opens "Add device" at once.
fn open_manager(pair: bool) {
    let sibling = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join(MANAGER_BIN)));
    let path = sibling.filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from(MANAGER_BIN));
    let mut cmd = std::process::Command::new(&path);
    if pair { cmd.arg(PAIR_FLAG); }
    if let Err(e) = cmd.spawn() {
        opusdm_ipc::err_log!("opusdm-panel", "cannot start {}: {e}", path.display());
    }
}

/// The Bluetooth rune in the text colour, faded while off, with a blue dot
/// while a device is connected.
fn draw_bluetooth(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, h: i32, look: Look) {
    let c = area.color();
    let alpha = if look == Look::Off { c.alpha() as f64 * OFF_ALPHA } else { c.alpha() as f64 };
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, alpha);
    let (w, h) = (w as f64, h as f64);
    let (top, bottom) = (h * 0.1, h * 0.9);
    let (cx, left, right) = (w * 0.45, w * 0.2, w * 0.7);
    let (upper, lower) = (h * 0.3, h * 0.7);
    cr.set_line_width(1.6);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    // The spine and the two arrow heads that cross over it.
    cr.move_to(left, upper);
    cr.line_to(right, lower);
    cr.line_to(cx, bottom);
    cr.line_to(cx, top);
    cr.line_to(right, upper);
    cr.line_to(left, lower);
    let _ = cr.stroke();
    if look != Look::Connected { return; }
    cr.set_source_rgb(CONNECTED_DOT.0, CONNECTED_DOT.1, CONNECTED_DOT.2);
    cr.arc(w * 0.85, h * 0.8, w * 0.13, 0.0, 2.0 * std::f64::consts::PI);
    let _ = cr.fill();
}
