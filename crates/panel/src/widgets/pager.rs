//! Virtual-desktop pager: one numbered button per desktop, the current one
//! highlighted, click switches. Names from `_NET_DESKTOP_NAMES` become
//! tooltips.

use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};

use crate::ewmh;

/// The pager widget and its live button list.
pub struct Pager {
    root: gtk::Box,
    buttons: RefCell<Vec<gtk::Button>>,
    count: Cell<u32>,
}

impl Pager {
    /// Build an empty pager. Call [`Pager::update`] with the first snapshot.
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        root.add_css_class("opusdm-panel-pager");
        Pager { root, buttons: RefCell::new(Vec::new()), count: Cell::new(0) }
    }

    /// The GTK widget to pack into the panel.
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Apply a desktop snapshot: rebuild the buttons when the count changed,
    /// otherwise just move the highlight and refresh tooltips.
    pub fn update(&self, count: u32, current: u32, names: &[String]) {
        if count != self.count.get() {
            self.rebuild(count);
        }
        for (i, btn) in self.buttons.borrow().iter().enumerate() {
            let i = i as u32;
            btn.set_tooltip_text(names.get(i as usize).map(|s| s.as_str()));
            if i == current {
                btn.add_css_class("active");
            } else {
                btn.remove_css_class("active");
            }
        }
    }

    /// Replace every button with a fresh `1..=count` row.
    fn rebuild(&self, count: u32) {
        let mut buttons = self.buttons.borrow_mut();
        for b in buttons.drain(..) {
            self.root.remove(&b);
        }
        for i in 0..count {
            let btn = gtk::Button::with_label(&format!("{}", i + 1));
            btn.add_css_class("flat");
            btn.connect_clicked(move |_| ewmh::switch_desktop(i));
            self.root.append(&btn);
            buttons.push(btn);
        }
        self.count.set(count);
    }
}

impl Default for Pager {
    fn default() -> Self {
        Self::new()
    }
}
