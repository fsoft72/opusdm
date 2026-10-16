//! "Set by this theme" wrapper: a section a theme may or may not provide.

use gtk4 as gtk;
use gtk::prelude::*;

/// A heading, a "Set by this theme" check box and the section content. The
/// content is greyed out while the box is off, and the theme then omits that
/// part from `theme.json`.
pub struct OptionalSection {
    pub root: gtk::Box,
    check: gtk::CheckButton,
}

impl OptionalSection {
    /// Wrap `content` under `title`; `is_set` is the initial state of the box.
    pub fn new(title: &str, content: &impl IsA<gtk::Widget>, is_set: bool) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = opusdm_ui::section_heading(title);
        let check = gtk::CheckButton::with_label("Set by this theme");
        check.set_active(is_set);
        let content: gtk::Widget = content.upcast_ref::<gtk::Widget>().clone();
        content.set_sensitive(is_set);
        {
            let content = content.clone();
            check.connect_toggled(move |c| content.set_sensitive(c.is_active()));
        }
        root.append(&heading);
        root.append(&check);
        root.append(&content);
        Self { root, check }
    }

    /// Whether the theme provides this section.
    pub fn is_set(&self) -> bool {
        self.check.is_active()
    }
}
