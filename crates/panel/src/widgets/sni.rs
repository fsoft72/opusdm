//! Hosted SNI item icons: one small clickable button per item registered
//! with `sni.rs`'s watcher, rendered from the item's pixmap (or icon-theme
//! name as a fallback). Click handling (DBusMenu popover vs a plain
//! `Activate`/`ContextMenu` call) is wired on top in a follow-up change to
//! this file - `build`/`apply_event` here only keep the icons themselves
//! in sync with the registered-item set.

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use crate::sni::{SniCmd, SniEvent, SniItem};

/// One hosted item's button plus the mutable bits its click handler needs
/// to read fresh on every click (menu path can change between clicks - an
/// app can add/remove `Menu` at runtime).
struct ItemButton {
    button: gtk::Button,
    menu_path: Rc<RefCell<Option<zbus::zvariant::OwnedObjectPath>>>,
    /// The item's actual `org.kde.StatusNotifierItem` object path - needed
    /// for `Activate`/`ContextMenu`, which are NOT always at the spec's
    /// default `/StatusNotifierItem` (see `sni::resolve_service`).
    item_path: Rc<RefCell<String>>,
}

/// Build the (initially empty) SNI icon strip and start draining `evt_rx`
/// into it. `cmd_tx` is the sender half from `sni::spawn`.
pub fn build(cmd_tx: Sender<SniCmd>, evt_rx: async_channel::Receiver<SniEvent>) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    root.set_valign(gtk::Align::Center);

    let buttons: Rc<RefCell<HashMap<String, ItemButton>>> = Rc::new(RefCell::new(HashMap::new()));

    {
        let root = root.clone();
        let buttons = buttons.clone();
        glib::spawn_future_local(async move {
            while let Ok(ev) = evt_rx.recv().await {
                apply_event(&root, &buttons, &cmd_tx, ev);
            }
        });
    }

    root.upcast()
}

/// Reconcile one `SniEvent` into the displayed buttons: add/update/remove.
fn apply_event(
    root: &gtk::Box,
    buttons: &Rc<RefCell<HashMap<String, ItemButton>>>,
    cmd_tx: &Sender<SniCmd>,
    ev: SniEvent,
) {
    match ev {
        SniEvent::Added { service, item } | SniEvent::Changed { service, item } => {
            let mut buttons = buttons.borrow_mut();
            if let Some(entry) = buttons.get(&service) {
                entry.button.set_child(Some(&icon_widget(&item)));
                *entry.menu_path.borrow_mut() = item.menu_path.clone();
                *entry.item_path.borrow_mut() = item.path.clone();
            } else {
                let btn = gtk::Button::new();
                btn.add_css_class("flat");
                btn.set_child(Some(&icon_widget(&item)));
                let menu_path = Rc::new(RefCell::new(item.menu_path.clone()));
                let item_path = Rc::new(RefCell::new(item.path.clone()));
                wire_click(&btn, service.clone(), menu_path.clone(), item_path.clone(), cmd_tx.clone());
                root.append(&btn);
                buttons.insert(service, ItemButton { button: btn, menu_path, item_path });
            }
        }
        SniEvent::Removed { service } => {
            if let Some(entry) = buttons.borrow_mut().remove(&service) {
                root.remove(&entry.button);
            }
        }
    }
}

/// Attach primary/secondary click handling to one item's button: show its
/// DBusMenu popover when it has one, otherwise send `Activate`/
/// `ContextMenu` over `cmd_tx`.
fn wire_click(
    btn: &gtk::Button,
    service: String,
    menu_path: Rc<RefCell<Option<zbus::zvariant::OwnedObjectPath>>>,
    item_path: Rc<RefCell<String>>,
    cmd_tx: Sender<SniCmd>,
) {
    let click = |button_id: u32| {
        let btn = btn.clone();
        let service = service.clone();
        let menu_path = menu_path.clone();
        let item_path = item_path.clone();
        let cmd_tx = cmd_tx.clone();
        move |_gesture: &gtk::GestureClick, _n: i32, x: f64, y: f64| {
            if let Some(path) = menu_path.borrow().clone() {
                show_menu(&btn, &service, &path);
                return;
            }
            let path = item_path.borrow().clone();
            let cmd = if button_id == 1 {
                SniCmd::Activate { service: service.clone(), path, x: x as i32, y: y as i32 }
            } else {
                SniCmd::ContextMenu { service: service.clone(), path, x: x as i32, y: y as i32 }
            };
            let _ = cmd_tx.send(cmd);
        }
    };

    let primary = gtk::GestureClick::new();
    primary.set_button(gtk::gdk::BUTTON_PRIMARY);
    primary.connect_pressed(click(1));
    btn.add_controller(primary);

    let secondary = gtk::GestureClick::new();
    secondary.set_button(gtk::gdk::BUTTON_SECONDARY);
    secondary.connect_pressed(click(3));
    btn.add_controller(secondary);
}

/// Fetch and show `service`'s DBusMenu, anchored to `btn`. A short-lived
/// connection + blocking call, same throwaway style as `sni.rs`'s
/// `send_activate`/`send_context_menu` - acceptable since this only runs
/// once per click, not per frame.
fn show_menu(btn: &gtk::Button, service: &str, path: &zbus::zvariant::OwnedObjectPath) {
    let Ok(conn) = zbus::blocking::Connection::session() else { return };
    let Ok(root) = crate::dbusmenu::fetch_layout(&conn, service, path) else { return };

    let service = service.to_string();
    let path = path.clone();
    let popover = crate::dbusmenu::build_popover(&root, move |id| {
        if let Ok(conn) = zbus::blocking::Connection::session() {
            crate::dbusmenu::send_event_clicked(&conn, &service, &path, id);
        }
    });
    popover.set_parent(btn);
    popover.popup();
}

/// Render one item's icon: its pixmap when present, else an icon-theme
/// lookup by name, else a generic placeholder glyph.
fn icon_widget(item: &SniItem) -> gtk::Widget {
    if let Some(icon) = &item.icon {
        let bytes = glib::Bytes::from(&icon.rgba);
        let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_bytes(
            &bytes,
            gtk::gdk_pixbuf::Colorspace::Rgb,
            true,
            8,
            icon.width,
            icon.height,
            icon.width * 4,
        );
        return gtk::Image::from_pixbuf(Some(&pixbuf)).upcast();
    }
    if let Some(name) = &item.icon_name {
        let image = gtk::Image::from_icon_name(name);
        image.set_pixel_size(18);
        return image.upcast();
    }
    let image = gtk::Image::from_icon_name("image-missing");
    image.set_pixel_size(18);
    image.upcast()
}
