//! Window list for the current desktop: icon + title per window, click to
//! activate, click the active window to minimise it, middle-click to close.
//! Each snapshot is diffed by window id: a button is rebuilt only when its
//! icon or menu changes, a title or state change updates it in place.

use gtk4 as gtk;
use gtk::{gio, glib};
use gtk::prelude::*;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ewmh::{self, ClientWin};

/// Longest title shown before ellipsis.
const TITLE_CHARS: i32 = 22;

/// Size of the window icon in a button.
const ICON_PX: i32 = 18;

/// How a window's button follows a new snapshot of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonChange {
    /// Nothing the button shows changed.
    Keep,
    /// The title, the active or the minimised state changed: update in place.
    Update,
    /// The icon or the context menu changed: build a new button.
    Rebuild,
}

/// Decide how the button of a window shown as `old` (active when
/// `old_active`) follows the window now being `new` (active when
/// `new_active`). The geometry and the desktop never show on the button.
fn button_change(old: &ClientWin, old_active: bool, new: &ClientWin, new_active: bool) -> ButtonChange {
    if old.pid != new.pid || old.icon != new.icon || old.app_id != new.app_id { return ButtonChange::Rebuild; }
    let same = old.title == new.title && old.minimized == new.minimized && old_active == new_active;
    if same { ButtonChange::Keep } else { ButtonChange::Update }
}

/// The text shown for a window title.
fn display_title(c: &ClientWin) -> &str {
    if c.title.is_empty() { "(untitled)" } else { c.title.as_str() }
}

/// One window's button and what it was last built or updated from.
struct Entry {
    client: ClientWin,
    active: bool,
    button: gtk::Button,
    label: gtk::Label,
    /// Whether a click minimises (the shown active window) or activates.
    iconify: Rc<Cell<bool>>,
    /// The context menu parented to `button`, released with it.
    popover: gtk::PopoverMenu,
}

impl Entry {
    /// Show `c` (active when `is_active`) on the existing button.
    fn update(&mut self, c: &ClientWin, is_active: bool) {
        self.label.set_text(display_title(c));
        _apply_state(&self.button, &self.iconify, c, is_active);
        self.client = c.clone();
        self.active = is_active;
    }

    /// Take the button and its menu out of `root`.
    fn release(self, root: &gtk::Box) {
        self.popover.unparent();
        root.remove(&self.button);
    }
}

/// The window-list widget.
pub struct Tasklist {
    root: gtk::Box,
    /// The shown buttons, by window id.
    entries: RefCell<HashMap<u32, Entry>>,
    /// Desktop count and names the context menus were built with.
    menu_desktops: RefCell<(u32, Vec<String>)>,
}

impl Tasklist {
    /// Build an empty, horizontally expanding list.
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        root.add_css_class("opusdm-panel-tasklist");
        root.set_hexpand(true);
        Tasklist { root, entries: RefCell::new(HashMap::new()), menu_desktops: RefCell::new((0, Vec::new())) }
    }

    /// The GTK widget to pack into the panel.
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Show a button for every window on `current_desktop` (plus omnipresent
    /// windows), in client list order, styling the active and minimised
    /// ones. Buttons of windows that did not change are kept.
    pub fn update(
        &self,
        clients: &[ClientWin],
        current_desktop: u32,
        active: Option<u32>,
        desktop_count: u32,
        desktop_names: &[String],
    ) {
        let visible: Vec<&ClientWin> = clients
            .iter()
            .filter(|c| c.desktop == -1 || c.desktop == current_desktop as i64)
            .collect();
        let desktops_changed = {
            let mut menu = self.menu_desktops.borrow_mut();
            let changed = menu.0 != desktop_count || menu.1 != desktop_names;
            if changed { *menu = (desktop_count, desktop_names.to_vec()); }
            changed
        };
        let mut entries = self.entries.borrow_mut();
        let shown: HashSet<u32> = visible.iter().map(|c| c.xid).collect();
        let gone: Vec<u32> = entries.keys().copied().filter(|xid| desktops_changed || !shown.contains(xid)).collect();
        for xid in gone {
            if let Some(entry) = entries.remove(&xid) { entry.release(&self.root); }
        }

        let mut previous: Option<gtk::Widget> = None;
        for c in visible {
            let is_active = active == Some(c.xid);
            let change = entries
                .get(&c.xid)
                .map_or(ButtonChange::Rebuild, |e| button_change(&e.client, e.active, c, is_active));
            match change {
                ButtonChange::Keep => {}
                ButtonChange::Update => {
                    if let Some(entry) = entries.get_mut(&c.xid) { entry.update(c, is_active); }
                }
                ButtonChange::Rebuild => {
                    if let Some(old) = entries.remove(&c.xid) { old.release(&self.root); }
                    let entry = new_entry(c, is_active, desktop_count, desktop_names);
                    self.root.append(&entry.button);
                    entries.insert(c.xid, entry);
                }
            }
            let Some(entry) = entries.get(&c.xid) else { continue };
            self.root.reorder_child_after(&entry.button, previous.as_ref());
            previous = Some(entry.button.clone().upcast());
        }
    }
}

/// Build the button and context menu of window `c`.
fn new_entry(c: &ClientWin, is_active: bool, desktop_count: u32, desktop_names: &[String]) -> Entry {
    let (button, label, iconify) = new_button(c, is_active);
    let popover = context_menu(&button, c, desktop_count, desktop_names);
    Entry { client: c.clone(), active: is_active, button, label, iconify, popover }
}

/// One window button: `[icon] title`, click activates or (if already the
/// shown active window) minimises, middle-click closes. Returns the button,
/// its title label and the shared "a click minimises" flag.
fn new_button(c: &ClientWin, is_active: bool) -> (gtk::Button, gtk::Label, Rc<Cell<bool>>) {
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    inner.append(&icon_image(c));

    let label = gtk::Label::new(Some(display_title(c)));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(TITLE_CHARS);
    label.set_xalign(0.0);
    inner.append(&label);

    let btn = gtk::Button::new();
    btn.set_child(Some(&inner));
    btn.add_css_class("flat");
    let iconify = Rc::new(Cell::new(false));
    _apply_state(&btn, &iconify, c, is_active);

    let xid = c.xid;
    {
        let iconify = iconify.clone();
        btn.connect_clicked(move |_| {
            if iconify.get() {
                ewmh::iconify_window(xid);
            } else {
                ewmh::activate_window(xid);
            }
        });
    }

    let middle = gtk::GestureClick::new();
    middle.set_button(gtk::gdk::BUTTON_MIDDLE);
    middle.connect_released(move |_, _, _, _| ewmh::close_window(xid));
    btn.add_controller(middle);
    (btn, label, iconify)
}

/// Style `btn` for the active and minimised state of `c`, and set what a
/// click on it does.
fn _apply_state(btn: &gtk::Button, iconify: &Cell<bool>, c: &ClientWin, is_active: bool) {
    let shown_active = is_active && !c.minimized;
    if shown_active { btn.add_css_class("active"); } else { btn.remove_css_class("active"); }
    if c.minimized { btn.add_css_class("dim-label"); } else { btn.remove_css_class("dim-label"); }
    iconify.set(shown_active);
}

/// Label of virtual desktop `index`: its name when set, else `Desktop N`.
fn desktop_label(index: u32, names: &[String]) -> String {
    match names.get(index as usize) {
        Some(n) if !n.is_empty() => n.clone(),
        _ => format!("Desktop {}", index + 1),
    }
}

/// Attach the right-click menu to `btn`: "Move to desktop" with one entry per
/// desktop, "Close Window" (polite close request), and "Force kill" (SIGKILL on the window's process) when the
/// client published its pid. Returns the popover so the caller can release it.
fn context_menu(btn: &gtk::Button, c: &ClientWin, count: u32, names: &[String]) -> gtk::PopoverMenu {
    let menu = gio::Menu::new();
    // Wayland has no request that moves another client's window.
    if opusdm_ipc::session::is_x11() {
        let desktops = gio::Menu::new();
        for i in 0..count {
            desktops.append(Some(&desktop_label(i, names)), Some(&format!("win.move({i})")));
        }
        menu.append_submenu(Some("Move to desktop"), &desktops);
    }
    menu.append(Some("Close Window"), Some("win.close"));
    if c.pid.is_some() {
        menu.append(Some("Force kill"), Some("win.kill"));
    }

    let actions = gio::SimpleActionGroup::new();
    let xid = c.xid;
    let move_action = gio::SimpleAction::new("move", Some(glib::VariantTy::INT32));
    move_action.connect_activate(move |_, param| {
        let Some(index) = param.and_then(|p| p.get::<i32>()) else { return };
        ewmh::move_window_to_desktop(xid, index.max(0) as u32);
    });
    actions.add_action(&move_action);
    let close_action = gio::SimpleAction::new("close", None);
    close_action.connect_activate(move |_, _| ewmh::close_window(xid));
    actions.add_action(&close_action);
    if let Some(pid) = c.pid {
        let kill_action = gio::SimpleAction::new("kill", None);
        kill_action.connect_activate(move |_, _| ewmh::force_kill(pid));
        actions.add_action(&kill_action);
    }
    btn.insert_action_group("win", Some(&actions));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(btn);
    popover.set_has_arrow(false);
    let secondary = gtk::GestureClick::new();
    secondary.set_button(gtk::gdk::BUTTON_SECONDARY);
    {
        let popover = popover.clone();
        secondary.connect_pressed(move |_, _, _, _| popover.popup());
    }
    btn.add_controller(secondary);
    popover
}

impl Default for Tasklist {
    fn default() -> Self {
        Self::new()
    }
}

/// A `gtk::Image` for the window: its `_NET_WM_ICON` when present, else the
/// icon of the desktop file its Wayland app id names, else a generic themed
/// icon.
fn icon_image(c: &ClientWin) -> gtk::Image {
    if let Some(icon) = &c.icon {
        if icon.width > 0 && icon.height > 0 && icon.rgba.len() == (icon.width * icon.height * 4) as usize
        {
            let bytes = glib::Bytes::from(&icon.rgba);
            let texture = gtk::gdk::MemoryTexture::new(
                icon.width,
                icon.height,
                gtk::gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                (icon.width * 4) as usize,
            );
            let img = gtk::Image::from_paintable(Some(&texture));
            img.set_pixel_size(ICON_PX);
            return img;
        }
    }
    let img = match c.app_id.as_deref().and_then(app_icon) {
        Some(icon) => gtk::Image::from_gicon(&icon),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    img.set_pixel_size(ICON_PX);
    img
}

/// The icon of the desktop file named by the Wayland `app_id`
/// (`org.gnome.Nautilus`, or `foot` for `foot.desktop`), trying the
/// lowercase name too.
fn app_icon(app_id: &str) -> Option<gio::Icon> {
    use gio::prelude::AppInfoExt;
    [app_id.to_string(), app_id.to_lowercase()]
        .iter()
        .find_map(|id| gio::DesktopAppInfo::new(&format!("{id}.desktop")))
        .and_then(|info| info.icon())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ewmh::{IconRgba, WinRect};

    fn win(title: &str) -> ClientWin {
        ClientWin { xid: 1, title: title.into(), desktop: 0, minimized: false, pid: None, icon: None, app_id: None, rect: WinRect::default() }
    }

    #[test]
    fn an_unchanged_window_keeps_its_button() {
        assert_eq!(button_change(&win("a"), false, &win("a"), false), ButtonChange::Keep);
    }

    #[test]
    fn a_geometry_or_desktop_change_keeps_the_button() {
        let mut moved = win("a");
        moved.rect = WinRect { x: 10, y: 10, w: 5, h: 5 };
        moved.desktop = -1;
        assert_eq!(button_change(&win("a"), false, &moved, false), ButtonChange::Keep);
    }

    #[test]
    fn title_and_state_changes_update_in_place() {
        assert_eq!(button_change(&win("a"), true, &win("b"), true), ButtonChange::Update);
        assert_eq!(button_change(&win("a"), false, &win("a"), true), ButtonChange::Update);
        let mut min = win("a");
        min.minimized = true;
        assert_eq!(button_change(&win("a"), false, &min, false), ButtonChange::Update);
    }

    #[test]
    fn icon_or_menu_changes_rebuild() {
        let mut pid = win("a");
        pid.pid = Some(42);
        assert_eq!(button_change(&win("a"), false, &pid, false), ButtonChange::Rebuild);
        let mut icon = win("a");
        icon.icon = Some(IconRgba { width: 1, height: 1, rgba: vec![0; 4] });
        assert_eq!(button_change(&win("a"), false, &icon, false), ButtonChange::Rebuild);
    }

    #[test]
    fn an_empty_title_shows_untitled() {
        assert_eq!(display_title(&win("")), "(untitled)");
        assert_eq!(display_title(&win("x")), "x");
    }
}
