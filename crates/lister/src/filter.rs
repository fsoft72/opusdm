//! Instant fuzzy filter for the active Lister view.

use crate::model::Row;
use gtk4 as gtk;
use gtk::glib::BoxedAnyObject;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Check whether `needle` fuzzy-matches `haystack` (case-insensitive).
/// A match occurs if needle is a substring or if all characters of needle
/// appear in order within haystack.
pub fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    let haystack = haystack.to_lowercase();
    if haystack.contains(&needle) {
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
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let text = text.to_lowercase();
    let p_chars: Vec<char> = pattern.chars().collect();
    let t_chars: Vec<char> = text.chars().collect();
    let (p_len, t_len) = (p_chars.len(), t_chars.len());
    let mut dp = vec![vec![false; t_len + 1]; p_len + 1];
    dp[0][0] = true;
    for i in 1..=p_len {
        if p_chars[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=p_len {
        for j in 1..=t_len {
            if p_chars[i - 1] == '*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if p_chars[i - 1] == '?' || p_chars[i - 1] == t_chars[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }
    dp[p_len][t_len]
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
            let q = q_clone.borrow();
            if q.is_empty() {
                return true;
            }
            if let Some(boxed) = obj.downcast_ref::<BoxedAnyObject>() {
                let row = boxed.borrow::<Rc<Row>>();
                fuzzy_match(&q, &row.name)
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
}
