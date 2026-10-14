//! The notification bell in the right side of the panel: a badge with the
//! unread count, a struck bell while Do Not Disturb is on, and a drop-down
//! with the history. Middle click flips Do Not Disturb.

use crate::notify_center::NotifyCenter;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::notify::{HistoryEntry, NotifyState};
use std::cell::Cell;
use std::f64::consts::PI;
use std::rc::Rc;

/// Size of the bell drawing in pixels (wider than tall: room for the badge).
const ICON_WIDTH: i32 = 26;
const ICON_HEIGHT: i32 = 20;
/// Size of the pictures in the history rows.
const ROW_ICON_SIZE: i32 = 24;
/// Width of the drop-down list.
const LIST_WIDTH: i32 = 360;
/// Height of the drop-down list before it scrolls.
const LIST_MAX_HEIGHT: i32 = 480;
/// Icon of a row whose notification names none.
const FALLBACK_ICON: &str = "dialog-information";
/// Colour of the badge.
const BADGE_RGB: (f64, f64, f64) = (0.898, 0.282, 0.302);

/// Text of the unread badge: none, the count, or `9+`.
pub fn badge_text(unread: u32) -> Option<String> {
    match unread {
        0 => None,
        1..=9 => Some(unread.to_string()),
        _ => Some("9+".to_string()),
    }
}

/// How long ago `time` was, seen at `now` (both seconds since the epoch).
pub fn ago(now: i64, time: i64) -> String {
    let secs = (now - time).max(0);
    match secs {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{} min", secs / 60),
        3600..=86_399 => format!("{} h", secs / 3600),
        _ => format!("{} d", secs / 86_400),
    }
}

/// What the bell draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Look {
    unread: u32,
    dnd: bool,
    running: bool,
}

/// The bell and its drop-down.
pub struct NotifyButton {
    root: gtk::Box,
    icon: gtk::DrawingArea,
    look: Rc<Cell<Look>>,
    dnd: gtk::Switch,
    /// Set while `refresh` moves the switch, so it is not taken as a click.
    syncing: Rc<Cell<bool>>,
    list: gtk::Box,
    clear: gtk::Button,
    popover: gtk::Popover,
}

impl NotifyButton {
    /// Build the bell; `refresh` fills it.
    pub fn new(center: &Rc<NotifyCenter>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);

        let look = Rc::new(Cell::new(Look::default()));
        let icon = gtk::DrawingArea::new();
        icon.set_content_width(ICON_WIDTH);
        icon.set_content_height(ICON_HEIGHT);
        {
            let look = look.clone();
            icon.set_draw_func(move |area, cr, w, h| draw_bell(area, cr, w, h, look.get()));
        }
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon));

        let (dnd, clear, list, content) = build_content();
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        button.set_popover(Some(&popover));
        root.append(&button);

        let syncing = Rc::new(Cell::new(false));
        let me = Rc::new(Self { root, icon, look, dnd, syncing, list, clear, popover });
        me.wire(center, &button);
        me
    }

    /// Connect the switch, the buttons and the clicks to the center.
    fn wire(self: &Rc<Self>, center: &Rc<NotifyCenter>, button: &gtk::MenuButton) {
        {
            let (center, syncing) = (center.clone(), self.syncing.clone());
            self.dnd.connect_active_notify(move |switch| {
                if !syncing.get() { center.client().set_dnd(switch.is_active()); }
            });
        }
        {
            let (center, popover) = (center.clone(), self.popover.clone());
            self.clear.connect_clicked(move |_| {
                center.client().clear_history();
                popover.popdown();
            });
        }
        {
            let (center, root) = (center.clone(), self.root.clone());
            self.popover.connect_show(move |_| {
                // The tooltip would stay over the open drop-down.
                root.set_has_tooltip(false);
                if center.state().is_some_and(|s| s.unread > 0) { center.client().mark_read(); }
            });
        }
        {
            let root = self.root.clone();
            self.popover.connect_closed(move |_| root.set_has_tooltip(true));
        }
        let middle = gtk::GestureClick::new();
        middle.set_button(2);
        {
            let center = center.clone();
            middle.connect_released(move |_, _, _, _| center.toggle_dnd());
        }
        button.add_controller(middle);
    }

    /// The widget to put in the bar.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Whether the widget is still inside a bar window.
    pub fn is_attached(&self) -> bool {
        self.root.parent().is_some()
    }

    /// Show the center's state.
    pub fn refresh(self: &Rc<Self>, center: &Rc<NotifyCenter>) {
        let state = center.state();
        let look = match &state {
            Some(s) => Look { unread: s.unread, dnd: s.dnd, running: true },
            None => Look::default(),
        };
        self.look.set(look);
        self.icon.queue_draw();
        self.root.set_tooltip_text(Some(&tooltip(look)));
        // Setting the text turns the tooltip back on: keep it off while open.
        if self.popover.is_visible() {
            self.root.set_has_tooltip(false);
        }

        self.syncing.set(true);
        self.dnd.set_active(look.dnd);
        self.syncing.set(false);
        self.dnd.set_sensitive(look.running);

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let Some(state) = state else {
            self.list.append(&dim_label("Notification service not running"));
            self.clear.set_sensitive(false);
            return;
        };
        self.clear.set_sensitive(!state.history.is_empty());
        if state.history.is_empty() {
            self.list.append(&dim_label("No notifications"));
            return;
        }
        let now = glib_now();
        for entry in &state.history {
            self.list.append(&self.build_row(center, &state, entry, now));
        }
    }

    /// One history row: picture, app and time, summary, body. Click runs
    /// the default action (or removes it); right click offers Remove and
    /// Mute / Unmute.
    fn build_row(self: &Rc<Self>, center: &Rc<NotifyCenter>, state: &NotifyState, entry: &HistoryEntry, now: i64) -> gtk::Widget {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let image = row_image(&entry.icon);
        image.set_valign(gtk::Align::Start);
        line.append(&image);
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
        texts.set_hexpand(true);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let app = text_label(&entry.app_name, 1);
        app.add_css_class("dim-label");
        app.set_hexpand(true);
        header.append(&app);
        let when = text_label(&ago(now, entry.time), 1);
        when.add_css_class("dim-label");
        header.append(&when);
        texts.append(&header);
        if !entry.summary.is_empty() {
            let summary = text_label(&entry.summary, 1);
            summary.add_css_class("heading");
            texts.append(&summary);
        }
        if !entry.body.is_empty() {
            texts.append(&text_label(&entry.body, 2));
        }
        line.append(&texts);

        let row = gtk::Button::new();
        row.add_css_class("flat");
        row.set_child(Some(&line));
        {
            let (center, popover, id, has_default) = (center.clone(), self.popover.clone(), entry.id, entry.has_default);
            row.connect_clicked(move |_| {
                if has_default { center.client().activate(id) } else { center.client().remove(id) }
                popover.popdown();
            });
        }
        let menu = row_menu(center, &row, entry, state.is_muted(&entry.app_name));
        let right = gtk::GestureClick::new();
        right.set_button(3);
        right.connect_released(move |_, _, _, _| menu.popup());
        row.add_controller(right);
        row.upcast()
    }
}

/// The inside of the drop-down: header with the DND switch, the list,
/// "Clear all". Returns the switch, the button, the list and the whole.
fn build_content() -> (gtk::Switch, gtk::Button, gtk::Box, gtk::Box) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(8);
    content.set_margin_end(8);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some("Notifications"));
    title.add_css_class("heading");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    header.append(&title);
    header.append(&gtk::Label::new(Some("Do not disturb")));
    let dnd = gtk::Switch::new();
    dnd.set_valign(gtk::Align::Center);
    header.append(&dnd);
    content.append(&header);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(LIST_MAX_HEIGHT)
        .min_content_width(LIST_WIDTH)
        .child(&list)
        .build();
    content.append(&scroller);

    let clear = gtk::Button::with_label("Clear all");
    clear.set_halign(gtk::Align::End);
    content.append(&clear);
    (dnd, clear, list, content)
}

/// The right click menu of a history row: Remove, Mute / Unmute its app.
fn row_menu(center: &Rc<NotifyCenter>, row: &gtk::Button, entry: &HistoryEntry, muted: bool) -> gtk::Popover {
    let menu = gtk::Popover::new();
    menu.set_parent(row);
    let items = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let remove = gtk::Button::with_label("Remove");
    remove.add_css_class("flat");
    {
        let (center, menu, id) = (center.clone(), menu.clone(), entry.id);
        remove.connect_clicked(move |_| {
            center.client().remove(id);
            menu.popdown();
        });
    }
    items.append(&remove);
    if !entry.app_name.is_empty() {
        let text = if muted { format!("Unmute {}", entry.app_name) } else { format!("Mute {}", entry.app_name) };
        let mute = gtk::Button::with_label(&text);
        mute.add_css_class("flat");
        let (center, menu, app) = (center.clone(), menu.clone(), entry.app_name.clone());
        mute.connect_clicked(move |_| {
            center.client().set_muted(&app, !muted);
            menu.popdown();
        });
        items.append(&mute);
    }
    menu.set_child(Some(&items));
    // The row is rebuilt on every change: let the menu go with it.
    {
        let menu = menu.clone();
        row.connect_destroy(move |_| menu.unparent());
    }
    menu
}

/// Tooltip of the bell.
fn tooltip(look: Look) -> String {
    if !look.running { return "Notifications (service not running)".to_string(); }
    let count = match look.unread {
        0 => "no new notifications".to_string(),
        1 => "1 new notification".to_string(),
        n => format!("{n} new notifications"),
    };
    if look.dnd { format!("Do not disturb - {count}") } else { format!("Notifications - {count}") }
}

/// A left aligned label of at most `lines` lines.
fn text_label(text: &str, lines: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if lines > 1 {
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_lines(lines);
    }
    label.set_max_width_chars(40);
    label
}

/// A dimmed one line message for the list.
fn dim_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_margin_top(12);
    label.set_margin_bottom(12);
    label
}

/// The picture of a row: a file, a theme icon, an application's icon, else
/// a generic one.
fn row_image(icon: &str) -> gtk::Image {
    let image = if icon.starts_with('/') && std::path::Path::new(icon).is_file() {
        gtk::Image::from_file(icon)
    } else if !icon.is_empty() && gtk::gdk::Display::default().is_some_and(|d| gtk::IconTheme::for_display(&d).has_icon(icon)) {
        gtk::Image::from_icon_name(icon)
    } else {
        app_icon(icon).unwrap_or_else(|| gtk::Image::from_icon_name(FALLBACK_ICON))
    };
    image.set_pixel_size(ROW_ICON_SIZE);
    image
}

/// The icon of the application whose desktop entry is `name`.
fn app_icon(name: &str) -> Option<gtk::Image> {
    if name.is_empty() { return None; }
    let info = gtk::gio::DesktopAppInfo::new(&format!("{name}.desktop"))?;
    let icon = gtk::gio::prelude::AppInfoExt::icon(&info)?;
    Some(gtk::Image::from_gicon(&icon))
}

/// Seconds since the epoch, now.
fn glib_now() -> i64 {
    gtk::glib::real_time() / 1_000_000
}

/// The bell, in the text colour (dimmed while the server is not running),
/// struck while Do Not Disturb is on, with a red badge of the unread count.
fn draw_bell(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, _w: i32, h: i32, look: Look) {
    let c = area.color();
    let alpha = if look.running { c.alpha() as f64 } else { c.alpha() as f64 * 0.4 };
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, alpha);
    let h = h as f64;
    let (cx, top, bottom) = (10.0, h * 0.18, h * 0.78);
    let r = 5.0;
    // Dome, sides flaring out, rim.
    cr.new_sub_path();
    cr.arc(cx, top + r, r, PI, 0.0);
    cr.line_to(cx + r, bottom - 2.5);
    cr.line_to(cx + r + 2.0, bottom);
    cr.line_to(cx - r - 2.0, bottom);
    cr.line_to(cx - r, bottom - 2.5);
    cr.close_path();
    cr.set_line_width(1.6);
    let _ = cr.stroke();
    // Knob and clapper.
    cr.arc(cx, top - 1.0, 1.2, 0.0, 2.0 * PI);
    let _ = cr.fill();
    cr.arc(cx, bottom + 1.6, 1.8, 0.0, PI);
    let _ = cr.fill();
    if look.dnd {
        cr.move_to(cx - 8.0, top - 1.0);
        cr.line_to(cx + 8.0, bottom + 2.0);
        cr.set_line_width(1.8);
        let _ = cr.stroke();
    }
    let Some(text) = badge_text(look.unread) else { return };
    let (bx, by, br) = (cx + 9.0, top + 1.5, 6.0);
    cr.set_source_rgb(BADGE_RGB.0, BADGE_RGB.1, BADGE_RGB.2);
    cr.arc(bx, by, br, 0.0, 2.0 * PI);
    let _ = cr.fill();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.select_font_face("Sans", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Bold);
    cr.set_font_size(if text.len() > 1 { 7.0 } else { 8.5 });
    if let Ok(ext) = cr.text_extents(&text) {
        cr.move_to(bx - ext.width() / 2.0 - ext.x_bearing(), by - ext.height() / 2.0 - ext.y_bearing());
        let _ = cr.show_text(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_caps_at_nine() {
        assert_eq!(badge_text(0), None);
        assert_eq!(badge_text(1).as_deref(), Some("1"));
        assert_eq!(badge_text(9).as_deref(), Some("9"));
        assert_eq!(badge_text(10).as_deref(), Some("9+"));
    }

    #[test]
    fn ago_rounds_down() {
        assert_eq!(ago(1000, 1000), "now");
        assert_eq!(ago(1000, 2000), "now", "a clock skew is not negative");
        assert_eq!(ago(10_000, 10_000 - 59), "now");
        assert_eq!(ago(10_000, 10_000 - 61), "1 min");
        assert_eq!(ago(100_000, 100_000 - 7200), "2 h");
        assert_eq!(ago(1_000_000, 1_000_000 - 3 * 86_400), "3 d");
    }

    #[test]
    fn tooltips() {
        assert_eq!(tooltip(Look::default()), "Notifications (service not running)");
        assert_eq!(tooltip(Look { unread: 1, dnd: false, running: true }), "Notifications - 1 new notification");
        assert_eq!(tooltip(Look { unread: 0, dnd: true, running: true }), "Do not disturb - no new notifications");
    }
}
