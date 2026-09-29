//! Choosing and applying the Lister format (F-22) for the current directory.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::{FormatColumn, FormatSource, ListerFormat};
use std::path::Path;
use std::rc::Rc;

impl Ui {
    /// The format in effect for `dir`: a "Use" format for this very directory,
    /// else the saved one. Archive paths use the default, since no saved path
    /// can name them.
    pub fn effective_format(&self, dir: &Path) -> ListerFormat {
        if let Some((path, format)) = self.temp_format.borrow().as_ref() {
            if path == dir {
                return format.clone();
            }
        }
        let store = self.hub.formats.borrow();
        if crate::archive::parse_archive_path(dir).is_some() {
            return store.fallback();
        }
        store.resolve(dir)
    }

    /// Where the saved format for `dir` comes from (used to save a pattern
    /// change back to the same place).
    pub fn format_source(&self, dir: &Path) -> FormatSource {
        self.hub.formats.borrow().resolve_with_source(dir).1
    }

    /// Show `format`: column visibility and order, sort column and direction,
    /// directory separation, gauge.
    pub fn apply_format(&self, format: &ListerFormat) {
        let view = &self.view;
        let column_for = |wanted: FormatColumn| {
            view.col_map.iter().find(|(_, fc)| *fc == wanted).map(|(col, _)| col.clone())
        };
        // Detach every column, then re-attach the shown ones in order and the
        // hidden ones after them, so display order follows the format.
        for (col, _) in view.col_map.iter() {
            view.column_view.remove_column(col);
        }
        for fc in &format.columns {
            let Some(col) = column_for(*fc) else { continue };
            col.set_visible(true);
            view.column_view.append_column(&col);
        }
        for fc in format.hidden_columns() {
            let Some(col) = column_for(fc) else { continue };
            col.set_visible(false);
            view.column_view.append_column(&col);
        }
        view.separate.set(format.separate);
        let order = if format.reverse { gtk::SortType::Descending } else { gtk::SortType::Ascending };
        view.column_view.sort_by_column(column_for(format.sort).as_ref(), order);
        // The separation can change without the header state changing.
        view.model_sorter.changed(gtk::SorterChange::Different);
        *self.format.borrow_mut() = format.clone();
        self.update_gauge(&self.cwd_path());
    }

    /// Forget what is on screen so the next listing reapplies the format,
    /// even when it equals the previous one.
    pub fn invalidate_format(&self) {
        *self.format.borrow_mut() = ListerFormat::stale();
    }

    /// Apply `format` to the current directory only ("Use").
    pub fn use_format(self: &Rc<Self>, format: ListerFormat) {
        self.invalidate_format();
        *self.temp_format.borrow_mut() = Some((self.cwd_path(), format));
        self.reload();
    }

    /// The saved formats changed: reload when that changes the format of the
    /// current directory.
    pub fn on_formats_changed(self: &Rc<Self>) {
        let cwd = self.cwd_path();
        // Reloading reapplies the format, so an invalidated one always does.
        if self.effective_format(&cwd) != *self.format.borrow() {
            self.reload();
        }
    }
}
