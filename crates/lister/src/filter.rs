//! Instant fuzzy filter for the active Lister view.

use crate::model::Row;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib::BoxedAnyObject;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Check whether `needle` fuzzy-matches `haystack` (case-insensitive).
/// A match occurs if needle is a substring or if all characters of needle
/// appear in order within haystack.
pub fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let needle = needle.trim().to_lowercase();
    let haystack = haystack.to_lowercase();
    fuzzy_match_lower(&needle, &haystack)
}

/// Same as [`fuzzy_match`], but assumes both arguments are already
/// lowercased and trimmed. Used on the filter-bar hot path, where the
/// needle is lowercased once per keystroke and each row's haystack is
/// precomputed on `Row` (see `Row::name_lower`), instead of re-lowercasing
/// every row on every keystroke.
pub fn fuzzy_match_lower(needle: &str, haystack: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.contains(needle) {
        return true;
    }
    let mut needle_chars = needle.chars();
    let mut current = match needle_chars.next() {
        Some(c) => c,
        None => return true,
    };
    for h in haystack.chars() {
        if h == current {
            if let Some(next_c) = needle_chars.next() {
                current = next_c;
            } else {
                return true;
            }
        }
    }
    false
}

/// Simple glob matcher supporting `*` and `?` wildcards.
///
/// Two-pointer greedy match with backtracking to the most recent `*`
/// (the standard wildcard-matching algorithm): O(pattern + text) space,
/// instead of allocating a full DP matrix per call.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_match = 0usize;

    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some(pi);
            star_match = ti;
            pi += 1;
        } else if let Some(si) = star {
            pi = si + 1;
            star_match += 1;
            ti = star_match;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

/// The inline fuzzy filter bar widget.
pub struct FilterBar {
    pub container: gtk::Box,
    pub entry: gtk::SearchEntry,
    pub count_label: gtk::Label,
    pub filter: gtk::CustomFilter,
    pub query: Rc<RefCell<String>>,
}

impl FilterBar {
    pub fn new() -> Self {
        let query = Rc::new(RefCell::new(String::new()));
        let q_clone = query.clone();

        let filter = gtk::CustomFilter::new(move |obj| {
            // `query` is kept pre-lowercased and pre-trimmed (see build.rs's
            // `connect_search_changed`), so this hot path (once per row, per
            // keystroke) never re-lowercases the needle or the haystack.
            let q = q_clone.borrow();
            if q.is_empty() {
                return true;
            }
            if let Some(boxed) = obj.downcast_ref::<BoxedAnyObject>() {
                let row = boxed.borrow::<Rc<Row>>();
                fuzzy_match_lower(&q, &row.name_lower)
            } else {
                true
            }
        });

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        container.set_margin_start(4);
        container.set_margin_end(4);
        container.set_margin_top(2);
        container.set_margin_bottom(2);
        container.add_css_class("filter-bar");
        container.set_visible(false);

        let entry = gtk::SearchEntry::new();
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("Type to filter files (Esc to close)..."));

        let count_label = gtk::Label::new(None);
        count_label.add_css_class("dim-label");

        let close_btn = gtk::Button::with_label("✕");
        close_btn.set_tooltip_text(Some("Close filter (Esc)"));
        close_btn.add_css_class("flat");

        container.append(&entry);
        container.append(&count_label);
        container.append(&close_btn);

        {
            let container_clone = container.clone();
            let entry_clone = entry.clone();
            let q = query.clone();
            let filt = filter.clone();
            close_btn.connect_clicked(move |_| {
                entry_clone.set_text("");
                *q.borrow_mut() = String::new();
                filt.changed(gtk::FilterChange::Different);
                container_clone.set_visible(false);
            });
        }

        // Capture ESC directly on search entry so it closes immediately
        {
            let container_clone = container.clone();
            let entry_clone = entry.clone();
            let q = query.clone();
            let filt = filter.clone();

            let key_ctrl = gtk::EventControllerKey::new();
            key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
            key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
                if keyval == gdk::Key::Escape {
                    entry_clone.set_text("");
                    *q.borrow_mut() = String::new();
                    filt.changed(gtk::FilterChange::Different);
                    container_clone.set_visible(false);
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            });
            entry.add_controller(key_ctrl);

            let container_clone2 = container.clone();
            let entry_clone2 = entry.clone();
            let q2 = query.clone();
            let filt2 = filter.clone();
            entry.connect_stop_search(move |_| {
                entry_clone2.set_text("");
                *q2.borrow_mut() = String::new();
                filt2.changed(gtk::FilterChange::Different);
                container_clone2.set_visible(false);
            });
        }

        Self {
            container,
            entry,
            count_label,
            filter,
            query,
        }
    }

    /// Open and focus the filter bar.
    pub fn reveal(&self) {
        self.container.set_visible(true);
        self.entry.grab_focus();
    }

    /// Hide and reset the filter bar.
    pub fn dismiss(&self) {
        self.entry.set_text("");
        *self.query.borrow_mut() = String::new();
        self.filter.changed(gtk::FilterChange::Different);
        self.container.set_visible(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match() {
        assert!(fuzzy_match("", "anything"));
        assert!(fuzzy_match("abc", "abcdef"));
        assert!(fuzzy_match("adf", "abcdef"));
        assert!(fuzzy_match("rs", "src/main.rs"));
        assert!(fuzzy_match("crgo", "Cargo.toml"));
        assert!(fuzzy_match("WIND", "window.rs"));
        assert!(!fuzzy_match("rs", "Cargo.lock"));
        assert!(!fuzzy_match("xyz", "abcdef"));
        assert!(!fuzzy_match("ba", "abc"));
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.RS", "MAIN.rs"));
        assert!(glob_match("test_*", "test_foo.c"));
        assert!(glob_match("file?.txt", "file1.txt"));
        assert!(!glob_match("file?.txt", "file12.txt"));
        assert!(!glob_match("*.rs", "main.toml"));
    }

    /// Multiple stars force backtracking to the most recent one, the case
    /// the two-pointer rewrite has to get right.
    #[test]
    fn test_glob_match_multiple_stars_backtrack() {
        assert!(glob_match("*a*b*", "xaxbx"));
        assert!(glob_match("*a*b*", "ab"));
        assert!(!glob_match("*a*b*", "bx"));
        assert!(glob_match("a*b*c", "aXXbXXc"));
        assert!(!glob_match("a*b*c", "aXXbXX"));
        assert!(glob_match("**", "anything"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("a*b", "a"));
    }
}
