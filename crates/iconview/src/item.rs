//! One icon-view item: a glyph (phase 2: a thumbnail) above an editable label.

use crate::glyphs::glyph_for;
use crate::types::IconEntry;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Optional rename callback: called with the committed new name.
type CommitFn = RefCell<Option<Box<dyn Fn(String)>>>;

struct Inner {
    root: gtk::Box,
    icon: gtk::Label, // phase 2 swaps in a gtk::Picture
    name_stack: gtk::Stack,
    name_label: gtk::Label,
    name_entry: gtk::Entry,
    original_name: RefCell<String>,
    on_commit: CommitFn,
}

/// A single item widget. Cheap to clone (shared `Rc`).
#[derive(Clone)]
pub struct IconItem {
    inner: Rc<Inner>,
}

impl IconItem {
    /// Build an item for `entry`, icon box `icon_size` px square.
    pub fn new(entry: &IconEntry, icon_size: i32) -> IconItem {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("icon-item");
        root.set_halign(gtk::Align::Center);

        let icon = gtk::Label::new(Some(&glyph_for(entry.kind, entry.mode, &entry.name).to_string()));
        icon.add_css_class("icon-item-glyph");
        icon.set_width_request(icon_size);
        icon.set_height_request(icon_size);

        let name_label = gtk::Label::new(Some(&entry.name));
        name_label.set_wrap(true);
        name_label.set_justify(gtk::Justification::Center);
        name_label.set_max_width_chars(14);
        name_label.set_lines(2);
        name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let name_entry = gtk::Entry::new();
        name_entry.set_max_width_chars(14);

        let name_stack = gtk::Stack::new();
        name_stack.add_named(&name_label, Some("label"));
        name_stack.add_named(&name_entry, Some("entry"));
        name_stack.set_visible_child_name("label");

        root.append(&icon);
        root.append(&name_stack);

        let inner = Rc::new(Inner {
            root,
            icon,
            name_stack,
            name_label,
            name_entry,
            original_name: RefCell::new(entry.name.clone()),
            on_commit: RefCell::new(None),
        });

        let item = IconItem { inner };
        item.wire_editing();
        item
    }

    fn wire_editing(&self) {
        let this = self.clone();
        self.inner.name_entry.connect_activate(move |e| {
            this.finish_edit(Some(e.text().to_string()));
        });
        let this = self.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |_| {
            if this.inner.name_stack.visible_child_name().as_deref() == Some("entry") {
                let text = this.inner.name_entry.text().to_string();
                this.finish_edit(Some(text));
            }
        });
        self.inner.name_entry.add_controller(focus);
        let this = self.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                this.finish_edit(None);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.inner.name_entry.add_controller(key);
    }

    pub(crate) fn finish_edit(&self, new_text: Option<String>) {
        if self.inner.name_stack.visible_child_name().as_deref() != Some("entry") {
            return;
        }
        self.inner.name_stack.set_visible_child_name("label");
        let Some(text) = new_text else {
            return;
        };
        let changed = *self.inner.original_name.borrow() != text;
        if !changed || text.is_empty() {
            return;
        }
        self.inner.name_label.set_text(&text);
        *self.inner.original_name.borrow_mut() = text.clone();
        if let Some(cb) = self.inner.on_commit.borrow().as_ref() {
            cb(text);
        }
    }

    /// The root widget, to add to a parent container.
    pub fn widget(&self) -> &gtk::Widget {
        self.inner.root.upcast_ref()
    }

    /// Toggle the `selected` style class.
    pub fn set_selected(&self, on: bool) {
        toggle_class(&self.inner.root, "selected", on);
    }

    /// Toggle the `cursor` style class.
    pub fn set_cursor(&self, on: bool) {
        toggle_class(&self.inner.root, "cursor", on);
    }

    /// Resize the icon box.
    pub fn set_icon_size(&self, px: i32) {
        self.inner.icon.set_width_request(px);
        self.inner.icon.set_height_request(px);
    }

    /// Natural size in pixels, for the arranger.
    pub fn measure(&self) -> (f64, f64) {
        let (_, nat_w, _, _) = self.inner.root.measure(gtk::Orientation::Horizontal, -1);
        let (_, nat_h, _, _) = self.inner.root.measure(gtk::Orientation::Vertical, nat_w);
        (nat_w as f64, nat_h as f64)
    }

    /// Enter inline editing: show the entry, preselect the basename.
    pub fn start_edit(&self) {
        let name = self.inner.original_name.borrow().clone();
        self.inner.name_entry.set_text(&name);
        let split = name.rfind('.').filter(|&i| i > 0).unwrap_or(name.len());
        self.inner.name_entry.select_region(0, split as i32);
        self.inner.name_stack.set_visible_child_name("entry");
        self.inner.name_entry.grab_focus();
    }

    /// Register the rename callback.
    pub fn connect_edit_committed(&self, f: impl Fn(String) + 'static) {
        *self.inner.on_commit.borrow_mut() = Some(Box::new(f));
    }

    /// Current displayed name.
    pub fn name(&self) -> String {
        self.inner.original_name.borrow().clone()
    }
}

fn toggle_class(w: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        w.add_css_class(class);
    } else {
        w.remove_css_class(class);
    }
}
