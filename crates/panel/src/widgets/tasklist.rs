//! Window list for the current desktop: icon + title per window, click to
//! activate, click the active window to minimise it. Rebuilt wholesale on
//! every snapshot - the window count is small.

use gtk4 as gtk;
use gtk::{gio, glib};
use gtk::prelude::*;

use std::cell::RefCell;

use crate::ewmh::{self, ClientWin};

/// Longest title shown before ellipsis.
const TITLE_CHARS: i32 = 22;

/// The window-list widget.
pub struct Tasklist {
    root: gtk::Box,
    /// Context menus parented to the current buttons, released on rebuild.
    popovers: RefCell<Vec<gtk::PopoverMenu>>,
}

impl Tasklist {
    /// Build an empty, horizontally expanding list.
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        root.add_css_class("opusdm-panel-tasklist");
        root.set_hexpand(true);
        Tasklist { root, popovers: RefCell::new(Vec::new()) }
    }

    /// The GTK widget to pack into the panel.
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Rebuild the buttons for every window on `current_desktop` (plus
    /// omnipresent windows), styling the active and minimised ones.
    pub fn update(
        &self,
        clients: &[ClientWin],
        current_desktop: u32,
        active: Option<u32>,
        desktop_count: u32,
        desktop_names: &[String],
    ) {
        for popover in self.popovers.borrow_mut().drain(..) {
            popover.unparent();
        }
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }
        for c in clients {
            if c.desktop != -1 && c.desktop != current_desktop as i64 {
                continue;
            }
            let btn = self.button(c, active == Some(c.xid));
            let popover = context_menu(&btn, c, desktop_count, desktop_names);
            self.popovers.borrow_mut().push(popover);
            self.root.append(&btn);
        }
    }

    /// One window button: `[icon] title`, click activates or (if already the
    /// shown active window) minimises.
    fn button(&self, c: &ClientWin, is_active: bool) -> gtk::Button {
        let inner = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        inner.append(&icon_image(c));

        let title = if c.title.is_empty() { "(untitled)" } else { c.title.as_str() };
        let label = gtk::Label::new(Some(title));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(TITLE_CHARS);
        label.set_xalign(0.0);
        inner.append(&label);

        let btn = gtk::Button::new();
        btn.set_child(Some(&inner));
        btn.add_css_class("flat");
        if is_active && !c.minimized {
            btn.add_css_class("active");
        }
        if c.minimized {
            btn.add_css_class("dim-label");
        }

        let xid = c.xid;
        let iconify = is_active && !c.minimized;
        btn.connect_clicked(move |_| {
            if iconify {
                ewmh::iconify_window(xid);
            } else {
                ewmh::activate_window(xid);
            }
        });
        btn
    }
}

/// Label of virtual desktop `index`: its name when set, else `Desktop N`.
fn desktop_label(index: u32, names: &[String]) -> String {
    match names.get(index as usize) {
        Some(n) if !n.is_empty() => n.clone(),
        _ => format!("Desktop {}", index + 1),
    }
}

/// Attach the right-click menu to `btn`: "Move to desktop" with one entry per
/// desktop, and "Force kill" (SIGKILL on the window's process) when the
/// client published its pid. Returns the popover so the caller can release it.
fn context_menu(btn: &gtk::Button, c: &ClientWin, count: u32, names: &[String]) -> gtk::PopoverMenu {
    let menu = gio::Menu::new();
    let desktops = gio::Menu::new();
    for i in 0..count {
        desktops.append(Some(&desktop_label(i, names)), Some(&format!("win.move({i})")));
    }
    menu.append_submenu(Some("Move to desktop"), &desktops);
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

/// A `gtk::Image` for the window: its `_NET_WM_ICON` when present, otherwise a
/// generic themed icon.
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
            img.set_pixel_size(18);
            return img;
        }
    }
    let img = gtk::Image::from_icon_name("application-x-executable");
    img.set_pixel_size(18);
    img
}
