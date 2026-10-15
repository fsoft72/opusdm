//! What the text documents (Markdown and source) share: the search marks, the
//! syntax colour tags and the dark / light look.

use crate::highlight::Category;
use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};

/// Shade of a search match and of the current one.
const MATCH_BG: &str = "#ffeb3c";
const CURRENT_MATCH_BG: &str = "#ff8c00";
const MATCH_FG: &str = "#000000";
/// Mark at the current match, to scroll to.
const SEARCH_MARK: &str = "search-match";


/// A text document the window can search and focus.
pub trait TextDoc {
    /// Mark every match of `query` (case ignored); returns how many there are.
    fn search(&self, query: &str) -> usize;
    /// Move to the next (or previous) match; `None` without matches.
    fn step(&self, forward: bool) -> Option<()>;
    /// `3/17`, or `no match`, for the search bar.
    fn label(&self) -> String;
    /// Give the keyboard focus to the text.
    fn focus(&self);
}

/// Create the `syn-*` tags of every `Category`, light coloured. Call before
/// `create_match_tags`, so the search marks win over the colours.
pub fn create_syntax_tags(buffer: &gtk::TextBuffer) {
    for category in Category::ALL {
        buffer.create_tag(Some(category.tag_name()), &[]);
    }
    color_syntax_tags(buffer, false);
}

/// Give the `syn-*` tags the colours of a dark or a light page.
pub fn color_syntax_tags(buffer: &gtk::TextBuffer, dark: bool) {
    let colors = if dark { &crate::highlight::DARK_COLORS } else { &crate::highlight::LIGHT_COLORS };
    let table = buffer.tag_table();
    for (category, color) in colors {
        let Some(tag) = table.lookup(category.tag_name()) else { continue };
        tag.set_foreground(Some(color));
    }
}

/// Create the tags of the search marks; created last, they win over the others.
pub fn create_match_tags(buffer: &gtk::TextBuffer) {
    buffer.create_tag(Some("match"), &[("background", &MATCH_BG), ("foreground", &MATCH_FG)]);
    buffer.create_tag(Some("current-match"), &[("background", &CURRENT_MATCH_BG), ("foreground", &MATCH_FG)]);
}

/// Dark or light colours for `widgets` (the document only; the window keeps
/// its own) and for the syntax tags of `buffer`.
pub fn set_doc_dark(widgets: &[&gtk::Widget], buffer: &gtk::TextBuffer, dark: bool) {
    let (on, off) = if dark { ("viewer-doc-dark", "viewer-doc-light") } else { ("viewer-doc-light", "viewer-doc-dark") };
    for widget in widgets {
        widget.remove_css_class(off);
        widget.add_css_class(on);
    }
    color_syntax_tags(buffer, dark);
}

/// The matches of the last search in a text view and the current one.
#[derive(Default)]
pub struct TextSearch {
    /// Character ranges of the matches.
    matches: RefCell<Vec<(i32, i32)>>,
    current: Cell<usize>,
}

impl TextSearch {
    /// Forget the matches without touching the buffer (its text is replaced).
    pub fn forget(&self) {
        self.matches.borrow_mut().clear();
        self.current.set(0);
    }

    /// Mark every match of `query` in `view` (case ignored) and make the first
    /// one the current one; returns how many there are. An empty query clears the marks.
    pub fn run(&self, view: &gtk::TextView, query: &str) -> usize {
        let buffer = view.buffer();
        let (start, end) = buffer.bounds();
        buffer.remove_tag_by_name("match", &start, &end);
        buffer.remove_tag_by_name("current-match", &start, &end);
        self.forget();
        if query.is_empty() { return 0; }
        let mut found = Vec::new();
        let mut from = buffer.start_iter();
        while let Some((start, end)) = from.forward_search(query, gtk::TextSearchFlags::CASE_INSENSITIVE | gtk::TextSearchFlags::VISIBLE_ONLY, None) {
            buffer.apply_tag_by_name("match", &start, &end);
            found.push((start.offset(), end.offset()));
            from = end;
        }
        *self.matches.borrow_mut() = found;
        self._mark_current(view);
        self.matches.borrow().len()
    }

    /// Move to the next (or previous) match, wrapping around; `None` without matches.
    pub fn step(&self, view: &gtk::TextView, forward: bool) -> Option<()> {
        let count = self.matches.borrow().len();
        if count == 0 { return None; }
        if let Some((start, end)) = self._current_range(view) { view.buffer().remove_tag_by_name("current-match", &start, &end); }
        let i = self.current.get();
        self.current.set(if forward { (i + 1) % count } else { (i + count - 1) % count });
        self._mark_current(view);
        Some(())
    }

    /// `3/17`, or `no match`, for the search bar.
    pub fn label(&self) -> String {
        let count = self.matches.borrow().len();
        if count == 0 { "no match".to_string() } else { format!("{}/{count}", self.current.get() + 1) }
    }

    /// Range of the current match as buffer iterators.
    fn _current_range(&self, view: &gtk::TextView) -> Option<(gtk::TextIter, gtk::TextIter)> {
        let (a, b) = *self.matches.borrow().get(self.current.get())?;
        let buffer = view.buffer();
        Some((buffer.iter_at_offset(a), buffer.iter_at_offset(b)))
    }

    /// Draw the current match in its own colour and scroll to it, a third down
    /// the page. A long line is not scrolled sideways when the match fits in
    /// the page from its left edge.
    fn _mark_current(&self, view: &gtk::TextView) {
        let Some((start, end)) = self._current_range(view) else { return };
        let buffer = view.buffer();
        buffer.apply_tag_by_name("current-match", &start, &end);
        let page = f64::from(view.visible_rect().width().max(1));
        let (start_x, end_x) = (f64::from(view.iter_location(&start).x()), f64::from(view.iter_location(&end).x()));
        // With `xalign` = start_x / page the page starts at x 0.
        let xalign = if end_x < page { (start_x / page).clamp(0.0, 1.0) } else { 0.3 };
        let mark = match buffer.mark(SEARCH_MARK) {
            Some(mark) => {
                buffer.move_mark(&mark, &start);
                mark
            }
            None => buffer.create_mark(Some(SEARCH_MARK), &start, true),
        };
        view.scroll_to_mark(&mark, 0.0, true, xalign, 0.3);
    }
}
