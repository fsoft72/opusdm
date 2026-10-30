//! Hosted SNI item icons: one small clickable button per item registered
//! with `sni.rs`'s watcher, rendered from the item's pixmap (or icon-theme
//! name as a fallback). A click shows the item's DBusMenu popover or sends
//! a plain `Activate`/`ContextMenu` call.

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

/// The SNI icon strip of one bar. The items themselves live in the panel's
/// shared state (see `sni::apply_to_list`), so a rebuilt bar (monitor
/// change, screen resize) gets a new strip seeded with every item already
/// hosted, instead of losing them.
pub struct SniStrip {
    root: gtk::Box,
    cmd_tx: Sender<SniCmd>,
    buttons: RefCell<HashMap<String, ItemButton>>,
}

impl SniStrip {
    /// Build an empty strip. `cmd_tx` is the sender half from `sni::spawn`.
    pub fn new(cmd_tx: Sender<SniCmd>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, crate::widgets::tray::ICON_SPACING);
        root.set_valign(gtk::Align::Center);
        Rc::new(Self { root, cmd_tx, buttons: RefCell::new(HashMap::new()) })
    }

    /// The strip's top-level widget, for packing into the bar.
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Reconcile one `SniEvent` into the displayed buttons: add/update/remove.
    pub fn apply(&self, ev: &SniEvent) {
        match ev {
            SniEvent::Added { id, item } | SniEvent::Changed { id, item } => {
                let mut buttons = self.buttons.borrow_mut();
                if let Some(entry) = buttons.get(id) {
                    entry.button.set_child(Some(&icon_widget(item)));
                    *entry.menu_path.borrow_mut() = item.menu_path.clone();
                    *entry.item_path.borrow_mut() = item.path.clone();
                } else {
                    let btn = gtk::Button::new();
                    btn.add_css_class("flat");
                    btn.add_css_class("opusdm-panel-tray-item");
                    btn.set_child(Some(&icon_widget(item)));
                    let menu_path = Rc::new(RefCell::new(item.menu_path.clone()));
                    let item_path = Rc::new(RefCell::new(item.path.clone()));
                    wire_click(&btn, item.service.clone(), menu_path.clone(), item_path.clone(), self.cmd_tx.clone());
                    self.root.append(&btn);
                    buttons.insert(id.clone(), ItemButton { button: btn, menu_path, item_path });
                }
            }
            SniEvent::Removed { id } => {
                if let Some(entry) = self.buttons.borrow_mut().remove(id) {
                    self.root.remove(&entry.button);
                }
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
                show_menu(&btn, &service, &path, &cmd_tx);
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

/// Text of the popover shown while the app has not sent its menu yet.
const MENU_WAITING_TEXT: &str = "Waiting for the app...";

/// Text of that popover when the app did not send a menu.
const MENU_FAILED_TEXT: &str = "The app did not send its menu";

/// Show `service`'s DBusMenu, anchored to `btn`. The menu is read on the SNI
/// side (`SniCmd::FetchMenu`), never here: a busy app would freeze the panel
/// until it answers. Meanwhile a small popover says the menu is coming; it
/// turns into the menu when the reply arrives, and a reply that comes after
/// the user closed it is dropped.
fn show_menu(btn: &gtk::Button, service: &str, path: &zbus::zvariant::OwnedObjectPath, cmd_tx: &Sender<SniCmd>) {
    let label = gtk::Label::new(Some(MENU_WAITING_TEXT));
    let waiting = gtk::Popover::new();
    waiting.set_child(Some(&label));
    waiting.set_parent(btn);
    unparent_when_closed(&waiting);
    waiting.popup();

    let (reply_tx, reply_rx) = async_channel::bounded(1);
    let fetch = SniCmd::FetchMenu { service: service.to_string(), menu_path: path.clone(), reply: reply_tx };
    if cmd_tx.send(fetch).is_err() {
        label.set_text(MENU_FAILED_TEXT);
        return;
    }

    let (btn, service, path, cmd_tx) = (btn.clone(), service.to_string(), path.clone(), cmd_tx.clone());
    glib::spawn_future_local(async move {
        let Ok(menu) = reply_rx.recv().await else { return };
        if !waiting.is_visible() {
            return;
        }
        let Ok(root) = menu else {
            label.set_text(MENU_FAILED_TEXT);
            return;
        };
        waiting.popdown();
        let popover = crate::dbusmenu::build_popover(&root, move |id| {
            let clicked = SniCmd::MenuClicked { service: service.clone(), menu_path: path.clone(), id };
            let _ = cmd_tx.send(clicked);
        });
        popover.set_parent(&btn);
        unparent_when_closed(&popover);
        popover.popup();
    });
}

/// Detach `popover` from its button once it closes, so every click doesn't
/// leave one more popover parented to the button. Deferred to idle: GTK
/// is still inside the popover's own code when `closed` is emitted.
fn unparent_when_closed(popover: &impl IsA<gtk::Popover>) {
    popover.upcast_ref::<gtk::Popover>().connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
}

/// True when an `IconName` is an absolute file path instead of a theme name.
fn is_icon_path(name: &str) -> bool {
    name.starts_with('/')
}

/// Render one item's icon: its pixmap when present, else the picture at an
/// absolute `IconName` path or an icon-theme lookup by name, else a generic
/// placeholder glyph.
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
        if is_icon_path(name) {
            match opusdm_apps::app_image(name) {
                Some(image) => {
                    image.set_pixel_size(18);
                    return image.upcast();
                }
                None => opusdm_ipc::err_log!("opusdm-panel", "tray icon: cannot load picture {name}"),
            }
        } else {
            let image = gtk::Image::from_icon_name(name);
            image.set_pixel_size(18);
            return image.upcast();
        }
    }
    let image = gtk::Image::from_icon_name("image-missing");
    image.set_pixel_size(18);
    image.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_path_is_a_picture_and_a_plain_name_is_a_theme_icon() {
        assert!(is_icon_path("/usr/share/icons/app.png"));
        assert!(!is_icon_path("network-wired"));
        assert!(!is_icon_path(""));
    }
}
