//! The network icon and its popover, in the right side of the panel.

use crate::network_center::NetworkCenter;
use crate::network_model::{is_locked, row_action, IconState, Request, RowAction};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_network::{AccessPoint, NetState, Security};
use std::cell::Cell;
use std::f64::consts::PI;
use std::rc::Rc;

/// Size of the panel icon in pixels.
const ICON_SIZE: i32 = 20;
/// Width of the popover, in pixels.
const POPOVER_WIDTH: i32 = 300;
/// The network list scrolls above this height, in pixels.
const LIST_MAX_HEIGHT: i32 = 260;
/// Width of the lock mark in a row, in pixels.
const LOCK_WIDTH: i32 = 12;
/// Width of the signal label, in characters.
const SIGNAL_CHARS: i32 = 5;
/// Arcs of the Wi-Fi symbol.
const ARCS: u8 = 3;
/// Opacity of an arc that is not lit.
const DIM_ALPHA: f64 = 0.3;

/// A switch with its label, and a flag that tells `refresh` from the user.
struct SwitchRow {
    root: gtk::Box,
    switch: gtk::Switch,
    /// Set while `refresh` changes the switch, so it does not send a request.
    updating: Rc<Cell<bool>>,
}

impl SwitchRow {
    /// Build a row; `on_toggle` runs only for the user's clicks.
    fn new(title: &str, on_toggle: impl Fn(bool) + 'static) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(title));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.add_css_class("heading");
        let switch = gtk::Switch::new();
        switch.set_valign(gtk::Align::Center);
        let updating = Rc::new(Cell::new(false));
        {
            let updating = updating.clone();
            switch.connect_active_notify(move |s| {
                if !updating.get() {
                    on_toggle(s.is_active());
                }
            });
        }
        root.append(&label);
        root.append(&switch);
        Self { root, switch, updating }
    }

    /// Show `active` without sending a request.
    fn show(&self, active: bool, sensitive: bool) {
        self.updating.set(true);
        self.switch.set_active(active);
        self.updating.set(false);
        self.switch.set_sensitive(sensitive);
    }
}

/// The button and its popover. Hidden while NetworkManager is not reachable.
pub struct NetworkButton {
    root: gtk::Box,
    icon: gtk::DrawingArea,
    shown: Rc<Cell<IconState>>,
    button: gtk::MenuButton,
    popover: gtk::Popover,
    message: gtk::Label,
    status: gtk::Label,
    eth: SwitchRow,
    wifi: SwitchRow,
    list: gtk::Box,
    scroll: gtk::ScrolledWindow,
}

impl NetworkButton {
    /// Build the button; `refresh` fills it.
    pub fn new(center: &Rc<NetworkCenter>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.set_valign(gtk::Align::Center);
        root.set_visible(false);

        let shown = Rc::new(Cell::new(IconState::Disconnected));
        let icon = gtk::DrawingArea::new();
        icon.set_content_width(ICON_SIZE);
        icon.set_content_height(ICON_SIZE);
        {
            let shown = shown.clone();
            icon.set_draw_func(move |area, cr, w, h| draw_icon(area, cr, w, h, shown.get()));
        }
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
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
        let status = gtk::Label::new(None);
        status.add_css_class("dim-label");
        status.set_visible(false);
        status.set_xalign(0.0);

        let eth = {
            let center = center.clone();
            SwitchRow::new("Ethernet", move |on| center.send(Request::SetEth(on)))
        };
        let wifi = {
            let center = center.clone();
            SwitchRow::new("Wi-Fi", move |on| center.send(Request::SetWifi(on)))
        };
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(LIST_MAX_HEIGHT);
        scroll.set_child(Some(&list));

        content.append(&message);
        content.append(&status);
        content.append(&eth.root);
        content.append(&wifi.root);
        content.append(&scroll);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        button.set_popover(Some(&popover));
        {
            // A fresh scan every time the popover opens.
            let center = center.clone();
            popover.connect_visible_notify(move |p| {
                if p.is_visible() {
                    center.send(Request::Scan);
                }
            });
        }

        root.append(&button);
        root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        Rc::new(Self { root, icon, shown, button, popover, message, status, eth, wifi, list, scroll })
    }

    /// The widget to put in the bar.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Ask for the password of `ssid` and connect with it.
    pub fn prompt_password(&self, center: &Rc<NetworkCenter>, ssid: &str, security: Security) {
        ask_and_connect(center, &self.popover, &self.button, ssid, security);
    }

    /// Whether the widget is still inside a bar window.
    pub fn is_attached(&self) -> bool {
        self.root.parent().is_some()
    }

    /// Show the shared state.
    pub fn refresh(&self, center: &Rc<NetworkCenter>) {
        self.root.set_visible(center.is_available());
        self.shown.set(center.icon_state());
        self.icon.queue_draw();
        self.button.set_tooltip_text(Some(&center.tooltip()));
        let msg = center.message();
        self.message.set_visible(msg.is_some());
        self.message.set_text(msg.as_deref().unwrap_or(""));
        let connecting = center.connecting();
        self.status.set_visible(connecting.is_some());
        self.status.set_text(&connecting.map(|s| format!("Connecting to {s}...")).unwrap_or_default());
        let state = center.state();
        self.show_switches(&state);
        self.show_list(center, &state);
    }

    /// The two switches follow the state; a missing device hides its row.
    fn show_switches(&self, state: &NetState) {
        self.eth.root.set_visible(state.eth_present);
        self.eth.show(state.eth_on, state.eth_available);
        self.wifi.root.set_visible(state.wifi_present);
        self.wifi.show(state.wifi_enabled, true);
    }

    /// Rebuild the list of networks.
    fn show_list(&self, center: &Rc<NetworkCenter>, state: &NetState) {
        let visible = state.wifi_present && state.wifi_enabled;
        self.scroll.set_visible(visible);
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        if !visible { return; }
        if state.aps.is_empty() {
            let empty = gtk::Label::new(Some("No networks in range"));
            empty.add_css_class("dim-label");
            self.list.append(&empty);
            return;
        }
        for ap in &state.aps {
            self.list.append(&build_row(center, &self.popover, &self.button, ap));
        }
    }
}

/// One network row: a check on the active one, the name, a lock and the signal.
fn build_row(center: &Rc<NetworkCenter>, popover: &gtk::Popover, button: &gtk::MenuButton, ap: &AccessPoint) -> gtk::Button {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let check = gtk::Label::new(Some(if ap.active { "\u{2713}" } else { "" }));
    check.set_width_chars(2);
    let name = gtk::Label::new(Some(&ap.ssid));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.append(&check);
    line.append(&name);
    if is_locked(ap.security) {
        let lock = gtk::DrawingArea::new();
        lock.set_content_width(LOCK_WIDTH);
        lock.set_content_height(LOCK_WIDTH);
        lock.set_valign(gtk::Align::Center);
        lock.set_draw_func(draw_lock);
        line.append(&lock);
    }
    let signal = gtk::Label::new(Some(&format!("{}%", ap.signal)));
    signal.set_width_chars(SIGNAL_CHARS);
    signal.set_xalign(1.0);
    signal.add_css_class("dim-label");
    line.append(&signal);

    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.set_child(Some(&line));
    let (center, popover, button, ap) = (center.clone(), popover.clone(), button.clone(), ap.clone());
    row.connect_clicked(move |_| on_row_clicked(&center, &popover, &button, &ap));
    row
}

/// The click on a network row: connect, disconnect or ask for the password.
fn on_row_clicked(center: &Rc<NetworkCenter>, popover: &gtk::Popover, button: &gtk::MenuButton, ap: &AccessPoint) {
    match row_action(ap) {
        RowAction::Disconnect => center.send(Request::DisconnectWifi),
        RowAction::Connect => center.send(Request::Connect { ssid: ap.ssid.clone(), password: None }),
        RowAction::Unsupported(why) => center.show_error(why.to_string()),
        RowAction::AskPassword => ask_and_connect(center, popover, button, &ap.ssid, ap.security),
    }
}

/// Close the popover, ask for the password of `ssid` and connect with it.
fn ask_and_connect(center: &Rc<NetworkCenter>, popover: &gtk::Popover, button: &gtk::MenuButton, ssid: &str, security: Security) {
    popover.popdown();
    let parent = button.root().and_then(|r| r.downcast::<gtk::Window>().ok());
    let (center, name) = (center.clone(), ssid.to_string());
    ask_password(parent.as_ref(), ssid, security, move |password| {
        center.send(Request::Connect { ssid: name.clone(), password: Some(password) });
    });
}

/// A small modal window that asks for the password of `ssid`.
/// `on_submit` gets the password when the user confirms.
fn ask_password(parent: Option<&gtk::Window>, ssid: &str, security: Security, on_submit: impl Fn(String) + 'static) {
    let window = gtk::Window::builder().title("Wi-Fi password").modal(true).resizable(false).build();
    window.set_transient_for(parent);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    for set in [gtk::Widget::set_margin_top, gtk::Widget::set_margin_bottom, gtk::Widget::set_margin_start, gtk::Widget::set_margin_end] {
        set(content.upcast_ref(), 12);
    }
    let kind = if security == Security::Sae { "WPA3" } else { "WPA" };
    let label = gtk::Label::new(Some(&format!("Password for {ssid} ({kind})")));
    label.set_xalign(0.0);
    let entry = gtk::PasswordEntry::new();
    entry.set_show_peek_icon(true);
    entry.set_width_chars(28);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let connect = gtk::Button::with_label("Connect");
    connect.add_css_class("suggested-action");
    connect.set_sensitive(false);
    buttons.append(&cancel);
    buttons.append(&connect);
    content.append(&label);
    content.append(&entry);
    content.append(&buttons);
    window.set_child(Some(&content));
    window.set_default_widget(Some(&connect));
    entry.set_activates_default(true);

    {
        let connect = connect.clone();
        entry.connect_changed(move |e| connect.set_sensitive(!e.text().is_empty()));
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let (window, entry) = (window.clone(), entry.clone());
        connect.connect_clicked(move |_| {
            let password = entry.text().to_string();
            // Do not keep the password in the widget after use.
            entry.set_text("");
            on_submit(password);
            window.close();
        });
    }
    window.present();
    entry.grab_focus();
}

/// Set the cairo source to the widget's text colour.
fn set_text_color(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, alpha: f64) {
    let c = area.color();
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64 * alpha);
}

/// A plug for Ethernet, Wi-Fi arcs with the lit ones bright, or dim arcs with a slash.
fn draw_icon(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, h: i32, state: IconState) {
    let (w, h) = (w as f64, h as f64);
    cr.set_line_width(1.6);
    match state {
        IconState::Ethernet => draw_plug(area, cr, w, h),
        IconState::Wifi(lit) => draw_wifi(area, cr, w, h, lit),
        IconState::Disconnected => {
            draw_wifi(area, cr, w, h, 0);
            set_text_color(area, cr, 1.0);
            cr.move_to(w * 0.15, h * 0.15);
            cr.line_to(w * 0.85, h * 0.85);
            let _ = cr.stroke();
        }
    }
}

/// The Wi-Fi symbol: a dot and three arcs, `lit` of them bright.
fn draw_wifi(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: f64, h: f64, lit: u8) {
    let (cx, cy) = (w * 0.5, h * 0.88);
    set_text_color(area, cr, 1.0);
    cr.arc(cx, cy, 1.8, 0.0, 2.0 * PI);
    let _ = cr.fill();
    for i in 1..=ARCS {
        set_text_color(area, cr, if i <= lit { 1.0 } else { DIM_ALPHA });
        cr.new_sub_path();
        cr.arc(cx, cy, h * 0.26 * i as f64, -0.75 * PI, -0.25 * PI);
        let _ = cr.stroke();
    }
}

/// An Ethernet plug: a box with three contacts.
fn draw_plug(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: f64, h: f64) {
    set_text_color(area, cr, 1.0);
    cr.rectangle(w * 0.2, h * 0.2, w * 0.6, h * 0.4);
    let _ = cr.stroke();
    for i in 0..3 {
        let x = w * (0.32 + 0.18 * i as f64);
        cr.move_to(x, h * 0.6);
        cr.line_to(x, h * 0.85);
    }
    let _ = cr.stroke();
}

/// A closed padlock in the text colour.
fn draw_lock(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, h: i32) {
    let (w, h) = (w as f64, h as f64);
    set_text_color(area, cr, 0.8);
    cr.rectangle(w * 0.15, h * 0.5, w * 0.7, h * 0.45);
    let _ = cr.fill();
    cr.set_line_width(1.4);
    cr.new_sub_path();
    cr.arc(w * 0.5, h * 0.5, w * 0.25, PI, 2.0 * PI);
    let _ = cr.stroke();
}
