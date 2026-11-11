//! The network icon and its popover, in the right side of the panel.

use crate::network_center::NetworkCenter;
use crate::network_model::{is_locked, row_action, Attempt, IconState, LineMark, Outcome, Request, RowAction};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_network::{AccessPoint, NetState, Security, VpnEntry};
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

/// The program that edits the connections, started by "Configure connections".
const MANAGER_BIN: &str = "opusdm-network-manager";
/// Size of the VPN lock drawn over the icon, as a part of the icon size.
const VPN_LOCK_SCALE: f64 = 0.5;

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
/// Width of the mark column of the status section, in characters.
const MARK_CHARS: i32 = 2;

/// A switch with its label, and a flag that tells `refresh` from the user.
struct SwitchRow {
    root: gtk::Box,
    switch: gtk::Switch,
    /// The IPv4 address, before the switch; empty while there is none.
    address: gtk::Label,
    /// Spins while the device is still negotiating the network.
    spinner: gtk::Spinner,
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
        let address = gtk::Label::new(None);
        address.add_css_class("dim-label");
        address.set_visible(false);
        address.set_tooltip_text(Some("Double click to copy"));
        {
            // A double click copies the address to the clipboard.
            let click = gtk::GestureClick::new();
            click.connect_pressed(|gesture, presses, _, _| {
                if presses != 2 { return; }
                let Some(label) = gesture.widget().and_then(|w| w.downcast::<gtk::Label>().ok()) else { return };
                label.clipboard().set_text(&label.text());
            });
            address.add_controller(click);
        }
        let spinner = gtk::Spinner::new();
        spinner.set_visible(false);
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
        root.append(&address);
        root.append(&spinner);
        root.append(&switch);
        Self { root, switch, address, spinner, updating }
    }

    /// Show the IPv4 address (only while the device is on) and the spinner.
    fn show_link(&self, ip4: Option<&str>, busy: bool) {
        self.address.set_visible(ip4.is_some());
        self.address.set_text(ip4.unwrap_or(""));
        self.spinner.set_visible(busy);
        self.spinner.set_spinning(busy);
    }

    /// Show `active` without sending a request.
    fn show(&self, active: bool, sensitive: bool) {
        self.updating.set(true);
        self.switch.set_active(active);
        self.updating.set(false);
        self.switch.set_sensitive(sensitive);
    }
}

/// The section above "Configure connections": the connect in progress (its
/// steps and time) or how the last one ended. Hidden with no connect.
struct StatusSection {
    root: gtk::Box,
    spinner: gtk::Spinner,
    headline: gtk::Label,
    time: gtk::Label,
    lines: gtk::Box,
    /// The lines shown now, so a refresh that changes only the time does
    /// not rebuild them.
    shown: RefCell<Vec<(LineMark, String)>>,
}

impl StatusSection {
    /// Build the empty, hidden section.
    fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.set_visible(false);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let spinner = gtk::Spinner::new();
        let headline = gtk::Label::new(None);
        headline.set_xalign(0.0);
        headline.set_hexpand(true);
        headline.set_ellipsize(gtk::pango::EllipsizeMode::End);
        headline.add_css_class("heading");
        let time = gtk::Label::new(None);
        time.add_css_class("dim-label");
        top.append(&spinner);
        top.append(&headline);
        top.append(&time);
        let lines = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.append(&top);
        root.append(&lines);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        Self { root, spinner, headline, time, lines, shown: RefCell::new(Vec::new()) }
    }

    /// Show `attempt`, or hide the section without one. `ip4` is the address
    /// shown once connected.
    fn show(&self, attempt: Option<&Attempt>, ip4: Option<&str>) {
        let Some(a) = attempt else {
            self.root.set_visible(false);
            self.shown.borrow_mut().clear();
            return;
        };
        self.root.set_visible(true);
        let running = a.is_running();
        self.spinner.set_visible(running);
        self.spinner.set_spinning(running);
        self.headline.set_text(&a.headline());
        self.time.set_text(&format!("{} s", a.elapsed(Instant::now()).as_secs()));
        let mut wanted: Vec<(LineMark, String)> = a.lines().into_iter().map(|l| (l.mark, l.text)).collect();
        if let (Outcome::Connected, Some(ip4), Some(last)) = (&a.outcome, ip4, wanted.last_mut()) {
            last.1 = format!("{} ({ip4})", last.1);
        }
        if *self.shown.borrow() == wanted { return; }
        while let Some(child) = self.lines.first_child() {
            self.lines.remove(&child);
        }
        for (mark, text) in &wanted {
            self.lines.append(&step_line(*mark, text));
        }
        *self.shown.borrow_mut() = wanted;
    }
}

/// One line of the status section: a mark and the text.
fn step_line(mark: LineMark, text: &str) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let (sign, class) = match mark {
        LineMark::Done => ("\u{2713}", "dim-label"),
        LineMark::Current => ("\u{2022}", ""),
        LineMark::Failed => ("\u{2717}", "error"),
    };
    let sign = gtk::Label::new(Some(sign));
    sign.set_width_chars(MARK_CHARS);
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_wrap(true);
    for widget in [&sign, &label] {
        if !class.is_empty() { widget.add_css_class(class); }
    }
    if mark == LineMark::Current { label.add_css_class("heading"); }
    line.append(&sign);
    line.append(&label);
    line
}

/// The button and its popover. Hidden while NetworkManager is not reachable.
pub struct NetworkButton {
    root: gtk::Box,
    icon: gtk::DrawingArea,
    shown: Rc<Cell<IconState>>,
    /// A VPN is up: the icon gets a small lock.
    vpn_shown: Rc<Cell<bool>>,
    /// Takes the place of the icon while a connect is in progress.
    icon_spinner: gtk::Spinner,
    button: gtk::MenuButton,
    popover: gtk::Popover,
    message: gtk::Label,
    /// The connect in progress or just ended, above "Configure connections".
    status: StatusSection,
    eth: SwitchRow,
    wifi: SwitchRow,
    list: gtk::Box,
    scroll: gtk::ScrolledWindow,
    /// The VPN heading and rows; hidden without VPN profiles.
    vpn_section: gtk::Box,
    vpn_rows: gtk::Box,
}

impl NetworkButton {
    /// Build the button; `refresh` fills it.
    pub fn new(center: &Rc<NetworkCenter>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.set_valign(gtk::Align::Center);
        root.set_visible(false);

        let shown = Rc::new(Cell::new(IconState::Disconnected));
        let vpn_shown = Rc::new(Cell::new(false));
        let icon = gtk::DrawingArea::new();
        icon.set_content_width(ICON_SIZE);
        icon.set_content_height(ICON_SIZE);
        {
            let (shown, vpn_shown) = (shown.clone(), vpn_shown.clone());
            icon.set_draw_func(move |area, cr, w, h| {
                draw_icon(area, cr, w, h, shown.get());
                if vpn_shown.get() { draw_vpn_lock(area, cr, w, h); }
            });
        }
        let icon_spinner = gtk::Spinner::new();
        icon_spinner.set_size_request(ICON_SIZE, ICON_SIZE);
        icon_spinner.set_visible(false);
        let icon_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        icon_box.append(&icon);
        icon_box.append(&icon_spinner);
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon_box));

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
        let status = StatusSection::new();

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

        let vpn_section = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let vpn_heading = gtk::Label::new(Some("VPN"));
        vpn_heading.set_xalign(0.0);
        vpn_heading.add_css_class("heading");
        let vpn_rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vpn_section.append(&vpn_heading);
        vpn_section.append(&vpn_rows);
        vpn_section.set_visible(false);
        let configure = gtk::Button::with_label("Configure connections...");
        configure.add_css_class("flat");

        content.append(&message);
        content.append(&eth.root);
        content.append(&wifi.root);
        content.append(&scroll);
        content.append(&vpn_section);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&status.root);
        content.append(&configure);
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

        {
            let popover = popover.clone();
            configure.connect_clicked(move |_| {
                popover.popdown();
                open_manager();
            });
        }

        root.append(&button);
        root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        Rc::new(Self { root, icon, shown, vpn_shown, icon_spinner, button, popover, message, status, eth, wifi, list, scroll, vpn_section, vpn_rows })
    }

    /// The widget to put in the bar.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Ask for the password of `ssid` and connect with it; `error` says why
    /// it is asked again (the last one was refused).
    pub fn prompt_password(&self, center: &Rc<NetworkCenter>, ssid: &str, security: Security, error: Option<&str>) {
        ask_and_connect(center, &self.popover, &self.button, ssid, security, error);
    }

    /// Whether the widget is still inside a bar window.
    pub fn is_attached(&self) -> bool {
        self.root.parent().is_some()
    }

    /// Show the shared state.
    pub fn refresh(&self, center: &Rc<NetworkCenter>) {
        self.root.set_visible(center.is_available());
        self.shown.set(center.icon_state());
        self.vpn_shown.set(center.vpn_active());
        self.icon.queue_draw();
        let msg = center.message();
        self.message.set_visible(msg.is_some());
        self.message.set_text(msg.as_deref().unwrap_or(""));
        self.refresh_status(center);
        let state = center.state();
        self.show_switches(&state);
        self.show_list(center, &state);
        self.show_vpns(center, &state);
    }

    /// Show the connect in progress: the spinner of the icon, the tooltip and
    /// the status section. Cheap: called every second while connecting.
    pub fn refresh_status(&self, center: &Rc<NetworkCenter>) {
        let busy = center.is_busy();
        self.icon.set_visible(!busy);
        self.icon_spinner.set_visible(busy);
        self.icon_spinner.set_spinning(busy);
        self.button.set_tooltip_text(Some(&center.tooltip()));
        let ip4 = center.state().wifi_ip4;
        self.status.show(center.attempt().as_ref(), ip4.as_deref());
    }

    /// Rebuild the VPN rows; the section is hidden without VPN profiles.
    fn show_vpns(&self, center: &Rc<NetworkCenter>, state: &NetState) {
        self.vpn_section.set_visible(!state.vpns.is_empty());
        while let Some(child) = self.vpn_rows.first_child() {
            self.vpn_rows.remove(&child);
        }
        for vpn in &state.vpns {
            self.vpn_rows.append(&vpn_row(center, &self.popover, &self.button, vpn));
        }
    }

    /// The two switches follow the state; a missing device hides its row.
    fn show_switches(&self, state: &NetState) {
        self.eth.root.set_visible(state.eth_present);
        self.eth.show(state.eth_on, state.eth_available);
        self.eth.show_link(state.eth_ip4.as_deref().filter(|_| state.eth_on), state.eth_busy);
        self.wifi.root.set_visible(state.wifi_present);
        self.wifi.show(state.wifi_enabled, true);
        self.wifi.show_link(state.wifi_ip4.as_deref().filter(|_| state.wifi_enabled), state.wifi_busy);
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
            let connecting = center.connecting();
        for ap in &state.aps {
            let busy = connecting.as_deref() == Some(ap.ssid.as_str());
            self.list.append(&build_row(center, &self.popover, &self.button, ap, busy));
        }
    }
}

/// One network row: a check on the active one (a spinner while `busy`
/// connecting to it), the name, a lock and the signal.
fn build_row(center: &Rc<NetworkCenter>, popover: &gtk::Popover, button: &gtk::MenuButton, ap: &AccessPoint, busy: bool) -> gtk::Button {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(&ap.ssid));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    // The spinner sits over the (empty) check label, so the column keeps the
    // same width and the name does not move.
    let check = gtk::Label::new(Some(if ap.active && !busy { "\u{2713}" } else { "" }));
    check.set_width_chars(2);
    let slot = gtk::Overlay::new();
    slot.set_child(Some(&check));
    if busy {
        let spinner = gtk::Spinner::new();
        spinner.set_spinning(true);
        spinner.set_halign(gtk::Align::Center);
        spinner.set_valign(gtk::Align::Center);
        slot.add_overlay(&spinner);
    }
    line.append(&slot);
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
        RowAction::AskPassword => ask_and_connect(center, popover, button, &ap.ssid, ap.security, None),
    }
}

/// Close the popover, ask for the password of `ssid` (saying `error` when
/// it is asked again) and connect with it; the popover opens again to show
/// the progress.
fn ask_and_connect(center: &Rc<NetworkCenter>, popover: &gtk::Popover, button: &gtk::MenuButton, ssid: &str, security: Security, error: Option<&str>) {
    popover.popdown();
    let parent = button.root().and_then(|r| r.downcast::<gtk::Window>().ok());
    let (center, name, button) = (center.clone(), ssid.to_string(), button.clone());
    ask_password(parent.as_ref(), ssid, security, error, move |password| {
        center.send(Request::Connect { ssid: name.clone(), password: Some(password) });
        // A bar closed by a monitor change has no window to open it in.
        if button.root().is_some() {
            button.popup();
        } else {
            opusdm_ipc::err_log!("opusdm-panel", "network tray: the bar of the password dialog is gone, progress not shown");
        }
    });
}

/// One VPN row: a check on the active one, the name and a switch.
fn vpn_row(center: &Rc<NetworkCenter>, popover: &gtk::Popover, button: &gtk::MenuButton, vpn: &VpnEntry) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let check = gtk::Label::new(Some(if vpn.active { "\u{2713}" } else { "" }));
    check.set_width_chars(2);
    let name = gtk::Label::new(Some(&vpn.name));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    // Set before the handler, so showing the state sends nothing.
    switch.set_active(vpn.active || vpn.activating);
    let (center, popover, button, vpn) = (center.clone(), popover.clone(), button.clone(), vpn.clone());
    switch.connect_active_notify(move |s| {
        if !s.is_active() { return center.send(Request::VpnDown(vpn.uuid.clone())); }
        if !vpn.ask_password { return center.send(Request::VpnUp { uuid: vpn.uuid.clone(), password: None }); }
        popover.popdown();
        let parent = button.root().and_then(|r| r.downcast::<gtk::Window>().ok());
        let (center, uuid) = (center.clone(), vpn.uuid.clone());
        ask_secret(parent.as_ref(), "VPN password", &format!("Password for {}", vpn.name), None, move |password| {
            center.send(Request::VpnUp { uuid: uuid.clone(), password: Some(password) });
        });
    });
    line.append(&check);
    line.append(&name);
    line.append(&switch);
    line
}

/// Start the network manager program, from the folder of the panel first.
fn open_manager() {
    let sibling = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join(MANAGER_BIN)));
    let path = sibling.filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from(MANAGER_BIN));
    if let Err(e) = std::process::Command::new(&path).spawn() {
        opusdm_ipc::err_log!("opusdm-panel", "cannot start {}: {e}", path.display());
    }
}

/// A small modal window that asks for the password of `ssid`; `error`, in
/// red, says why it is asked again. `on_submit` gets the password when the
/// user confirms.
fn ask_password(parent: Option<&gtk::Window>, ssid: &str, security: Security, error: Option<&str>, on_submit: impl Fn(String) + 'static) {
    let kind = if security == Security::Sae { "WPA3" } else { "WPA" };
    let error = error.map(|why| format!("{why}. Try again."));
    ask_secret(parent, "Wi-Fi password", &format!("Password for {ssid} ({kind})"), error.as_deref(), on_submit);
}

/// A small modal window titled `title` that asks for a password with the
/// line `prompt`, and the red line `error` above it when given.
/// `on_submit` gets the password when the user confirms.
fn ask_secret(parent: Option<&gtk::Window>, title: &str, prompt: &str, error: Option<&str>, on_submit: impl Fn(String) + 'static) {
    let window = gtk::Window::builder().title(title).modal(true).resizable(false).build();
    window.set_transient_for(parent);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    for set in [gtk::Widget::set_margin_top, gtk::Widget::set_margin_bottom, gtk::Widget::set_margin_start, gtk::Widget::set_margin_end] {
        set(content.upcast_ref(), 12);
    }
    let label = gtk::Label::new(Some(prompt));
    label.set_xalign(0.0);
    if let Some(error) = error {
        let line = gtk::Label::new(Some(error));
        line.add_css_class("error");
        line.set_xalign(0.0);
        line.set_wrap(true);
        line.set_max_width_chars(36);
        content.append(&line);
    }
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

/// A small padlock in the lower right corner of the icon: a VPN is up.
fn draw_vpn_lock(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, h: i32) {
    let size = f64::from(w.min(h)) * VPN_LOCK_SCALE;
    let _ = cr.save();
    cr.translate(f64::from(w) - size, f64::from(h) - size);
    draw_lock(area, cr, size as i32, size as i32);
    let _ = cr.restore();
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
