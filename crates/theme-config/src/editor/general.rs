//! General tab: label, author, version.

use crate::writer::ThemeDraft;
use gtk4 as gtk;
use gtk::prelude::*;

/// The values of the General tab.
type General = (String, String, String);

/// Label, author and version entries.
pub struct GeneralTab {
    pub root: gtk::Box,
    label: gtk::Entry,
    author: gtk::Entry,
    version: gtk::Entry,
    initial: General,
}

/// `Some(text)` unless `text` is blank.
fn non_blank(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

impl GeneralTab {
    /// Build the tab seeded from `draft`.
    pub fn new(draft: &ThemeDraft) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_margin_top(12);
        root.set_margin_start(12);
        root.set_margin_end(12);
        let grid = gtk::Grid::builder().row_spacing(8).column_spacing(12).build();
        let make = |row: i32, caption: &str, text: &str| {
            let caption = gtk::Label::builder().label(caption).xalign(0.0).build();
            let entry = gtk::Entry::builder().text(text).hexpand(true).build();
            grid.attach(&caption, 0, row, 1, 1);
            grid.attach(&entry, 1, row, 1, 1);
            entry
        };
        let label = make(0, "Name", &draft.label);
        let author = make(1, "Author", draft.author.as_deref().unwrap_or(""));
        let version = make(2, "Version", draft.version.as_deref().unwrap_or(""));
        root.append(&grid);
        let mut tab = Self { root, label, author, version, initial: Default::default() };
        tab.initial = tab.read();
        tab
    }

    fn read(&self) -> General {
        (self.label.text().to_string(), self.author.text().to_string(), self.version.text().to_string())
    }

    /// Write the tab's values into `draft` when they changed.
    pub fn collect_into(&self, draft: &mut ThemeDraft) {
        let now = self.read();
        if now == self.initial {
            return;
        }
        draft.label = now.0;
        draft.author = non_blank(now.1);
        draft.version = non_blank(now.2);
    }
}
