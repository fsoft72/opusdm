//! Window list for the current desktop: icon + title per window, click to
//! activate, click the active window to minimise it. Rebuilt wholesale on
//! every snapshot - the window count is small.

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;

use crate::ewmh::{self, ClientWin};

/// Longest title shown before ellipsis.
const TITLE_CHARS: i32 = 22;

/// The window-list widget.
pub struct Tasklist {
    root: gtk::Box,
}

impl Tasklist {
    /// Build an empty, horizontally expanding list.
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        root.add_css_class("opusdm-panel-tasklist");
        root.set_hexpand(true);
        Tasklist { root }
    }

    /// The GTK widget to pack into the panel.
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Rebuild the buttons for every window on `current_desktop` (plus
    /// omnipresent windows), styling the active and minimised ones.
    pub fn update(&self, clients: &[ClientWin], current_desktop: u32, active: Option<u32>) {
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }
        for c in clients {
            if c.desktop != -1 && c.desktop != current_desktop as i64 {
                continue;
            }
            self.root.append(&self.button(c, active == Some(c.xid)));
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
