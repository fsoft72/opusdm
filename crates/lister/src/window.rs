//! The Lister window: types and behavior once the window exists.
//! Construction and signal wiring live in `build.rs`.

use crate::client::{Client, Roles};
use crate::model::Row;
use gtk4 as gtk;
use gtk::gio;
use gtk::glib::{self, BoxedAnyObject};
use gtk::prelude::*;
use opusdm_ipc::{ListerId, Settings, ToHub, ToLister};
use std::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Whether the keyboard focus currently sits in a text field, where a plain
/// letter is a character rather than a shortcut.
pub fn is_editing(window: &gtk::ApplicationWindow) -> bool {
    gtk::prelude::RootExt::focus(window)
        .map(|w| w.is::<gtk::Editable>() || w.is::<gtk::TextView>())
        .unwrap_or(false)
}

/// Which representation the file area shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Details,
    Icons,
}

/// This Lister's connection to the hub: identity, role state and the
/// settings/watch state the hub keeps it in sync with.
pub struct HubConnection {
    /// Hub connection (send half).
    pub client: Rc<Client>,
    /// This Lister's id.
    pub id: ListerId,
    /// Latest known global roles.
    pub roles: RefCell<Roles>,
    /// inotify watch on the open directory. `None` when the kernel refused an
    /// inotify instance (watch limit reached), in which case the Lister simply
    /// keeps working without auto-refresh.
    pub watcher: Option<crate::watcher::DirWatcher>,
    /// Current settings (colors, and whatever else joins later). Updated
    /// wholesale from `Welcome` and `SettingsUpdate`.
    pub settings: RefCell<Settings>,
    /// Latest hotkeys, user menu and script events. Updated wholesale from
    /// `Welcome` and `BindingsUpdate`.
    pub bindings: RefCell<opusdm_ipc::Bindings>,
    /// Latest saved formats. Updated wholesale from `Welcome` and
    /// `FormatsUpdate`.
    pub formats: RefCell<opusdm_ipc::FormatStore>,
    /// Lookup table of the local user hotkeys, rebuilt whenever `bindings`
    /// changes.
    pub hotkeys: RefCell<crate::hotkeys::HotkeyMap>,
}

/// The top status bar: D/F/B counts, the role badge and pin, and the
/// transient message line below the file area.
pub struct StatusBar {
    /// Status line (top bar with D/F/B counts).
    pub label: gtk::Label,
    /// Stat bar box (for background color based on role).
    pub container: gtk::Box,
    /// SOURCE / DEST badge.
    pub role_badge: gtk::Label,
    /// Pin: locks this Lister's current role against focus changes (stat bar).
    pub role_lock_btn: gtk::ToggleButton,
    /// Guards against feedback while syncing toggle state from the hub.
    pub role_guard: Cell<bool>,
    /// In-window transient message line (errors, job status).
    pub job_line: gtk::Label,
    /// Cached directory totals: (total_dirs, total_files, total_bytes)
    pub totals: Cell<(u32, u32, u64)>,
    /// Used-space gauge; visible only when the format asks for it.
    pub gauge: gtk::LevelBar,
    /// Counts gauge requests so a slow `statvfs` cannot overwrite a newer one.
    pub gauge_generation: Rc<Cell<u64>>,
}

/// The file area: which representation is shown and the model/selection
/// feeding whichever one is active.
pub struct ViewState {
    /// Which view is active.
    pub mode: Cell<ViewMode>,
    /// Stack holding the column-view scroller ("details") and the icon field
    /// ("icons").
    pub stack: gtk::Stack,
    /// Reusable icon view, shown in place of the column view in
    /// `ViewMode::Icons`.
    pub icon_field: opusdm_iconview::IconField,
    /// Icon size in the icon view, cycled by Ctrl+scroll.
    pub icon_size: Cell<i32>,
    /// The file list itself, kept for cursor movement and scrolling.
    pub column_view: gtk::ColumnView,
    /// Selection model feeding the column view.
    pub selection: gtk::MultiSelection,
    /// Backing store of `BoxedAnyObject`-wrapped `Rc<Row>`.
    pub store: gio::ListStore,
    /// Row the keyboard cursor sits on. Kept in step with the mouse by
    /// `selection`'s changed signal.
    pub cursor: Cell<u32>,
    /// Filter bar widget (search entry, filter model, etc.)
    pub filter_bar: Rc<crate::filter::FilterBar>,
    /// Whether dotfiles are shown in the file list. Off by default.
    pub show_hidden: Cell<bool>,
    /// Every column with the format column it shows. The columns are built
    /// once; a format only changes visibility and order.
    pub col_map: Rc<Vec<(gtk::ColumnViewColumn, opusdm_ipc::FormatColumn)>>,
    /// Directory / file separation read by the model sorter.
    pub separate: Rc<Cell<opusdm_ipc::Separate>>,
    /// The sorter feeding the list; told when the separation changes.
    pub model_sorter: gtk::CustomSorter,
}

/// The back / forward buttons and the history drop-down of the path bar.
pub struct NavHistory {
    /// Go back one entry.
    pub back: gtk::Button,
    /// Go forward one entry.
    pub forward: gtk::Button,
    /// The rows of the history pop-over, newest first.
    pub list: gtk::ListBox,
}

/// Everything the window's callbacks need. Shared by `Rc` across Tasks 13-15.
pub struct Ui {
    /// The top-level window.
    pub window: gtk::ApplicationWindow,
    /// Hub connection and role/settings state.
    pub hub: HubConnection,
    /// Current directory.
    pub cwd: RefCell<PathBuf>,
    /// Whether the hub minimized this window when it hid the desktop, so a
    /// reveal knows it should bring it back.
    pub hidden_by_hub: Cell<bool>,
    /// The file area's model, selection and view-mode state.
    pub view: ViewState,
    /// Top status bar widgets and cached totals.
    pub status_bar: StatusBar,
    /// Editable path bar.
    pub path_entry: gtk::Entry,
    /// Empty box the button row is built into by `actions::attach`.
    pub button_row: gtk::Box,
    /// Active Quick Look preview window, if open.
    pub preview_win: RefCell<Option<Rc<crate::preview::PreviewWindow>>>,
    /// Quake-style dropdown terminal drawer.
    pub terminal: Rc<crate::terminal::TerminalDrawer>,
    /// Single stylesheet for settings-driven colors; reloaded in place on
    /// change rather than re-registered, so providers don't stack up on the
    /// display.
    pub css_provider: gtk::CssProvider,
    /// Bumped on every `navigate_to`; an in-flight background listing whose
    /// generation no longer matches was superseded by a later navigation and
    /// its result is dropped instead of applied.
    pub nav_generation: Cell<u64>,
    /// The log buffer of the currently-open Function Editor's per-function
    /// window, if one is open - Function run events are appended here while
    /// set, so the Test button shows real-time output. `None` when no
    /// editor window is open (a palette-triggered run then only flashes a
    /// summary).
    pub function_log: RefCell<Option<gtk::TextBuffer>>,
    /// Names selected when a job last started or the selection was last
    /// cleared, for Reselect.
    pub last_selection: RefCell<Vec<String>>,
    /// The "Menu" drop-down of the button row; its popup is built lazily by
    /// `user_menu::install`.
    pub user_menu_btn: gtk::MenuButton,
    /// Text typed so far for type-to-find; empty when no search is open.
    pub typeahead_buffer: RefCell<String>,
    /// The toolbar's show-hidden toggle, so keyboard and palette changes can
    /// keep its state and icon in step.
    pub hidden_btn: RefCell<Option<gtk::ToggleButton>>,
    /// Idle timeout that closes type-to-find.
    pub typeahead_timer: RefCell<Option<glib::SourceId>>,
    /// The format applied to the current directory.
    pub format: RefCell<opusdm_ipc::ListerFormat>,
    /// A format applied with "Use": valid only for the directory it was
    /// applied in.
    pub temp_format: RefCell<Option<(PathBuf, opusdm_ipc::ListerFormat)>>,
    /// Recently read directory listings (raw rows, before the Show / Hide filter).
    pub dir_cache: RefCell<crate::dircache::DirCache>,
    /// Open FTP / SFTP connections (F-39).
    pub remote: crate::remote_ui::RemoteState,
    /// Back / forward history.
    pub history: RefCell<crate::history::History>,
    /// Back / forward buttons and the history drop-down.
    pub nav_history: NavHistory,
}

impl Ui {
    /// Reveal and focus the filter bar.
    pub fn reveal_filter(&self) {
        self.view.filter_bar.reveal();
    }

    /// Dismiss and reset the filter bar.
    pub fn dismiss_filter(&self) {
        self.view.filter_bar.dismiss();
    }

    /// Append `text` to the open Function Editor's log, if one is open.
    fn append_function_log(self: &Rc<Self>, text: &str) {
        if let Some(buf) = self.function_log.borrow().as_ref() {
            let mut end = buf.end_iter();
            buf.insert(&mut end, text);
        }
    }

    /// Toggle or open Quick Look preview for the currently selected item.
    pub fn toggle_preview(self: &Rc<Self>) {
        let mut p_guard = self.preview_win.borrow_mut();
        if let Some(prev) = p_guard.take() {
            prev.window.close();
            return;
        }

        let paths = self.selected_paths();
        let target = if let Some(p) = paths.first() {
            p.clone()
        } else {
            self.cwd_path()
        };

        let prev = crate::preview::PreviewWindow::new(&self.window);
        prev.load_file(&target);

        let ui_clone = self.clone();
        prev.window.connect_close_request(move |_| {
            ui_clone.preview_win.borrow_mut().take();
            glib::Propagation::Proceed
        });

        prev.window.present();
        *p_guard = Some(prev);
    }

    /// If preview window is open, update it with the current selection.
    pub fn update_preview_if_open(&self) {
        let guard = self.preview_win.borrow();
        if let Some(prev) = guard.as_ref() {
            let paths = self.selected_paths();
            if let Some(target) = paths.first() {
                prev.load_file(target);
            }
        }
    }

    /// Toggle the Quake-style dropdown terminal drawer.
    pub fn toggle_terminal(&self) {
        self.terminal.toggle();
    }

    /// Go to `dir` (see [`Ui::navigate`]): a cached listing shows at once and
    /// the disk is re-read on a background thread.
    pub fn navigate_to(self: &Rc<Self>, dir: &Path) {
        self.navigate(dir, true, || {});
    }

    /// Apply a listing produced by `navigate_to`'s background read.
    pub(crate) fn apply_navigation(self: &Rc<Self>, dir: &Path, rows: io::Result<Vec<Row>>) {
        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                if let Some((arch, _)) = crate::archive::parse_archive_path(dir) {
                    self.status_bar.job_line.set_text(&format!("cannot read archive {}: {e}", arch.display()));
                } else if crate::remote::url_of(dir).is_some() {
                    self.status_bar.job_line.set_text(&format!("cannot open {dir}: {e}", dir = dir.display()));
                } else {
                    self.status_bar.job_line.set_text(&format!("cannot open {}: {e}", dir.display()));
                }
                return;
            }
        };
        self.dismiss_filter();
        if let Some((arch, _)) = crate::archive::parse_archive_path(dir) {
            if let Some(parent) = arch.parent() {
                self.terminal.sync_cwd(parent);
            }
        } else if crate::remote::url_of(dir).is_none() {
            self.terminal.sync_cwd(dir);
        }
        *self.cwd.borrow_mut() = dir.to_path_buf();
        self.history.borrow_mut().push(dir.to_path_buf());
        self.refresh_history_ui();
        // A "Use" format only lives in the directory it was applied in.
        if self.temp_format.borrow().as_ref().is_some_and(|(p, _)| p != dir) {
            *self.temp_format.borrow_mut() = None;
        }
        let format = self.effective_format(dir);
        if format != *self.format.borrow() {
            self.apply_format(&format);
        }
        let name_filter = opusdm_ipc::NameFilter::new(&format.show_glob, &format.hide_glob);
        let show_hidden = self.view.show_hidden.get();
        let mut total_dirs = 0u32;
        let mut total_files = 0u32;
        let mut total_bytes = 0u64;
        let mut new_items = Vec::new();
        for row in rows {
            if row.name == opusdm_iconview::scan::OPUSDM_DIR {
                continue;
            }
            if !show_hidden && row.name.starts_with('.') {
                continue;
            }
            if !row.is_dir && !name_filter.allows(&row.name) {
                continue;
            }
            if row.is_dir {
                total_dirs += 1;
            } else {
                total_files += 1;
                total_bytes += row.size;
            }
            new_items.push(BoxedAnyObject::new(Rc::new(row)));
        }
        self.sync_watch(dir);
        self.status_bar.totals.set((total_dirs, total_files, total_bytes));
        // A new listing invalidates the old cursor row.
        self.view.cursor.set(0);
        let old_len = self.view.store.n_items();
        self.view.store.splice(0, old_len, &new_items);
        self.window.set_title(Some(&dir.to_string_lossy()));
        crate::dlog!("navigate_to {} done: {} items", dir.display(), new_items.len());
        self.refresh_path_entry();
        self.update_status();
        self.update_gauge(dir);
        self.refresh_icon_view();
        let _ = self.hub.client.send(&ToHub::PathChanged(dir.to_path_buf()));
    }

    /// Reconcile the icon view's entries with the current sorted model. A
    /// no-op unless the icon view is the active one. Incremental: selection,
    /// cached thumbnails and unchanged item widgets survive the update.
    pub fn refresh_icon_view(self: &Rc<Self>) {
        if self.view.mode.get() != ViewMode::Icons {
            return;
        }
        let dir = self.cwd.borrow().clone();
        let n = self.view.selection.n_items();
        let mut entries = Vec::with_capacity(n as usize);
        for i in 0..n {
            if let Some(row) = self.row_at(i) {
                entries.push(crate::model::row_to_entry(&row, &dir));
            }
        }
        self.view.icon_field.refresh_entries(&entries);
    }

    /// Rename one row in the store in place, without re-reading the
    /// directory. Falls back to a full `reload` if the row is gone (e.g. it
    /// was removed by something else between the rename and this call).
    pub fn rename_row(self: &Rc<Self>, old_name: &str, new_name: &str) {
        for i in 0..self.view.store.n_items() {
            let Some(item) = self.view.store.item(i) else { continue };
            let Some(boxed) = item.downcast_ref::<BoxedAnyObject>() else { continue };
            let row = boxed.borrow::<Rc<Row>>();
            if row.name != old_name {
                continue;
            }
            let mut updated = (**row).clone();
            updated.name = new_name.to_string();
            drop(row);
            self.view.store.splice(i, 1, &[BoxedAnyObject::new(Rc::new(updated))]);
            self.refresh_icon_view();
            self.update_status();
            for j in 0..self.view.selection.n_items() {
                if self.row_at(j).is_some_and(|r| r.name == new_name) {
                    self.view.selection.select_item(j, true);
                    break;
                }
            }
            return;
        }
        self.reload();
    }

    /// Force every row to rebind (e.g. after an icon-set or type-indicator
    /// setting change) by resplicing the store's current rows, without
    /// touching disk.
    pub fn touch_rows(self: &Rc<Self>) {
        let n = self.view.store.n_items();
        let items: Vec<_> = (0..n).filter_map(|i| self.view.store.item(i)).collect();
        self.view.store.splice(0, n, &items);
        self.refresh_icon_view();
    }

    /// Re-read the current directory from disk (never from the cache),
    /// keeping the selection by name once the new listing lands.
    pub fn reload(self: &Rc<Self>) {
        let selected = self.selected_names();
        let cwd = self.cwd.borrow().clone();
        let this = self.clone();
        self.navigate(&cwd, false, move || this.select_names(&selected));
    }

    /// Answer the hub hiding (`true`) or revealing (`false`) the desktop:
    /// minimize this window, or bring it back if the hub minimized it.
    pub fn set_hidden(&self, hidden: bool) {
        let minimized = self
            .window
            .surface()
            .and_then(|s| s.downcast::<gtk::gdk::Toplevel>().ok())
            .is_some_and(|t| t.state().contains(gtk::gdk::ToplevelState::MINIMIZED));
        let (action, hidden_by_hub) = crate::hidden::next_step(self.hidden_by_hub.get(), minimized, hidden);
        self.hidden_by_hub.set(hidden_by_hub);
        match action {
            crate::hidden::HideAction::Minimize => self.window.minimize(),
            // No `present()`: raising every restored window in turn would give
            // the focus (and the SOURCE role) to whichever comes last. The hub
            // raises the SOURCE Lister itself once everyone is back.
            crate::hidden::HideAction::Restore => self.window.unminimize(),
            crate::hidden::HideAction::Nothing => {}
        }
    }

    /// Absolute paths of the selected rows.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let cwd = self.cwd.borrow().clone();
        self.selected_names().into_iter().map(|n| cwd.join(n)).collect()
    }

    /// Names of all currently selected rows.
    pub fn selected_names(&self) -> Vec<String> {
        self.selected_positions()
            .into_iter()
            .filter_map(|i| self.row_at(i).map(|row| row.name.clone()))
            .collect()
    }

    /// Positions of all currently selected rows, without testing every row
    /// in the list: `MultiSelection::selection()` already tracks them as a
    /// `gtk::Bitset`, so walk only that instead of scanning `0..n_items()`.
    fn selected_positions(&self) -> Vec<u32> {
        let bitset = self.view.selection.selection();
        let mut out = Vec::with_capacity(bitset.size() as usize);
        if let Some((iter, first)) = gtk::BitsetIter::init_first(&bitset) {
            out.push(first);
            out.extend(iter);
        }
        out
    }

    /// Retrieve the row model at `position` in the visual list.
    pub fn row_at(&self, position: u32) -> Option<Rc<Row>> {
        self.view.selection
            .item(position)
            .and_downcast::<BoxedAnyObject>()
            .map(|o| o.borrow::<Rc<Row>>().clone())
    }

    pub fn refresh_path_entry(&self) {
        self.path_entry.set_text(&self.cwd.borrow().to_string_lossy());
    }

    /// Current directory (clone).
    pub fn cwd_path(&self) -> PathBuf {
        self.cwd.borrow().clone()
    }

    /// Recompute the status line and the role badge.
    pub fn update_status(&self) {
        let (total_dirs, total_files, total_bytes) = self.status_bar.totals.get();
        let mut sel_dirs = 0u32;
        let mut sel_files = 0u32;
        let mut sel_bytes = 0u64;

        for i in self.selected_positions() {
            if let Some(row) = self.row_at(i) {
                if row.is_dir {
                    sel_dirs += 1;
                } else {
                    sel_files += 1;
                    sel_bytes += row.size;
                }
            }
        }

        let roles = self.hub.roles.borrow();
        let (badge, stat_class) = if roles.source == Some(self.hub.id) {
            ("SOURCE", "stat-source")
        } else if roles.is_dest(self.hub.id) {
            ("DEST", "stat-dest")
        } else {
            ("NONE", "")
        };
        self.status_bar.role_badge.set_text(badge);

        // Update stat bar background color based on role.
        self.status_bar.container.remove_css_class("stat-source");
        self.status_bar.container.remove_css_class("stat-dest");
        if !stat_class.is_empty() {
            self.status_bar.container.add_css_class(stat_class);
        }
        drop(roles);

        let format = self.hub.settings.borrow().behavior.status_format.clone();
        let format = if format.is_empty() { opusdm_ipc::DEFAULT_STATUS_FORMAT.to_string() } else { format };
        let info = crate::status_format::StatusInfo {
            dirs: total_dirs,
            files: total_files,
            bytes: total_bytes,
            sel_dirs,
            sel_files,
            sel_bytes,
            path: self.cwd.borrow().to_string_lossy().into_owned(),
        };
        self.status_bar.label.set_text(&crate::status_format::format_status(&format, &info));
    }

    /// Handle a message from the hub.
    pub fn apply_inbound(self: &Rc<Self>, msg: ToLister) {
        crate::dlog!("hub msg: {msg:?}");
        match msg {
            ToLister::RoleChanged { source, dests, source_locked } => {
                *self.hub.roles.borrow_mut() = Roles { source, dests, source_locked };
                self.update_status();
                crate::actions::sync_role_buttons(self);
            }
            ToLister::InvalidatePath(p) => {
                if self.dir_cache.borrow_mut().remove(&p) {
                    self.report_cache();
                }
                if p == *self.cwd.borrow() {
                    self.reload();
                }
            }
            ToLister::Present => self.window.present(),
            ToLister::SetHidden(hidden) => self.set_hidden(hidden),
            ToLister::Shutdown => self.window.close(),
            // Progress is shown centrally by the hub's progress window now.
            ToLister::JobProgress { .. } => {}
            ToLister::JobDone { job_id, outcome, errors } => {
                crate::actions::on_job_done(self, job_id, &outcome, &errors);
            }
            ToLister::ConflictPrompt { req_id, path, src, dst, .. } => {
                crate::actions::on_conflict(self, req_id, &path, src, dst);
            }
            ToLister::SettingsUpdate(new_settings) => {
                let names_changed = {
                    let old = self.hub.settings.borrow();
                    old.theme.icon_set != new_settings.theme.icon_set
                        || old.appearance.type_indicators != new_settings.appearance.type_indicators
                };
                *self.hub.settings.borrow_mut() = new_settings;
                let capacity = self.hub.settings.borrow().behavior.cache_capacity();
                self.dir_cache.borrow_mut().set_capacity(capacity);
                self.report_cache();
                self.update_status();
                opusdm_ui::apply_theme_mode(self.hub.settings.borrow().appearance.theme_mode);
                self.update_css_for_colors();
                self.view.icon_field.set_icon_set(self.hub.settings.borrow().theme.icon_set.clone());
                // Auto-refresh may have just been switched on or off.
                let cwd = self.cwd.borrow().clone();
                self.sync_watch(&cwd);
                // Name-column text is built at bind time, so glyph and
                // indicator changes only show after the rows are rebound.
                if names_changed {
                    self.touch_rows();
                }
            }
            ToLister::Pong => {}
            ToLister::FreeCaches => self.free_caches(),
            ToLister::GoTo { path } => {
                self.window.present();
                self.navigate_to(&path);
            }
            ToLister::CacheList(entries) => crate::cache_list::show(self, entries),
            ToLister::FormatsUpdate(new_formats) => {
                *self.hub.formats.borrow_mut() = new_formats;
                self.on_formats_changed();
            }
            ToLister::BindingsUpdate(new_bindings) => {
                *self.hub.hotkeys.borrow_mut() = crate::hotkeys::HotkeyMap::from_hotkeys(&new_bindings.hotkeys);
                *self.hub.bindings.borrow_mut() = new_bindings;
            }
            ToLister::QuerySelection => {
                let _ = self.hub.client.send(&opusdm_ipc::ToHub::SelectionReply {
                    paths: self.selected_paths(),
                    cwd: self.cwd_path(),
                });
            }
            ToLister::CheckFitResult { dest_dir, needed, free, fits, percent } => {
                crate::checkfit_ui::show_result(self, &dest_dir, needed, free, fits, percent);
            }
            ToLister::CheckFitFailed { message } => {
                crate::actions::flash(self, &format!("check fit failed: {message}"));
            }
            ToLister::FunctionOutput { step_idx, line } => {
                self.append_function_log(&format!("[step {step_idx}] {line}\n"));
            }
            ToLister::FunctionStepDone { step_idx, exit_code } => {
                self.append_function_log(&format!("[step {step_idx}] done (exit {exit_code:?})\n"));
            }
            ToLister::FunctionFailed { step_idx, error } => {
                self.append_function_log(&format!("[step {step_idx}] FAILED: {error}\n"));
                crate::actions::flash(self, &format!("function failed at step {step_idx}: {error}"));
            }
            ToLister::FunctionDone => {
                self.append_function_log("function finished\n");
                crate::actions::flash(self, "function finished");
            }
        }
    }

    /// Carry out a binding resolved by [`crate::keys::resolve`].
    pub fn run_key_action(self: &Rc<Self>, action: crate::keys::Action) {
        use crate::keys::Action;
        match action {
            Action::Command(id) => crate::palette::execute_command(self, id),
            Action::Enter => self.activate_row(self.view.cursor.get()),
            Action::ToggleSelectAndAdvance => self.toggle_select_and_advance(),
            Action::SwapRoleFocus => self.swap_role_focus(),
            Action::OpenPalette => crate::palette::open_command_palette(self),
            Action::RevealFilter => self.reveal_filter(),
            Action::ToggleTerminal => self.toggle_terminal(),
            Action::PreviewOrSize => {
                // A directory selection means "how big is this?", anything else
                // means "show me what is inside".
                if self.selected_paths().iter().any(|p| p.is_dir()) {
                    crate::tools::calculate_dir_sizes_async(self);
                } else {
                    self.toggle_preview();
                }
            }
            Action::Escape => self.dismiss_topmost(),
        }
    }

    /// Escape unwinds one layer at a time: filter bar, then terminal drawer,
    /// then - only once the focus is already back on the list - the selection.
    fn dismiss_topmost(self: &Rc<Self>) {
        if self.view.filter_bar.container.is_visible() {
            self.dismiss_filter();
        } else if self.terminal.container.is_visible() {
            self.terminal.container.set_visible(false);
        } else if is_editing(&self.window) {
            // Escape out of the path entry without touching the selection.
        } else {
            self.remember_selection();
            self.view.selection.unselect_all();
            return;
        }
        self.view.column_view.grab_focus();
    }

    /// Open the row at `position`: descend into a directory, step into an
    /// archive, run the double click function of its file type, or hand the
    /// file to its default application.
    pub fn activate_row(self: &Rc<Self>, position: u32) {
        let Some(row) = self.row_at(position) else { return };
        let cwd = self.cwd.borrow().clone();
        if let Some((arch, inner)) = crate::archive::parse_archive_path(&cwd) {
            let next_inner = if inner.is_empty() {
                row.name.clone()
            } else {
                format!("{}/{}", inner.trim_end_matches('/'), row.name)
            };
            if row.is_dir {
                self.navigate_to(&crate::archive::make_archive_path(&arch, &next_inner));
            } else if let Ok(temp_path) =
                crate::archive::extract_single_file_temp(&arch, &next_inner)
            {
                let uri = format!("file://{}", temp_path.to_string_lossy());
                let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
            }
            return;
        }
        let target = cwd.join(&row.name);
        if let Some(url) = crate::remote::url_of(&cwd) {
            if row.is_dir {
                self.navigate_to(&target);
            } else {
                let item = opusdm_ftp::RemoteItem { path: url.join(&row.name).path, name: row.name.clone(), is_dir: false };
                self.download_remote(&url, vec![item], |_ui, paths| {
                    if let Some(path) = paths.first() {
                        let uri = format!("file://{}", path.to_string_lossy());
                        let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
                    }
                });
            }
            return;
        }
        if row.is_dir {
            self.navigate_to(&target);
        } else if crate::archive::is_archive_file(&target) {
            self.navigate_to(&crate::archive::make_archive_path(&target, ""));
        } else if let Some(function) = crate::filetypes_ui::type_for(&target, &row).and_then(|t| t.actions.dblclick) {
            crate::filetypes_ui::run_on_row(self, &row.name, &function);
        } else {
            let uri = format!("file://{}", target.to_string_lossy());
            let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
        }
    }

    /// Put the keyboard cursor on `pos` and make that row the only
    /// selection, the way arrow keys already behave.
    pub fn jump_to_row(&self, pos: u32) {
        let n = self.view.selection.n_items();
        if pos >= n {
            return;
        }
        self.view.cursor.set(pos);
        self.view.selection.select_item(pos, true);
        self.scroll_to_cursor();
    }

    /// Toggle the row under the cursor and step down, so a run of `Insert`
    /// marks consecutive files the way Directory Opus does.
    pub fn toggle_select_and_advance(&self) {
        let n = self.view.selection.n_items();
        if n == 0 {
            return;
        }
        let pos = self.view.cursor.get().min(n - 1);
        if self.view.selection.is_selected(pos) {
            self.view.selection.unselect_item(pos);
        } else {
            self.view.selection.select_item(pos, false);
        }
        // Set the cursor after toggling: the selection-changed handler would
        // otherwise drag it back to whatever is selected.
        self.view.cursor.set((pos + 1).min(n - 1));
        self.scroll_to_cursor();
    }

    /// Bring the cursor row into view. `ColumnView::scroll_to` needs GTK 4.12,
    /// so this goes through the `GtkListBase` action available at 4.10.
    fn scroll_to_cursor(&self) {
        let pos = self.view.cursor.get();
        let _ = self
            .view
            .column_view
            .activate_action("list.scroll-to-item", Some(&pos.to_variant()));
    }

    /// Follow the mouse: a plain click leaves exactly one row selected, and the
    /// cursor should continue from there.
    pub fn sync_cursor_from_selection(&self) {
        let positions = self.selected_positions();
        if let [only] = positions[..] {
            self.view.cursor.set(only);
        }
        // More than one (or zero) selected: leave the cursor alone.
    }

    /// Tab: raise the Lister holding the opposite role. A SOURCE jumps to the
    /// first DEST and vice versa; a Lister with no role jumps to SOURCE.
    fn swap_role_focus(self: &Rc<Self>) {
        let target = {
            let roles = self.hub.roles.borrow();
            let picked = if roles.source == Some(self.hub.id) {
                roles.dests.first().copied()
            } else if roles.is_dest(self.hub.id) {
                roles.source
            } else {
                roles.source.or_else(|| roles.dests.first().copied())
            };
            picked.filter(|t| *t != self.hub.id)
        };
        let Some(target) = target else {
            crate::actions::flash(self, "no other SOURCE/DEST Lister to switch to");
            return;
        };
        let _ = self.hub.client.send(&ToHub::PresentLister(target));
    }

    /// Point the inotify watch at `dir`, or drop it when auto-refresh is off or
    /// the path is a virtual archive (nothing on disk to watch).
    fn sync_watch(&self, dir: &Path) {
        let Some(watcher) = &self.hub.watcher else { return };
        let enabled = self.hub.settings.borrow().behavior.auto_refresh;
        if enabled && crate::archive::parse_archive_path(dir).is_none() && crate::remote::url_of(dir).is_none() {
            watcher.watch(dir);
        } else {
            watcher.unwatch();
        }
    }

    /// A watched directory changed on disk. Reloads only if it is still the one
    /// on screen.
    ///
    /// `reload` goes through `navigate_to`, which resets the filter bar, so an
    /// auto-refresh would otherwise wipe a filter the user is halfway through
    /// typing. Save it across the reload and put it back.
    pub fn on_directory_changed(self: &Rc<Self>, dir: &Path) {
        crate::dlog!("watch event: {}", dir.display());
        if *self.cwd.borrow() != dir {
            return;
        }
        let filtering = self.view.filter_bar.container.is_visible();
        let query = filtering.then(|| self.view.filter_bar.entry.text().to_string());
        let had_focus = filtering && self.view.filter_bar.entry.has_focus();

        self.reload();

        let Some(query) = query else { return };
        self.view.filter_bar.container.set_visible(true);
        // set_text re-fires "changed", which re-applies the filter.
        self.view.filter_bar.entry.set_text(&query);
        self.view.filter_bar.entry.set_position(-1);
        if had_focus {
            // Hiding the entry during the reload took the focus with it.
            self.view.filter_bar.entry.grab_focus();
            self.view.filter_bar.entry.set_position(-1);
        }
    }

    /// Reload the settings-driven stylesheet in place (same provider, new
    /// content) instead of registering a fresh provider each time.
    pub fn update_css_for_colors(&self) {
        let s = self.hub.settings.borrow();
        let css_data = crate::theme::generate_css(&s);
        self.css_provider.load_from_data(&css_data);
    }
}
