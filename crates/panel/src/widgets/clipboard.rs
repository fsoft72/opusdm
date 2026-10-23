//! The clipboard manager icon and its popover, in the right side of the
//! panel: a search box, the texts copied lately (a click puts one back on the
//! clipboard) and a button that forgets them all.

use crate::clipboard_center::ClipboardCenter;
use crate::clipboard_model::preview;
use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;

/// Size of the icon drawing in pixels.
const ICON_SIZE: i32 = 20;
/// Width of the popover, in pixels.
const POPOVER_WIDTH: i32 = 340;
/// The list scrolls above this height, in pixels.
const LIST_MAX_HEIGHT: i32 = 360;

/// The button and its popover.
pub struct ClipboardButton {
    root: gtk::Box,
    button: gtk::MenuButton,
    popover: gtk::Popover,
    search: gtk::SearchEntry,
    list: gtk::Box,
    clear: gtk::Button,
}

impl ClipboardButton {
    /// Build the button; `refresh` fills it.
    pub fn new(center: &Rc<ClipboardCenter>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        let icon = gtk::DrawingArea::new();
        icon.set_content_width(ICON_SIZE);
        icon.set_content_height(ICON_SIZE);
        icon.set_draw_func(draw_clipboard);
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_width_request(POPOVER_WIDTH);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search the clipboard history"));
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(LIST_MAX_HEIGHT);
        scroll.set_child(Some(&list));
        let clear = gtk::Button::with_label("Clear history");
        clear.add_css_class("flat");
        content.append(&search);
        content.append(&scroll);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&clear);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        button.set_popover(Some(&popover));
        root.append(&button);

        let this = Rc::new(Self { root, button, popover, search, list, clear });
        {
            // The handlers live in widgets this button owns, so they hold the
            // button weakly (a strong one would be a cycle) and the center
            // strongly (it lives as long as the panel). A failed upgrade
            // means the bar is being rebuilt: logged, not silent.
            let weak = Rc::downgrade(&this);
            let center = center.clone();
            this.search.connect_search_changed(move |_| match weak.upgrade() {
                Some(b) => b.show_list(&center),
                None => opusdm_ipc::err_log!("opusdm-panel", "clipboard manager: search changed on a button that is gone"),
            });
        }
        {
            let center = center.clone();
            this.clear.connect_clicked(move |_| center.clear());
        }
        {
            let search = this.search.clone();
            this.popover.connect_show(move |_| {
                search.set_text("");
                search.grab_focus();
            });
        }
        this
    }

    /// The widget to put in the bar.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Whether the widget is still inside a bar window.
    pub fn is_attached(&self) -> bool {
        self.root.parent().is_some()
    }

    /// Show the shared state.
    pub fn refresh(&self, center: &Rc<ClipboardCenter>) {
        let count = center.len();
        self.button.set_tooltip_text(Some(&match count {
            0 => "Clipboard: empty".to_string(),
            1 => "Clipboard: 1 entry".to_string(),
            n => format!("Clipboard: {n} entries"),
        }));
        self.clear.set_sensitive(count > 0);
        self.show_list(center);
    }

    /// Rebuild the rows that match the search box.
    fn show_list(&self, center: &Rc<ClipboardCenter>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let entries = center.entries(&self.search.text());
        if entries.is_empty() {
            let text = if center.len() == 0 { "Nothing copied yet" } else { "No match" };
            let empty = gtk::Label::new(Some(text));
            empty.add_css_class("dim-label");
            empty.set_margin_top(8);
            empty.set_margin_bottom(8);
            self.list.append(&empty);
            return;
        }
        for (index, text) in entries {
            self.list.append(&self.row(center, index, &text));
        }
    }

    /// One entry: a button that copies it again, and one that forgets it.
    fn row(&self, center: &Rc<ClipboardCenter>, index: usize, text: &str) -> gtk::Box {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let label = gtk::Label::new(Some(&preview(text)));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let pick = gtk::Button::new();
        pick.set_child(Some(&label));
        pick.set_hexpand(true);
        pick.add_css_class("flat");
        pick.set_tooltip_text(Some(&tooltip_text(text)));
        {
            let center = center.clone();
            let popover = self.popover.clone();
            pick.connect_clicked(move |_| {
                popover.popdown();
                center.select(index);
            });
        }
        let forget = gtk::Button::with_label("\u{d7}");
        forget.add_css_class("flat");
        forget.set_tooltip_text(Some("Remove from the history"));
        {
            let center = center.clone();
            forget.connect_clicked(move |_| center.remove(index));
        }
        line.append(&pick);
        line.append(&forget);
        line
    }
}

/// The start of the whole text, for the tooltip of a row.
fn tooltip_text(text: &str) -> String {
    const TOOLTIP_CHARS: usize = 400;
    if text.chars().count() <= TOOLTIP_CHARS { return text.to_string(); }
    let cut: String = text.chars().take(TOOLTIP_CHARS).collect();
    format!("{cut}...")
}

/// A clipboard in the text colour: the board, its clip and two lines of text.
fn draw_clipboard(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, h: i32) {
    let c = area.color();
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
    let (w, h) = (w as f64, h as f64);
    cr.set_line_width(1.6);
    cr.rectangle(w * 0.2, h * 0.16, w * 0.6, h * 0.76);
    let _ = cr.stroke();
    cr.rectangle(w * 0.36, h * 0.06, w * 0.28, h * 0.18);
    let _ = cr.fill();
    cr.set_line_width(1.2);
    for y in [0.42, 0.58, 0.74] {
        cr.move_to(w * 0.32, h * y);
        cr.line_to(w * 0.68, h * y);
        let _ = cr.stroke();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tooltip_cuts_a_long_text() {
        assert_eq!(tooltip_text("short"), "short");
        assert_eq!(tooltip_text(&"a".repeat(500)).chars().count(), 403);
    }
}
