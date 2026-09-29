//! Reselect: remember the last selection so it can be restored after a job
//! or "Deselect All" cleared it.

use crate::window::Ui;
use gtk::prelude::*;
use gtk4 as gtk;
use std::rc::Rc;

/// Positions of `rows` whose name is in `names`.
pub fn matching_indices<'a>(names: &[String], rows: impl Iterator<Item = &'a str>) -> Vec<u32> {
    rows.enumerate()
        .filter(|(_, row)| names.iter().any(|n| n == row))
        .map(|(i, _)| i as u32)
        .collect()
}

impl Ui {
    /// Remember the current selection (when there is one) for [`reselect`](Self::reselect).
    pub fn remember_selection(&self) {
        let names = self.selected_names();
        if !names.is_empty() {
            *self.last_selection.borrow_mut() = names;
        }
    }

    /// Select again the entries of the remembered selection that are still listed.
    pub fn reselect(self: &Rc<Self>) {
        let names = self.last_selection.borrow().clone();
        if names.is_empty() {
            crate::actions::flash(self, "nothing to reselect");
            return;
        }
        let count = self.view.selection.n_items();
        let rows: Vec<String> = (0..count)
            .map(|i| self.row_at(i).map(|r| r.name.clone()).unwrap_or_default())
            .collect();
        let hits = matching_indices(&names, rows.iter().map(String::as_str));
        for i in &hits {
            self.view.selection.select_item(*i, false);
        }
        crate::actions::flash(self, &format!("{} item(s) reselected", hits.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_names_still_listed() {
        let names = vec!["b".to_string(), "gone".to_string(), "d".to_string()];
        let rows = ["a", "b", "c", "d"];
        assert_eq!(matching_indices(&names, rows.into_iter()), vec![1, 3]);
    }

    #[test]
    fn empty_memory_selects_nothing() {
        assert!(matching_indices(&[], ["a"].into_iter()).is_empty());
    }
}
