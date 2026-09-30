//! One icon-view item: an icon image (or thumbnail) above an editable label.

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
    icon: gtk::Label,
    thumb: gtk::Image,
    name_stack: gtk::Stack,
    name_label: gtk::Label,
    name_entry: gtk::Entry,
    original_name: RefCell<String>,
    on_commit: CommitFn,
    /// The left-out arrow badge, when the entry is a left-out desktop icon.
    badge: Option<gtk::Label>,
}

/// A single item widget. Cheap to clone (shared `Rc`).
#[derive(Clone)]
pub struct IconItem {
    inner: Rc<Inner>,
}

impl IconItem {
    /// Build an item for `entry`, icon box `icon_size` px square, under icon
    /// set `icon_set` (the themed icon image; a blank slot when the theme has
    /// no icon for this entry).
    pub fn new(entry: &IconEntry, icon_size: i32, icon_set: &opusdm_ipc::IconSetId) -> IconItem {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("icon-item");
        root.set_halign(gtk::Align::Center);

        // Blank placeholder that keeps reserving its `icon_size` square
        // while no image is shown, same as every other item and as the
        // thumbnail slot - collapsing it would shrink this item alone and,
        // through `FreeArrange`'s max-of-all-items pitch, respace the grid.
        let icon = gtk::Label::new(None);
        icon.add_css_class("icon-item-glyph");
        icon.set_width_request(icon_size);
        icon.set_height_request(icon_size);

        // A `gtk::Image` sized to the icon box, not a `gtk::Picture`: a Picture
        // reports the texture's intrinsic size as its natural size, so a tall
        // thumbnail (portrait photo, PDF page) would grow the item and, through
        // `FreeArrange`'s max-of-all-items pitch, space out the whole desktop
        // grid once thumbnails finish loading. `Image` always measures
        // `pixel_size` square and scales the paintable to fit.
        let thumb = gtk::Image::new();
        thumb.set_pixel_size(icon_size);
        thumb.set_visible(false);

        // A themed icon set fills the same `thumb` slot a file-content
        // thumbnail later replaces via `set_thumbnail` - a real preview should
        // win over a generic per-type icon. `clear_thumbnail` (only called on
        // a thumbnail-size-bucket change) reverts to the blank slot rather
        // than the themed icon; that rare resize path is an accepted
        // limitation, not a regression.
        if let Some(bytes) =
            crate::icon_set::icon_bytes_for(entry.kind, entry.mode, entry.ext.as_deref(), icon_set)
        {
            let gbytes = glib::Bytes::from_owned(bytes.into_owned());
            if let Ok(texture) = gtk::gdk::Texture::from_bytes(&gbytes) {
                thumb.set_paintable(Some(&texture));
                thumb.set_pixel_size(icon_size);
                thumb.set_visible(true);
                icon.set_visible(false);
            }
        }

        let name_label = gtk::Label::new(Some(&entry.name));
        name_label.set_wrap(true);
        // Break inside a long token too: a spaceless, hyphenless filename is one
        // unbreakable word under the default `Word` mode, so `max_width_chars`
        // is ignored and the label reports its full single-line width. That one
        // wide item then stretches every column of the auto-slotted grid.
        name_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        name_label.set_justify(gtk::Justification::Center);
        name_label.set_max_width_chars(14);
        name_label.set_lines(2);
        name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        // The entry is added to the stack only while editing (see `start_edit`).
        // A `gtk::Entry` always requests room for `max_width_chars`, and a
        // `gtk::Stack` measures the largest of all its children whether shown or
        // not, so a permanently-parented entry would widen every item to ~130px
        // regardless of its label. Through `FreeArrange`'s max-of-all-items
        // pitch that spaces the whole desktop grid out.
        let name_entry = gtk::Entry::new();
        name_entry.set_max_width_chars(14);

        let name_stack = gtk::Stack::new();
        name_stack.add_named(&name_label, Some("label"));
        name_stack.set_visible_child_name("label");

        // A left-out entry wraps the icon art in an overlay that carries the
        // arrow badge; an overlay child never changes the measured size.
        let badge = if entry.left_out {
            let art = gtk::Box::new(gtk::Orientation::Vertical, 0);
            art.append(&icon);
            art.append(&thumb);
            let badge = gtk::Label::new(Some("\u{2197}"));
            badge.add_css_class("icon-item-badge");
            badge.set_halign(gtk::Align::End);
            badge.set_valign(gtk::Align::End);
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&art));
            overlay.add_overlay(&badge);
            root.append(&overlay);
            Some(badge)
        } else {
            root.append(&icon);
            root.append(&thumb);
            None
        };
        root.append(&name_stack);

        let inner = Rc::new(Inner {
            root,
            icon,
            thumb,
            name_stack,
            name_label,
            name_entry,
            original_name: RefCell::new(entry.name.clone()),
            on_commit: RefCell::new(None),
            badge,
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
        // Drop the entry back out of the layout so it stops padding the item.
        if self.inner.name_stack.child_by_name("entry").is_some() {
            self.inner.name_stack.remove(&self.inner.name_entry);
        }
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

    /// Resize the icon box (placeholder and thumbnail).
    pub fn set_icon_size(&self, px: i32) {
        self.inner.icon.set_width_request(px);
        self.inner.icon.set_height_request(px);
        self.inner.thumb.set_pixel_size(px);
    }

    /// Whether the icon box is currently showing an image (real icon-set icon
    /// or content thumbnail) rather than the blank placeholder.
    pub fn shows_image(&self) -> bool {
        self.inner.thumb.is_visible()
    }

    /// Whether the left-out arrow badge is drawn on this item.
    pub fn shows_badge(&self) -> bool {
        self.inner.badge.is_some()
    }

    /// Show `texture` in place of the placeholder.
    pub fn set_thumbnail(&self, texture: &gtk::gdk::Texture) {
        self.inner.thumb.set_paintable(Some(texture));
        self.inner.thumb.set_visible(true);
        self.inner.icon.set_visible(false);
    }

    /// Drop any thumbnail and show the placeholder again.
    pub fn clear_thumbnail(&self) {
        self.inner.thumb.set_paintable(gtk::gdk::Paintable::NONE);
        self.inner.thumb.set_visible(false);
        self.inner.icon.set_visible(true);
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
        // Attach the entry on demand; it lives outside the stack when idle.
        if self.inner.name_stack.child_by_name("entry").is_none() {
            self.inner.name_stack.add_named(&self.inner.name_entry, Some("entry"));
        }
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
