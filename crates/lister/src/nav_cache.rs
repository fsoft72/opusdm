//! Navigation with the directory cache: show the cached listing at once, then
//! re-read the disk and update the view only if it changed.

use crate::model::{read_dir_rows, Row};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::ToHub;
use std::io;
use std::path::Path;
use std::rc::Rc;

impl Ui {
    /// Go to `dir`. With `use_cache` a cached listing is shown immediately;
    /// the disk is read either way, on a thread, and the result replaces the
    /// view only when it differs from what is shown. `on_done` runs once the
    /// final listing is on screen.
    pub fn navigate(self: &Rc<Self>, dir: &Path, use_cache: bool, on_done: impl Fn() + 'static) {
        let history_before = self.history.borrow().clone();
        self.navigate_from(dir, use_cache, history_before, on_done);
    }

    /// [`Ui::navigate`] with the history as it was before the caller moved
    /// its position (back / forward / menu move it before loading). If the
    /// directory cannot be read that history is put back, so the buttons keep
    /// matching what is on screen.
    fn navigate_from(
        self: &Rc<Self>,
        dir: &Path,
        use_cache: bool,
        history_before: crate::history::History,
        on_done: impl Fn() + 'static,
    ) {
        crate::dlog!("navigate {} (cache: {use_cache})", dir.display());
        // A remote folder needs a connection first; the attempt is repeated
        // once it exists. Nothing changes if the user gives up.
        let remote_url = crate::remote::url_of(dir);
        if let Some(url) = remote_url.as_ref().filter(|u| self.remote_session(u).is_none()) {
            let target = dir.to_path_buf();
            self.ensure_remote(url, move |ui| ui.navigate_from(&target, false, history_before, on_done));
            return;
        }
        let previous = self.cwd_path();
        let gen = self.nav_generation.get() + 1;
        self.nav_generation.set(gen);

        let is_archive = crate::archive::parse_archive_path(dir).is_some() || remote_url.is_some();
        let cached = if use_cache && !is_archive { self.dir_cache.borrow_mut().get(dir) } else { None };
        if let Some(rows) = &cached {
            self.apply_navigation(dir, Ok((**rows).clone()));
        }

        let dir_owned = dir.to_path_buf();
        let session = remote_url.as_ref().and_then(|u| self.remote_session(u));
        let (tx, rx) = async_channel::bounded::<io::Result<Vec<Row>>>(1);
        std::thread::spawn(move || {
            let result = if let (Some(url), Some(session)) = (crate::remote::url_of(&dir_owned), session.as_ref()) {
                crate::remote_ui::read_rows(session, &url)
            } else if let Some((arch, inner)) = crate::archive::parse_archive_path(&dir_owned) {
                crate::archive::read_archive_rows(&arch, &inner)
            } else {
                read_dir_rows(&dir_owned)
            };
            let _ = tx.send_blocking(result);
        });

        let this = self.clone();
        let dir_owned = dir.to_path_buf();
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            // Superseded by a later navigation while this was loading.
            if this.nav_generation.get() != gen {
                return;
            }
            // A failed read leaves the previous listing on screen; callers that
            // act on the new listing (select an entry, restore a selection)
            // must not run against the wrong folder.
            if this.finish_listing(&dir_owned, result, cached, previous, history_before) {
                on_done();
            }
        });
    }

    /// Handle the disk read that follows a navigation. `cached` is the
    /// listing already shown for it, if any. False when the read failed.
    fn finish_listing(
        self: &Rc<Self>,
        dir: &Path,
        result: io::Result<Vec<Row>>,
        cached: Option<Rc<Vec<Row>>>,
        previous: std::path::PathBuf,
        history_before: crate::history::History,
    ) -> bool {
        let remote_url = crate::remote::url_of(dir);
        let is_archive = crate::archive::parse_archive_path(dir).is_some() || remote_url.is_some();
        let rows = match result {
            Ok(rows) => rows,
            Err(e) => {
                // A failed remote listing usually means a dead connection:
                // forget it so the next visit connects again.
                if let Some(url) = &remote_url {
                    self.drop_remote_session(url);
                }
                // The directory is gone or unreadable: forget any copy of it.
                if !is_archive && self.dir_cache.borrow_mut().remove(dir) {
                    self.report_cache();
                }
                self.apply_navigation(dir, Err(e));
                // The move never happened: undo its history step, and if the
                // cached rows were already shown, go back where we were.
                *self.history.borrow_mut() = history_before;
                self.refresh_history_ui();
                if cached.is_some() && previous != dir {
                    self.navigate_to(&previous);
                }
                return false;
            }
        };
        if cached.as_ref().is_some_and(|c| crate::dircache::same_listing(c, &rows)) {
            return true;
        }
        // Keep the user's selection when the fresh listing replaces a cached one.
        let selected = if cached.is_some() { self.selected_names() } else { Vec::new() };
        if !is_archive {
            self.dir_cache.borrow_mut().put(dir.to_path_buf(), rows.clone());
            self.report_cache();
        }
        self.apply_navigation(dir, Ok(rows));
        self.select_names(&selected);
        true
    }

    /// Select the rows called `names`, adding to the current selection.
    pub fn select_names(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        for i in 0..self.view.selection.n_items() {
            if self.row_at(i).is_some_and(|row| names.contains(&row.name)) {
                self.view.selection.select_item(i, false);
            }
        }
    }

    /// Tell the hub which directories this Lister has cached.
    pub fn report_cache(&self) {
        let paths = self.dir_cache.borrow().paths();
        let _ = self.hub.client.send(&ToHub::CacheReport(paths));
    }

    /// Drop the whole cache and tell the hub.
    pub fn free_caches(&self) {
        self.dir_cache.borrow_mut().clear();
        self.size_cache.borrow_mut().clear();
        self.report_cache();
    }

    /// Go to the previous history entry.
    pub fn history_back(self: &Rc<Self>) {
        let before = self.history.borrow().clone();
        let target = self.history.borrow_mut().back();
        if let Some(dir) = target {
            self.navigate_from(&dir, true, before, || {});
        }
    }

    /// Go to the next history entry.
    pub fn history_forward(self: &Rc<Self>) {
        let before = self.history.borrow().clone();
        let target = self.history.borrow_mut().forward();
        if let Some(dir) = target {
            self.navigate_from(&dir, true, before, || {});
        }
    }

    /// Go to history entry `index`.
    pub fn history_jump(self: &Rc<Self>, index: usize) {
        let before = self.history.borrow().clone();
        let target = self.history.borrow_mut().jump(index);
        if let Some(dir) = target {
            self.navigate_from(&dir, true, before, || {});
        }
    }

    /// Enable / disable the buttons and rebuild the menu rows (newest first,
    /// the current entry in bold).
    pub fn refresh_history_ui(&self) {
        use gtk::prelude::*;
        let history = self.history.borrow();
        self.nav_history.back.set_sensitive(history.can_back());
        self.nav_history.forward.set_sensitive(history.can_forward());
        let list = &self.nav_history.list;
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        let current = history.index();
        for (i, path) in history.entries().iter().enumerate().rev() {
            let label = gtk::Label::new(None);
            label.set_xalign(0.0);
            let text = glib::markup_escape_text(&path.to_string_lossy());
            if Some(i) == current {
                label.set_markup(&format!("<b>{text}</b>"));
            } else {
                label.set_markup(&text);
            }
            list.append(&label);
        }
    }
}

/// Wire the back / forward buttons and the history menu.
pub fn install_history(ui: &Rc<Ui>, popover: &gtk::Popover) {
    use gtk::prelude::*;
    {
        let ui = ui.clone();
        ui.clone().nav_history.back.connect_clicked(move |_| ui.history_back());
    }
    {
        let ui = ui.clone();
        ui.clone().nav_history.forward.connect_clicked(move |_| ui.history_forward());
    }
    {
        let ui_for_click = ui.clone();
        let popover = popover.clone();
        ui.nav_history.list.connect_row_activated(move |_, row| {
            let len = ui_for_click.history.borrow().entries().len();
            // Rows are listed newest first.
            let Some(index) = len.checked_sub(1 + row.index() as usize) else { return };
            popover.popdown();
            ui_for_click.history_jump(index);
        });
    }
    ui.refresh_history_ui();
}
