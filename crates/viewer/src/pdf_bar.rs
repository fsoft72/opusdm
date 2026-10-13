//! The bar over a PDF: the button that opens and closes the page panel and the
//! search field with its previous / next buttons and the match count.

use gtk4 as gtk;
use gtk::prelude::*;

/// Widgets of the bar; the window connects their signals.
pub struct PdfBar {
    pub root: gtk::Box,
    pub pages: gtk::ToggleButton,
    pub entry: gtk::SearchEntry,
    pub prev: gtk::Button,
    pub next: gtk::Button,
    pub status: gtk::Label,
    /// Dark / light switch of a text document; shown for Markdown only.
    pub dark_box: gtk::Box,
    pub dark: gtk::Switch,
}

impl PdfBar {
    /// Build the bar, hidden until a PDF shows.
    pub fn new() -> PdfBar {
        let pages = gtk::ToggleButton::with_label("Pages");
        pages.set_focusable(false);
        pages.set_tooltip_text(Some("Show or hide the page panel (F9)"));
        let entry = gtk::SearchEntry::new();
        entry.set_placeholder_text(Some("Find in document (Ctrl+F)"));
        entry.set_width_chars(28);
        let prev = gtk::Button::from_icon_name("go-up-symbolic");
        prev.set_focusable(false);
        prev.set_tooltip_text(Some("Previous match (Shift+Enter, Shift+F3)"));
        let next = gtk::Button::from_icon_name("go-down-symbolic");
        next.set_focusable(false);
        next.set_tooltip_text(Some("Next match (Enter, F3)"));
        let status = gtk::Label::new(None);
        status.set_width_chars(10);
        status.set_xalign(0.0);
        let dark = gtk::Switch::new();
        dark.set_focusable(false);
        dark.set_valign(gtk::Align::Center);
        dark.set_tooltip_text(Some("Dark or light document (the window is not changed)"));
        let dark_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        dark_box.append(&gtk::Label::new(Some("Dark")));
        dark_box.append(&dark);
        dark_box.set_visible(false);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        root.add_css_class("viewer-pdf-bar");
        for widget in [pages.upcast_ref::<gtk::Widget>(), spacer.upcast_ref(), dark_box.upcast_ref(), entry.upcast_ref(), prev.upcast_ref(), next.upcast_ref(), status.upcast_ref()] {
            root.append(widget);
        }
        root.set_visible(false);
        PdfBar { root, pages, entry, prev, next, status, dark_box, dark }
    }

    /// True while the keyboard focus is in the search field.
    pub fn entry_has_focus(&self, window: &gtk::ApplicationWindow) -> bool {
        gtk::prelude::GtkWindowExt::focus(window).is_some_and(|w| w == *self.entry.upcast_ref::<gtk::Widget>() || w.is_ancestor(&self.entry))
    }
}
