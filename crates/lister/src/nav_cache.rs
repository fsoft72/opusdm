//! Navigation with the directory cache: show the cached listing at once, then
//! re-read the disk and update the view only if it changed.

use crate::background::CancelToken;
use crate::model::{read_dir_rows, Listing, Row};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::ToHub;
use std::cell::Cell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// How long an archive read runs before the progress panel shows up.
const PROGRESS_SHOW_DELAY: Duration = Duration::from_millis(400);
/// How often the progress panel is refreshed while it is shown.
const PROGRESS_TICK: Duration = Duration::from_millis(100);

/// The panel message while an archive is read.
fn progress_message(name: &str, entries: u64) -> String {
    match entries {
        0 => format!("Opening {name}..."),
        1 => format!("Opening {name}: 1 entry read"),
        n => format!("Opening {name}: {n} entries read"),
    }
}

impl Ui {
    /// Go to `dir`. With `use_cache` a cached listing is shown immediately;
    /// the disk is read either way, on a thread, and the result replaces the
    /// view only when it differs from what is shown. `on_done` runs once the
    /// final listing is on screen.
    pub fn navigate(self: &Rc<Self>, dir: &Path, use_cache: bool, on_done: impl Fn() + 'static) {
        let history_before = self.nav.history.borrow().clone();
        self.navigate_from(dir, use_cache, false, history_before, on_done);
    }

    /// [`Ui::navigate`] with the history as it was before the caller moved
    /// its position (back / forward / menu move it before loading). If the
    /// directory cannot be read that history is put back, so the buttons keep
    /// matching what is on screen. With `restore` the folder comes back as
    /// it was when it was left (scroll, cursor, selection).
    pub(crate) fn navigate_from(
        self: &Rc<Self>,
        dir: &Path,
        use_cache: bool,
        restore: bool,
        history_before: crate::history::History,
        on_done: impl Fn() + 'static,
    ) {
        crate::dlog!("navigate {} (cache: {use_cache})", dir.display());
        // A remote folder needs a connection first; the attempt is repeated
        // once it exists. Nothing changes if the user gives up.
        let remote_url = crate::remote::url_of(dir);
        if let Some(url) = remote_url.as_ref().filter(|u| self.remote_session(u).is_none()) {
            let target = dir.to_path_buf();
            self.ensure_remote(url, move |ui| ui.navigate_from(&target, false, restore, history_before, on_done));
            return;
        }
        let previous = self.cwd_path();
        // Leaving a folder: remember where the user was in it. A reload of
        // the folder on screen is not leaving it.
        if previous != dir {
            self.save_view();
        }
        let gen = self.nav.generation.get() + 1;
        self.nav.generation.set(gen);
        // This navigation supersedes the archive read the panel shows, if any.
        self.progress.hide();

        let is_archive = crate::archive::parse_archive_path(dir).is_some() || remote_url.is_some();
        let cached = if use_cache && !is_archive { self.caches.dirs.borrow_mut().get(dir) } else { None };
        if let Some(rows) = &cached {
            self.apply_navigation(dir, Ok(rows.clone()));
            if restore {
                self.restore_view(dir);
            }
        }

        let dir_owned = dir.to_path_buf();
        let session = remote_url.as_ref().and_then(|u| self.remote_session(u));
        // The worker pool reuses its threads, and a read that was superseded
        // before it started is skipped through the cancel token.
        let cancel = crate::background::CancelToken::new();
        self.nav.cancel.replace(cancel.clone()).cancel();
        let progress = Arc::new(crate::archive::ReadProgress::default());
        let worker_progress = progress.clone();
        let read = move |cancel: &crate::background::CancelToken| {
            if let (Some(url), Some(session)) = (crate::remote::url_of(&dir_owned), session.as_ref()) {
                crate::remote_ui::read_rows(session, &url)
            } else if let Some((arch, inner)) = crate::archive::parse_archive_path(&dir_owned) {
                crate::archive::read_archive_rows_watched(&arch, &inner, &worker_progress, cancel)
            } else {
                read_dir_rows(&dir_owned)
            }
        };

        // Cleared when the result arrives, so a late progress timer does
        // not show the panel for a read that is already over.
        let loading = Rc::new(Cell::new(true));
        if let Some((arch, _)) = crate::archive::parse_archive_path(dir) {
            self.watch_archive_read(arch, progress, &cancel, &loading, history_before.clone());
        }

        let this = self.clone();
        let dir_owned = dir.to_path_buf();
        crate::background::run_cancellable(&cancel, read, move |result: io::Result<Vec<Row>>| {
            loading.set(false);
            // Superseded by a later navigation while this was loading.
            if this.nav.generation.get() != gen {
                return;
            }
            this.progress.hide();
            // A failed read leaves the previous listing on screen; callers that
            // act on the new listing (select an entry, restore a selection)
            // must not run against the wrong folder.
            if this.finish_listing(&dir_owned, result, cached, restore, previous, history_before) {
                on_done();
            }
        });
    }

    /// Show the progress panel for the read of archive `arch` if it is still
    /// going after [`PROGRESS_SHOW_DELAY`] (a cached or small archive never
    /// shows it), then refresh it until the read ends. The read is over when
    /// `loading` is cleared or `cancel` is cancelled (a newer navigation, or
    /// the panel's Cancel, see [`Ui::cancel_archive_read`]).
    fn watch_archive_read(
        self: &Rc<Self>,
        arch: PathBuf,
        progress: Arc<crate::archive::ReadProgress>,
        cancel: &CancelToken,
        loading: &Rc<Cell<bool>>,
        history_before: crate::history::History,
    ) {
        let (weak, cancel, loading) = (Rc::downgrade(self), cancel.clone(), loading.clone());
        glib::timeout_add_local_once(PROGRESS_SHOW_DELAY, move || {
            if cancel.is_cancelled() || !loading.get() { return; }
            let Some(ui) = weak.upgrade() else {
                opusdm_ipc::err_log!("opusdm-lister", "archive progress: the Lister window is gone");
                return;
            };
            let name = arch.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let total = match std::fs::metadata(&arch) {
                Ok(m) => m.len(),
                Err(e) => {
                    crate::dlog!("archive progress: no size for {}: {e}", arch.display());
                    0
                }
            };
            {
                let (weak, cancel, loading, name) = (Rc::downgrade(&ui), cancel.clone(), loading.clone(), name.clone());
                ui.progress.show(&format!("Opening {name}..."), move || {
                    let Some(ui) = weak.upgrade() else {
                        opusdm_ipc::err_log!("opusdm-lister", "archive progress: Cancel pressed but the Lister window is gone");
                        return;
                    };
                    ui.cancel_archive_read(&cancel, &loading, history_before.clone(), &name);
                });
            }
            let weak = Rc::downgrade(&ui);
            let tick = move || {
                if cancel.is_cancelled() || !loading.get() { return glib::ControlFlow::Break; }
                let Some(ui) = weak.upgrade() else {
                    opusdm_ipc::err_log!("opusdm-lister", "archive progress: the Lister window is gone");
                    return glib::ControlFlow::Break;
                };
                let entries = progress.entries.load(Ordering::Relaxed);
                let bytes = progress.bytes.load(Ordering::Relaxed);
                ui.progress.set_message(&progress_message(&name, entries));
                ui.progress.set_fraction((total > 0).then(|| bytes as f64 / total as f64));
                glib::ControlFlow::Continue
            };
            glib::timeout_add_local(PROGRESS_TICK, tick);
        });
    }

    /// The user cancelled the archive read the panel shows: stop it, stay in
    /// the folder on screen and undo the history step of the move.
    fn cancel_archive_read(&self, cancel: &CancelToken, loading: &Cell<bool>, history_before: crate::history::History, name: &str) {
        if cancel.is_cancelled() || !loading.get() {
            crate::dlog!("archive progress: Cancel for a read that is already over");
            self.progress.hide();
            return;
        }
        crate::dlog!("archive progress: opening {name} cancelled by the user");
        cancel.cancel();
        self.progress.hide();
        *self.nav.history.borrow_mut() = history_before;
        self.refresh_history_ui();
        self.refresh_path_entry();
        self.flash(&format!("opening {name} cancelled"));
    }

    /// Handle the disk read that follows a navigation. `cached` is the
    /// listing already shown for it, if any. False when the read failed.
    fn finish_listing(
        self: &Rc<Self>,
        dir: &Path,
        result: io::Result<Vec<Row>>,
        cached: Option<Listing>,
        restore: bool,
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
                if !is_archive && self.caches.dirs.borrow_mut().remove(dir) {
                    self.report_cache();
                }
                self.apply_navigation(dir, Err(e));
                // The move never happened: undo its history step, and if the
                // cached rows were already shown, go back where we were.
                *self.nav.history.borrow_mut() = history_before;
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
        let rows = crate::model::listing(rows);
        // Keep the user's selection when the fresh listing replaces a cached one.
        let selected = if cached.is_some() { self.selected_names() } else { Vec::new() };
        if !is_archive {
            self.caches.dirs.borrow_mut().put(dir.to_path_buf(), rows.clone());
            self.report_cache();
        }
        let was_shown = cached.is_some();
        self.apply_navigation(dir, Ok(rows));
        self.select_names(&selected);
        // Nothing was shown yet (no cache, an archive): this is the arrival.
        if restore && !was_shown {
            self.restore_view(dir);
        }
        true
    }

    /// Select the rows called `names`, adding to the current selection.
    /// One `set_selection` call, so `selection-changed` fires once however
    /// many rows match.
    pub fn select_names(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let wanted: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
        let matches = gtk::Bitset::new_empty();
        for i in 0..self.view.selection.n_items() {
            if self.row_at(i).is_some_and(|row| wanted.contains(row.name.as_str())) {
                matches.add(i);
            }
        }
        if matches.is_empty() {
            return;
        }
        // The mask is the same set: only these rows change, the others keep their state.
        self.view.selection.set_selection(&matches, &matches);
    }

    /// Tell the hub which directories this Lister has cached.
    pub fn report_cache(&self) {
        let paths = self.caches.dirs.borrow().paths();
        self.send_hub(&ToHub::CacheReport(paths));
    }

    /// Drop the whole cache and tell the hub.
    pub fn free_caches(&self) {
        self.caches.clear();
        self.report_cache();
    }

    /// Go to the previous history entry.
    pub fn history_back(self: &Rc<Self>) {
        let before = self.nav.history.borrow().clone();
        let target = self.nav.history.borrow_mut().back();
        if let Some(dir) = target {
            self.navigate_from(&dir, true, true, before, || {});
        }
    }

    /// Go to the next history entry.
    pub fn history_forward(self: &Rc<Self>) {
        let before = self.nav.history.borrow().clone();
        let target = self.nav.history.borrow_mut().forward();
        if let Some(dir) = target {
            self.navigate_from(&dir, true, true, before, || {});
        }
    }

    /// Go to history entry `index`.
    pub fn history_jump(self: &Rc<Self>, index: usize) {
        let before = self.nav.history.borrow().clone();
        let target = self.nav.history.borrow_mut().jump(index);
        if let Some(dir) = target {
            self.navigate_from(&dir, true, true, before, || {});
        }
    }

    /// Enable / disable the buttons and rebuild the menu rows (newest first,
    /// the current entry in bold).
    pub fn refresh_history_ui(&self) {
        use gtk::prelude::*;
        let history = self.nav.history.borrow();
        self.nav.widgets.back.set_sensitive(history.can_back());
        self.nav.widgets.forward.set_sensitive(history.can_forward());
        let list = &self.nav.widgets.list;
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
        let weak = Rc::downgrade(ui);
        ui.nav.widgets.back.connect_clicked(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            ui.history_back();
        });
    }
    {
        let weak = Rc::downgrade(ui);
        ui.nav.widgets.forward.connect_clicked(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            ui.history_forward();
        });
    }
    {
        let weak = Rc::downgrade(ui);
        let popover = popover.clone();
        ui.nav.widgets.list.connect_row_activated(move |_, row| {
            let Some(ui_for_click) = weak.upgrade() else { return };
            let len = ui_for_click.nav.history.borrow().entries().len();
            // Rows are listed newest first.
            let Some(index) = len.checked_sub(1 + row.index() as usize) else { return };
            popover.popdown();
            ui_for_click.history_jump(index);
        });
    }
    ui.refresh_history_ui();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_progress_message_counts_the_entries_read() {
        assert_eq!(progress_message("a.tar.bz2", 0), "Opening a.tar.bz2...");
        assert_eq!(progress_message("a.tar.bz2", 1), "Opening a.tar.bz2: 1 entry read");
        assert_eq!(progress_message("a.tar.bz2", 1200), "Opening a.tar.bz2: 1200 entries read");
    }
}
