//! The widgets of the window that hold no state: a device row, the details
//! of a device, and the texts they show. The window fills them and reads
//! them.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_bluetooth::{BtState, Device};
use opusdm_ui::{labeled_row, LabelWidth};

/// Pixels between widgets.
pub const SPACING: i32 = 8;
/// Pixels around a page.
pub const MARGIN: i32 = 12;
/// Width of the labels of the detail rows, in characters.
const LABEL_CHARS: i32 = 12;
/// Size of the big icon of the details, in pixels.
const DETAIL_ICON_SIZE: i32 = 48;
/// Size of the icon of a list row, in pixels.
const ROW_ICON_SIZE: i32 = 24;

/// The line under the header switch.
pub fn status_text(available: bool, state: &BtState) -> &'static str {
    if !available { return "The Bluetooth service is not running"; }
    let Some(adapter) = &state.adapter else { return "No Bluetooth adapter found" };
    if state.rfkill.hard { return "Blocked by a hardware switch"; }
    if state.rfkill.soft { return "Bluetooth is off (blocked)"; }
    if !adapter.powered { return "Bluetooth is off"; }
    if adapter.discoverable { return "On, visible to other devices"; }
    "On"
}

/// The second line of a device: connected or not, and the battery.
pub fn device_status(d: &Device) -> String {
    let mut s = if d.blocked { "Blocked".to_string() } else if d.connected { "Connected".to_string() } else { "Not connected".to_string() };
    if let Some(b) = d.battery { s.push_str(&format!(", battery {b}%")); }
    s
}

/// One device of a list: its icon, its name and its state.
pub fn device_row(d: &Device, show_status: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
    row.set_margin_top(4);
    row.set_margin_bottom(4);
    row.set_margin_start(4);
    let icon = gtk::Image::from_icon_name(d.kind.icon_name());
    icon.set_pixel_size(ROW_ICON_SIZE);
    row.append(&icon);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let name = gtk::Label::new(Some(&d.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&name);
    let detail = gtk::Label::new(Some(&if show_status { device_status(d) } else { d.address.clone() }));
    detail.set_xalign(0.0);
    detail.add_css_class("dim-label");
    text.append(&detail);
    row.append(&text);
    row
}

/// A read-only value label.
fn value_label() -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_xalign(0.0);
    l.set_hexpand(true);
    l.set_selectable(true);
    l
}

/// The details of the selected device and its actions.
pub struct DetailPage {
    pub root: gtk::Box,
    icon: gtk::Image,
    title: gtk::Label,
    pub name: gtk::Entry,
    pub rename: gtk::Button,
    address: gtk::Label,
    kind: gtk::Label,
    state: gtk::Label,
    battery: gtk::Label,
    battery_row: gtk::Box,
    pub trusted: gtk::Switch,
    pub blocked: gtk::Switch,
    pub connect: gtk::Button,
    pub remove: gtk::Button,
}

impl DetailPage {
    /// Build the empty page.
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        root.set_margin_top(MARGIN);
        root.set_margin_bottom(MARGIN);
        root.set_margin_start(MARGIN);
        root.set_margin_end(MARGIN);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, SPACING * 2);
        let icon = gtk::Image::new();
        icon.set_pixel_size(DETAIL_ICON_SIZE);
        let title = gtk::Label::new(None);
        title.add_css_class("title-2");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        head.append(&icon);
        head.append(&title);
        root.append(&head);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let name = gtk::Entry::new();
        name.set_hexpand(true);
        let rename = gtk::Button::with_label("Rename");
        let name_box = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        name_box.append(&name);
        name_box.append(&rename);
        let (address, kind, state, battery) = (value_label(), value_label(), value_label(), value_label());
        let trusted = gtk::Switch::new();
        trusted.set_halign(gtk::Align::Start);
        let blocked = gtk::Switch::new();
        blocked.set_halign(gtk::Align::Start);
        let row = |text: &str, w: &gtk::Widget| labeled_row(text, LabelWidth::Chars(LABEL_CHARS), SPACING, w);
        root.append(&row("Name", name_box.upcast_ref()));
        root.append(&row("Address", address.upcast_ref()));
        root.append(&row("Type", kind.upcast_ref()));
        root.append(&row("State", state.upcast_ref()));
        let battery_row = row("Battery", battery.upcast_ref());
        root.append(&battery_row);
        let trusted_row = row("Trusted", trusted.upcast_ref());
        trusted_row.set_tooltip_text(Some("A trusted device connects without asking"));
        root.append(&trusted_row);
        let blocked_row = row("Blocked", blocked.upcast_ref());
        blocked_row.set_tooltip_text(Some("A blocked device cannot connect"));
        root.append(&blocked_row);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        actions.set_margin_top(MARGIN);
        let connect = gtk::Button::with_label("Connect");
        connect.add_css_class("suggested-action");
        let remove = gtk::Button::with_label("Remove");
        remove.add_css_class("destructive-action");
        actions.append(&connect);
        actions.append(&remove);
        root.append(&actions);
        Self { root, icon, title, name, rename, address, kind, state, battery, battery_row, trusted, blocked, connect, remove }
    }

    /// Show `d`; `edit_name` also puts its name in the entry (not while the
    /// user types there).
    pub fn show(&self, d: &Device, edit_name: bool) {
        self.icon.set_icon_name(Some(d.kind.icon_name()));
        self.title.set_text(&d.name);
        if edit_name { self.name.set_text(&d.name); }
        self.address.set_text(&d.address);
        self.kind.set_text(d.kind.label());
        self.state.set_text(if d.connected { "Connected" } else { "Not connected" });
        self.battery_row.set_visible(d.battery.is_some());
        self.battery.set_text(&d.battery.map(|b| format!("{b}%")).unwrap_or_default());
        self.trusted.set_active(d.trusted);
        self.blocked.set_active(d.blocked);
        self.connect.set_label(if d.connected { "Disconnect" } else { "Connect" });
        self.connect.set_sensitive(!d.blocked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_bluetooth::model::test_device;
    use opusdm_bluetooth::{Adapter, Rfkill};

    #[test]
    fn status_follows_service_adapter_and_switches() {
        let mut s = BtState::default();
        assert_eq!(status_text(false, &s), "The Bluetooth service is not running");
        assert_eq!(status_text(true, &s), "No Bluetooth adapter found");
        s.adapter = Some(Adapter { powered: false, ..Default::default() });
        assert_eq!(status_text(true, &s), "Bluetooth is off");
        s.rfkill = Rfkill { soft: false, hard: true };
        assert_eq!(status_text(true, &s), "Blocked by a hardware switch");
        s.rfkill = Rfkill { soft: true, hard: false };
        assert_eq!(status_text(true, &s), "Bluetooth is off (blocked)");
        s.rfkill = Rfkill::default();
        s.adapter = Some(Adapter { powered: true, discoverable: true, ..Default::default() });
        assert_eq!(status_text(true, &s), "On, visible to other devices");
    }

    #[test]
    fn device_status_shows_battery() {
        let mut d = test_device("/d", "Keys", true, true);
        assert_eq!(device_status(&d), "Connected");
        d.battery = Some(15);
        d.connected = false;
        assert_eq!(device_status(&d), "Not connected, battery 15%");
        d.blocked = true;
        assert_eq!(device_status(&d), "Blocked, battery 15%");
    }
}
