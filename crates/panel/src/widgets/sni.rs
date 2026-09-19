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

/// Build the (initially empty) SNI icon strip and start draining `evt_rx`
/// into it. `cmd_tx` is the sender half from `sni::spawn`.
pub fn build(cmd_tx: Sender<SniCmd>, evt_rx: async_channel::Receiver<SniEvent>) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    root.set_valign(gtk::Align::Center);

    let buttons: Rc<RefCell<HashMap<String, gtk::Button>>> = Rc::new(RefCell::new(HashMap::new()));

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
pub fn apply_event(
    root: &gtk::Box,
    buttons: &Rc<RefCell<HashMap<String, gtk::Button>>>,
    _cmd_tx: &Sender<SniCmd>,
    ev: SniEvent,
) {
    match ev {
        SniEvent::ItemAdded { service, item } | SniEvent::ItemChanged { service, item } => {
            let mut buttons = buttons.borrow_mut();
            if let Some(btn) = buttons.get(&service) {
                btn.set_child(Some(&icon_widget(&item)));
            } else {
                let btn = gtk::Button::new();
                btn.add_css_class("flat");
                btn.set_child(Some(&icon_widget(&item)));
                root.append(&btn);
                buttons.insert(service, btn);
            }
        }
        SniEvent::ItemRemoved { service } => {
            if let Some(btn) = buttons.borrow_mut().remove(&service) {
                root.remove(&btn);
            }
        }
    }
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
