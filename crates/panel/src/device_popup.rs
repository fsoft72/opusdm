//! The popup that offers to mount a device when it is plugged in.

use crate::devices_center::{DeviceCenter, Op};
use crate::widgets::devices::{row_title};
use crate::window::PANEL_HEIGHT;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_devices::Device;
use opusdm_ipc::PanelPosition;
use std::cell::RefCell;
use std::rc::Rc;

/// Seconds before the popup closes by itself.
const TIMEOUT_SECS: u32 = 10;
/// Fixed size of the popup window.
const POPUP_W: i32 = 360;
const POPUP_H: i32 = 110;
/// Gap between the popup and the screen edge and the panel.
const MARGIN: i32 = 8;
/// Title of the popup window.
const POPUP_TITLE: &str = "OpusDM Device";

thread_local! {
    /// The popup on screen: the volume it is about and its window.
    static CURRENT: RefCell<Option<(String, gtk::Window)>> = const { RefCell::new(None) };
}

/// Top left corner of a popup of `size` in the corner of monitor `mon`
/// (x, y, w, h) that is next to the panel, below it or above it.
pub fn popup_position(mon: (i32, i32, i32, i32), panel_top: bool, size: (i32, i32)) -> (i32, i32) {
    let (mx, my, mw, mh) = mon;
    let x = mx + mw - size.0 - MARGIN;
    let y = if panel_top { my + PANEL_HEIGHT + MARGIN } else { my + mh - PANEL_HEIGHT - size.1 - MARGIN };
    (x, y)
}

/// Open the popup for `device`, closing any popup that is already open.
pub fn show(center: &Rc<DeviceCenter>, device: &Device) {
    close();
    let window = gtk::Window::builder()
        .title(POPUP_TITLE)
        .decorated(false)
        .resizable(false)
        .default_width(POPUP_W)
        .default_height(POPUP_H)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.add_css_class("opusdm-panel");
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    let title = gtk::Label::new(Some(&row_title(device)));
    title.set_xalign(0.0);
    title.add_css_class("heading");
    let sub = gtk::Label::new(Some(&format!("New device {}", device.dev_node)));
    sub.set_xalign(0.0);
    sub.add_css_class("dim-label");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_vexpand(true);
    buttons.set_valign(gtk::Align::End);
    let ignore = gtk::Button::with_label("Ignore");
    let mount = gtk::Button::with_label("Mount");
    let open = gtk::Button::with_label("Mount and open");
    buttons.append(&ignore);
    buttons.append(&mount);
    buttons.append(&open);
    content.append(&title);
    content.append(&sub);
    content.append(&buttons);
    window.set_child(Some(&content));

    {
        let window = window.clone();
        ignore.connect_clicked(move |_| window.close());
    }
    for (button, with_open) in [(&mount, false), (&open, true)] {
        let (center, path, window) = (center.clone(), device.path.clone(), window.clone());
        button.connect_clicked(move |_| {
            center.run(if with_open { Op::MountAndOpen(path.clone()) } else { Op::Mount(path.clone()) });
            window.close();
        });
    }
    {
        let window = window.clone();
        gtk::glib::timeout_add_seconds_local_once(TIMEOUT_SECS, move || window.close());
    }

    let panel_top = center.position() != PanelPosition::Bottom;
    window.connect_map(move |w| {
        let w = w.clone();
        opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || place(&w, panel_top));
    });
    window.connect_close_request(|w| {
        CURRENT.with(|c| {
            let mut c = c.borrow_mut();
            if c.as_ref().is_some_and(|(_, cur)| cur == w) { *c = None; }
        });
        gtk::glib::Propagation::Proceed
    });
    CURRENT.with(|c| *c.borrow_mut() = Some((device.path.clone(), window.clone())));
    window.present();
}

/// Close the popup of a device that was unplugged or mounted meanwhile.
pub fn sync(center: &Rc<DeviceCenter>) {
    let stale = CURRENT.with(|c| c.borrow().as_ref().filter(|(p, _)| !center.is_pending(p)).map(|(_, w)| w.clone()));
    if let Some(w) = stale { w.close(); }
}

/// Close the popup, if any.
fn close() {
    let current = CURRENT.with(|c| c.borrow_mut().take());
    if let Some((_, w)) = current { w.close(); }
}

/// Put the mapped popup in its corner and above the other windows.
fn place(window: &gtk::Window, panel_top: bool) {
    let Some(surface) = window.surface() else { return };
    let Ok(x11_surface) = surface.downcast::<gdk4_x11::X11Surface>() else { return };
    let xid = x11_surface.xid() as u32;
    let mon = gtk::gdk::Display::default()
        .as_ref()
        .map(opusdm_ui::list_monitors)
        .and_then(|m| m.first().copied())
        .unwrap_or((0, 0, 1280, 800));
    let (x, y) = popup_position(mon, panel_top, (POPUP_W, POPUP_H));
    let res = opusdm_ui::with_x11(|x11| {
        x11.pin_geometry(xid, x, y, POPUP_W as u32, POPUP_H as u32)?;
        x11.raise_above(xid)
    });
    if let Err(e) = res {
        opusdm_ipc::err_log!("opusdm-panel", "cannot place the device popup: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_sits_in_the_corner_next_to_the_panel() {
        let mon = (0, 0, 1920, 1080);
        assert_eq!(popup_position(mon, true, (360, 110)), (1920 - 360 - MARGIN, PANEL_HEIGHT + MARGIN));
        assert_eq!(popup_position(mon, false, (360, 110)), (1920 - 360 - MARGIN, 1080 - PANEL_HEIGHT - 110 - MARGIN));
        assert_eq!(popup_position((1920, 0, 1280, 800), true, (360, 110)).0, 1920 + 1280 - 360 - MARGIN);
    }

    #[test]
    fn popup_closes_after_ten_seconds() {
        assert_eq!(TIMEOUT_SECS, 10);
    }
}
