//! The Lister window: path bar, column view, status line, navigation.

use crate::client::{Client, Inbound, Roles};
use crate::model::{dir_first_cmp, read_dir_rows, Row, SortCol};
use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::glib::{self, BoxedAnyObject};
use gtk::prelude::*;
use opusdm_ipc::{ListerId, Role, Settings, ToHub, ToLister};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Everything the window's callbacks need. Shared by `Rc` across Tasks 13-15.
pub struct Ui {
    /// The top-level window.
    pub window: gtk::ApplicationWindow,
    /// Hub connection (send half).
    pub client: Rc<Client>,
    /// This Lister's id.
    pub id: ListerId,
    /// Current directory.
    pub cwd: RefCell<PathBuf>,
    /// Backing store of `BoxedAnyObject`-wrapped `Rc<Row>`.
    pub store: gio::ListStore,
    /// Selection model feeding the column view.
    pub selection: gtk::MultiSelection,
    /// The single sorter driven by the sort buttons.
    pub sorter: gtk::CustomSorter,
    /// Status line (top bar with D/F/B counts).
    pub status: gtk::Label,
    /// Stat bar box (for background color based on role).
    pub stat_bar: gtk::Box,
    /// SOURCE / DEST badge.
    pub role_badge: gtk::Label,
    /// Latest known global roles.
    pub roles: RefCell<Roles>,
    /// Editable path bar.
    pub path_entry: gtk::Entry,
    /// Cached directory totals: (total_dirs, total_files, total_bytes)
    pub totals: Cell<(u32, u32, u64)>,
    /// Pin: locks this Lister's current role against focus changes (stat bar).
    pub role_lock_btn: gtk::ToggleButton,
    /// Guards against feedback while syncing toggle state from the hub.
    pub role_guard: Cell<bool>,
    /// In-window transient message line (errors, job status).
    pub job_line: gtk::Label,
    /// Empty box the button row is built into by `actions::attach`.
    pub button_row: gtk::Box,
    /// Current settings (colors, and whatever else joins later). Updated
    /// wholesale from `Welcome` and `SettingsUpdate`.
    pub settings: RefCell<Settings>,
    /// Whether dotfiles are shown in the file list. Off by default.
    pub show_hidden: Cell<bool>,
    /// Filter bar widget (search entry, filter model, etc.)
    pub filter_bar: Rc<crate::filter::FilterBar>,
    /// Filter list model wrapping store.
    #[allow(dead_code)]
    pub filter_model: gtk::FilterListModel,
    /// Active Quick Look preview window, if open.
    pub preview_win: RefCell<Option<Rc<crate::preview::PreviewWindow>>>,
    /// Quake-style dropdown terminal drawer.
    pub terminal: Rc<crate::terminal::TerminalDrawer>,
    /// Single stylesheet for settings-driven colors; reloaded in place on
    /// change rather than re-registered, so providers don't stack up on the
    /// display.
    pub css_provider: gtk::CssProvider,
}

/// Build and run the window application.
pub fn run(
    mut client: Client,
    id: ListerId,
    roles: Roles,
    settings: Settings,
    start: PathBuf,
) -> glib::ExitCode {
    let inbox = client.take_inbox();
    let client = Rc::new(client);
    let shared: Rc<RefCell<Option<StartState>>> = Rc::new(RefCell::new(Some(StartState {
        client,
        id,
        roles,
        settings,
        start,
        inbox: Some(inbox),
    })));

    let app = gtk::Application::builder()
        .application_id("org.opusdm.Lister")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        let mut guard = shared.borrow_mut();
        let Some(st) = guard.as_mut() else { return };
        let inbox = st.inbox.take().expect("activate once");
        let ui = build_window(app, st.client.clone(), st.id, st.roles.clone(), st.settings.clone());
        ui.navigate_to(&st.start);

        let ui_for_pump = ui.clone();
        glib::spawn_future_local(async move {
            while let Ok(inb) = inbox.recv().await {
                match inb {
                    Inbound::Msg(m) => ui_for_pump.apply_inbound(m),
                    Inbound::HubGone => {
                        ui_for_pump.window.close();
                        break;
                    }
                }
            }
        });

        ui.window.present();
    });

    app.run_with_args::<&str>(&[])
}

struct StartState {
    client: Rc<Client>,
    id: ListerId,
    roles: Roles,
    settings: Settings,
    start: PathBuf,
    inbox: Option<async_channel::Receiver<Inbound>>,
}

fn build_window(
    app: &gtk::Application,
    client: Rc<Client>,
    id: ListerId,
    roles: Roles,
    settings: Settings,
) -> Rc<Ui> {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Lister")
        .default_width(800)
        .default_height(600)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);

    // Top stat bar: D/F/B counts (left) + pin + SOURCE/DEST badge (right).
    // Click the badge to cycle SOURCE -> DEST -> NONE.
    let stat_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let stat_info = gtk::Label::new(Some("D: [0/0] F: [0/0] B: [0/0]"));
    stat_info.set_xalign(0.0);
    stat_info.set_hexpand(true);
    stat_bar.append(&stat_info);
    let pin_btn = crate::icons::icon_toggle_button(crate::icons::SVG_PIN, "Pin / Lock Role", 14);
    pin_btn.add_css_class("pin-btn");
    stat_bar.append(&pin_btn);
    let role_badge = gtk::Label::new(Some("NONE"));
    // Fixed pixel width so it never reflows between SOURCE/DEST/NONE -
    // width_chars is font-metric dependent and still shifted.
    role_badge.set_size_request(60, -1);
    role_badge.set_xalign(0.5); // center text within that fixed width
    stat_bar.append(&role_badge);
    root.append(&stat_bar);

    // Button row (filled by actions::attach).
    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    button_row.set_margin_start(4);
    button_row.set_margin_end(4);
    root.append(&button_row);

    // Model + filter + sorter + selection + column view.
    let store = gio::ListStore::new::<BoxedAnyObject>();
    let filter_bar = Rc::new(crate::filter::FilterBar::new());
    let filter_model = gtk::FilterListModel::new(Some(store.clone()), Some(filter_bar.filter.clone()));

    let sort_cell = Rc::new(Cell::new((SortCol::Name, false)));
    let sorter = gtk::CustomSorter::new({
        let sort_cell = sort_cell.clone();
        move |a, b| {
            let ra = a.downcast_ref::<BoxedAnyObject>().unwrap().borrow::<Rc<Row>>().clone();
            let rb = b.downcast_ref::<BoxedAnyObject>().unwrap().borrow::<Rc<Row>>().clone();
            let (col, desc) = sort_cell.get();
            dir_first_cmp(&ra, &rb, col, desc).into()
        }
    });
    let sort_model = gtk::SortListModel::new(Some(filter_model.clone()), Some(sorter.clone()));
    let selection = gtk::MultiSelection::new(Some(sort_model));
    let column_view = gtk::ColumnView::new(Some(selection.clone()));
    column_view.set_vexpand(true);
    // Compact rows: default theme pads cells 8px 6px, data-table cuts it to 2px.
    column_view.add_css_class("data-table");

    let ui_slot: Rc<RefCell<Option<Rc<Ui>>>> = Rc::new(RefCell::new(None));
    let ui_getter = {
        let slot = ui_slot.clone();
        move || slot.borrow().clone()
    };

    crate::columns::add_column(&column_view, "Name", crate::columns::ColKind::Name, ui_getter.clone());
    crate::columns::add_column(&column_view, "Size", crate::columns::ColKind::Size, ui_getter.clone());
    crate::columns::add_column(&column_view, "Date", crate::columns::ColKind::Modified, ui_getter.clone());
    crate::columns::add_column(&column_view, "User", crate::columns::ColKind::User, ui_getter.clone());
    crate::columns::add_column(&column_view, "Group", crate::columns::ColKind::Group, ui_getter.clone());
    crate::columns::add_column(&column_view, "Access", crate::columns::ColKind::Access, ui_getter);

    root.append(&filter_bar.container);

    let scroller = gtk::ScrolledWindow::builder().child(&column_view).vexpand(true).build();
    scroller.set_margin_start(4);
    scroller.set_margin_end(4);
    root.append(&scroller);

    // Filter entry interaction
    {
        let count_label = filter_bar.count_label.clone();
        let filter_model_for_count = filter_model.clone();
        let store_for_count = store.clone();
        let q = filter_bar.query.clone();
        let filt = filter_bar.filter.clone();
        filter_bar.entry.connect_search_changed(move |e| {
            *q.borrow_mut() = e.text().to_string();
            filt.changed(gtk::FilterChange::Different);
            let filtered = filter_model_for_count.n_items();
            let total = store_for_count.n_items();
            if e.text().is_empty() {
                count_label.set_text("");
            } else {
                count_label.set_text(&format!("{filtered}/{total}"));
            }
        });
    }
    {
        let selection_clone = selection.clone();
        let column_view_clone = column_view.clone();
        filter_bar.entry.connect_activate(move |_| {
            if selection_clone.n_items() > 0 {
                selection_clone.select_item(0, false);
                column_view_clone.grab_focus();
            }
        });
    }
    {
        let filter_bar_clone = filter_bar.clone();
        let column_view_clone = column_view.clone();
        filter_bar.entry.connect_stop_search(move |_| {
            filter_bar_clone.dismiss();
            column_view_clone.grab_focus();
        });
    }

    // Quake-style dropdown terminal drawer
    let terminal = crate::terminal::TerminalDrawer::new();
    root.append(&terminal.container);

    // Transient message line.
    let job_line = gtk::Label::new(None);
    job_line.set_xalign(0.0);
    job_line.set_vexpand(false);
    root.append(&job_line);

    // Bottom navigation: home, parent, path.
    let nav_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    nav_bar.set_vexpand(false);
    nav_bar.set_margin_bottom(4);
    nav_bar.set_margin_start(4);
    nav_bar.set_margin_end(4);
    let home_btn = crate::icons::icon_button(crate::icons::SVG_HOME, "Home Directory", 16);
    let parent_btn = crate::icons::icon_button(crate::icons::SVG_PARENT, "Parent Directory", 16);
    let path_entry = gtk::Entry::new();
    path_entry.set_hexpand(true);
    nav_bar.append(&home_btn);
    nav_bar.append(&parent_btn);
    nav_bar.append(&path_entry);
    root.append(&nav_bar);

    window.set_child(Some(&root));

    // Registered once; update_css_for_colors() reloads its content in place.
    let css_provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().unwrap(),
        &css_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let ui = Rc::new(Ui {
        window: window.clone(),
        client,
        id,
        cwd: RefCell::new(PathBuf::from("/")),
        store,
        filter_bar: filter_bar.clone(),
        filter_model,
        selection: selection.clone(),
        sorter: sorter.clone(),
        status: stat_info,
        stat_bar: stat_bar.clone(),
        role_badge,
        roles: RefCell::new(roles),
        path_entry: path_entry.clone(),
        totals: Cell::new((0, 0, 0)),
        role_lock_btn: pin_btn,
        role_guard: Cell::new(false),
        preview_win: RefCell::new(None),
        terminal: terminal.clone(),
        job_line,
        button_row: button_row.clone(),
        settings: RefCell::new(settings),
        show_hidden: Cell::new(false),
        css_provider,
    });
    *ui_slot.borrow_mut() = Some(ui.clone());
    terminal.attach_events(&ui);
    ui.update_css_for_colors();

    // Click the role badge to cycle SOURCE -> DEST -> NONE. If SOURCE is
    // pinned by another Lister, a SetRole(Source) from here would just be
    // rejected every time (see roles.rs) - skip straight to DEST instead of
    // retrying a doomed request and looking stuck.
    {
        let ui_for_click = ui.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            let roles = ui_for_click.roles.borrow();
            let is_source = roles.source == Some(ui_for_click.id);
            let is_dest = roles.is_dest(ui_for_click.id);
            let source_unreachable =
                roles.source_locked && roles.source != Some(ui_for_click.id);
            drop(roles);
            let next = if is_source {
                Role::Dest
            } else if is_dest {
                Role::None
            } else if source_unreachable {
                Role::Dest
            } else {
                Role::Source
            };
            let _ = ui_for_click.client.send(&ToHub::SetRole(next));
        });
        ui.role_badge.add_controller(gesture);
    }

    // Navigation.
    {
        let ui = ui.clone();
        parent_btn.connect_clicked(move |_| {
            let cwd = ui.cwd.borrow().clone();
            if let Some((arch, inner)) = crate::archive::parse_archive_path(&cwd) {
                if inner.is_empty() {
                    if let Some(p) = arch.parent() {
                        ui.navigate_to(p);
                    }
                } else {
                    let parent_inner = Path::new(&inner)
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.navigate_to(&crate::archive::make_archive_path(&arch, &parent_inner));
                }
            } else if let Some(p) = cwd.parent().map(PathBuf::from) {
                ui.navigate_to(&p);
            }
        });
    }
    {
        let ui = ui.clone();
        home_btn.connect_clicked(move |_| {
            if let Ok(home) = std::env::var("HOME") {
                ui.navigate_to(Path::new(&home));
            }
        });
    }
    {
        let ui = ui.clone();
        path_entry.connect_activate(move |entry| {
            let candidate = PathBuf::from(entry.text().to_string());
            let is_arch_sub = crate::archive::parse_archive_path(&candidate).is_some();
            let is_arch_file = crate::archive::is_archive_file(&candidate) && candidate.is_file();
            if candidate.is_dir() || is_arch_sub || is_arch_file {
                if is_arch_file {
                    ui.navigate_to(&crate::archive::make_archive_path(&candidate, ""));
                } else {
                    ui.navigate_to(&candidate);
                }
            } else {
                entry.add_css_class("error");
                let entry = entry.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
                    entry.remove_css_class("error");
                });
                ui.refresh_path_entry();
            }
        });
    }

    // Row activation: enter directory, open archive, or open file.
    {
        let ui = ui.clone();
        column_view.connect_activate(move |_, position| {
            if let Some(row) = ui.row_at(position) {
                let cwd = ui.cwd.borrow().clone();
                if let Some((arch, inner)) = crate::archive::parse_archive_path(&cwd) {
                    let next_inner = if inner.is_empty() {
                        row.name.clone()
                    } else {
                        format!("{}/{}", inner.trim_end_matches('/'), row.name)
                    };
                    if row.is_dir {
                        ui.navigate_to(&crate::archive::make_archive_path(&arch, &next_inner));
                    } else if let Ok(temp_path) = crate::archive::extract_single_file_temp(&arch, &next_inner) {
                        let uri = format!("file://{}", temp_path.to_string_lossy());
                        let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
                    }
                } else {
                    let target = cwd.join(&row.name);
                    if row.is_dir {
                        ui.navigate_to(&target);
                    } else if crate::archive::is_archive_file(&target) {
                        ui.navigate_to(&crate::archive::make_archive_path(&target, ""));
                    } else {
                        let uri = format!("file://{}", target.to_string_lossy());
                        let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
                    }
                }
            }
        });
    }

    // Selection changes update the status line and active preview if open.
    {
        let ui = ui.clone();
        selection.connect_selection_changed(move |_, _, _| {
            ui.update_status();
            ui.update_preview_if_open();
        });
    }

    // Focus -> notify the hub.
    {
        let ui = ui.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(move |_| {
            let _ = ui.client.send(&ToHub::FocusGained);
        });
        window.add_controller(focus);
    }

    // Window close -> Goodbye, let the process exit.
    {
        let ui = ui.clone();
        window.connect_close_request(move |_| {
            let _ = ui.client.send(&ToHub::Goodbye);
            glib::Propagation::Proceed
        });
    }

    // Keyboard shortcuts: /, Ctrl+P, Ctrl+F, Ctrl+A, Ctrl+R, Ctrl+H, Esc, etc.
    {
        let ui_for_keys = ui.clone();
        let column_view_for_keys = column_view.clone();
        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.connect_key_pressed(move |_, keyval, _, state| {
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

            // Ctrl shortcuts (work everywhere)
            if ctrl {
                match keyval {
                    gdk::Key::p | gdk::Key::P => {
                        crate::palette::open_command_palette(&ui_for_keys);
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::f | gdk::Key::F => {
                        ui_for_keys.reveal_filter();
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::a | gdk::Key::A => {
                        ui_for_keys.selection.select_all();
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::r | gdk::Key::R => {
                        ui_for_keys.reload();
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::h | gdk::Key::H => {
                        let active = !ui_for_keys.show_hidden.get();
                        ui_for_keys.show_hidden.set(active);
                        ui_for_keys.reload();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
            }

            // Global F4 shortcut for terminal drawer
            if keyval == gdk::Key::F4 {
                ui_for_keys.toggle_terminal();
                return glib::Propagation::Stop;
            }

            // Check if focus is inside an editable field
            let is_editing = gtk::prelude::RootExt::focus(&ui_for_keys.window)
                .map(|w| w.is::<gtk::Editable>() || w.is::<gtk::TextView>())
                .unwrap_or(false);

            if !is_editing {
                match keyval {
                    gdk::Key::grave | gdk::Key::asciitilde => {
                        ui_for_keys.toggle_terminal();
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::F3 => {
                        ui_for_keys.toggle_preview();
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::space => {
                        let paths = ui_for_keys.selected_paths();
                        let has_dir = paths.iter().any(|p| p.is_dir());
                        if has_dir {
                            crate::tools::calculate_dir_sizes_async(&ui_for_keys);
                        } else {
                            ui_for_keys.toggle_preview();
                        }
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::slash => {
                        ui_for_keys.reveal_filter();
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::asterisk => {
                        crate::palette::execute_command(&ui_for_keys, "invert_selection");
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::plus => {
                        crate::palette::execute_command(&ui_for_keys, "select_pattern");
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::minus => {
                        crate::palette::execute_command(&ui_for_keys, "deselect_pattern");
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::F2 => {
                        crate::palette::execute_command(&ui_for_keys, "rename");
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::F7 if !shift => {
                        crate::palette::execute_command(&ui_for_keys, "mkdir");
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::F7 if shift => {
                        crate::palette::execute_command(&ui_for_keys, "newfile");
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::Escape => {
                        if ui_for_keys.filter_bar.container.is_visible() {
                            ui_for_keys.dismiss_filter();
                            column_view_for_keys.grab_focus();
                            return glib::Propagation::Stop;
                        } else if ui_for_keys.terminal.container.is_visible() {
                            ui_for_keys.terminal.container.set_visible(false);
                            column_view_for_keys.grab_focus();
                            return glib::Propagation::Stop;
                        } else {
                            ui_for_keys.selection.unselect_all();
                            return glib::Propagation::Stop;
                        }
                    }
                    _ => {}
                }
            } else {
                if keyval == gdk::Key::Escape {
                    if ui_for_keys.filter_bar.container.is_visible() {
                        ui_for_keys.dismiss_filter();
                        column_view_for_keys.grab_focus();
                        return glib::Propagation::Stop;
                    } else if ui_for_keys.terminal.container.is_visible() {
                        ui_for_keys.terminal.container.set_visible(false);
                        column_view_for_keys.grab_focus();
                        return glib::Propagation::Stop;
                    }
                }
            }

            glib::Propagation::Proceed
        });
        window.add_controller(key_ctrl);
    }

    crate::actions::attach(&ui);
    crate::dnd::attach(&ui, &column_view);
    ui.update_status();
    ui
}

impl Ui {
    /// Reveal and focus the filter bar.
    pub fn reveal_filter(&self) {
        self.filter_bar.reveal();
    }

    /// Dismiss and reset the filter bar.
    pub fn dismiss_filter(&self) {
        self.filter_bar.dismiss();
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

    /// Read `dir`, repopulate the store, update title/entry/status, notify the hub.
    pub fn navigate_to(self: &Rc<Self>, dir: &Path) {
        let rows = if let Some((arch, inner)) = crate::archive::parse_archive_path(dir) {
            match crate::archive::read_archive_rows(&arch, &inner) {
                Ok(r) => r,
                Err(e) => {
                    self.job_line.set_text(&format!("cannot read archive {}: {e}", arch.display()));
                    return;
                }
            }
        } else {
            match read_dir_rows(dir) {
                Ok(r) => r,
                Err(e) => {
                    self.job_line.set_text(&format!("cannot open {}: {e}", dir.display()));
                    return;
                }
            }
        };
        self.dismiss_filter();
        if let Some((arch, _)) = crate::archive::parse_archive_path(dir) {
            if let Some(parent) = arch.parent() {
                self.terminal.sync_cwd(parent);
            }
        } else {
            self.terminal.sync_cwd(dir);
        }
        *self.cwd.borrow_mut() = dir.to_path_buf();
        let show_hidden = self.show_hidden.get();
        let mut total_dirs = 0u32;
        let mut total_files = 0u32;
        let mut total_bytes = 0u64;
        let mut new_items = Vec::new();
        for row in rows {
            if !show_hidden && row.name.starts_with('.') {
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
        self.totals.set((total_dirs, total_files, total_bytes));
        let old_len = self.store.n_items();
        self.store.splice(0, old_len, &new_items);
        self.sorter.changed(gtk::SorterChange::Different);
        self.window.set_title(Some(&dir.to_string_lossy()));
        self.refresh_path_entry();
        self.update_status();
        let _ = self.client.send(&ToHub::PathChanged(dir.to_path_buf()));
    }

    /// Re-read the current directory, keeping the selection by name.
    pub fn reload(self: &Rc<Self>) {
        let selected = self.selected_names();
        let cwd = self.cwd.borrow().clone();
        self.navigate_to(&cwd);
        if selected.is_empty() {
            return;
        }
        for i in 0..self.selection.n_items() {
            if let Some(row) = self.row_at(i) {
                if selected.contains(&row.name) {
                    self.selection.select_item(i, false);
                }
            }
        }
    }

    /// Absolute paths of the selected rows.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let cwd = self.cwd.borrow().clone();
        self.selected_names().into_iter().map(|n| cwd.join(n)).collect()
    }

    /// Names of all currently selected rows.
    pub fn selected_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for i in 0..self.selection.n_items() {
            if self.selection.is_selected(i) {
                if let Some(row) = self.row_at(i) {
                    names.push(row.name.clone());
                }
            }
        }
        names
    }

    /// Retrieve the row model at `position` in the visual list.
    pub fn row_at(&self, position: u32) -> Option<Rc<Row>> {
        self.selection
            .item(position)
            .and_downcast::<BoxedAnyObject>()
            .map(|o| o.borrow::<Rc<Row>>().clone())
    }

    fn refresh_path_entry(&self) {
        self.path_entry.set_text(&self.cwd.borrow().to_string_lossy());
    }

    /// Current directory (clone).
    pub fn cwd_path(&self) -> PathBuf {
        self.cwd.borrow().clone()
    }

    /// Recompute the status line and the role badge.
    pub fn update_status(&self) {
        let (total_dirs, total_files, total_bytes) = self.totals.get();
        let mut sel_dirs = 0u32;
        let mut sel_files = 0u32;
        let mut sel_bytes = 0u64;

        for i in 0..self.selection.n_items() {
            if self.selection.is_selected(i) {
                if let Some(row) = self.row_at(i) {
                    if row.is_dir {
                        sel_dirs += 1;
                    } else {
                        sel_files += 1;
                        sel_bytes += row.size;
                    }
                }
            }
        }

        let roles = self.roles.borrow();
        let (badge, stat_class) = if roles.source == Some(self.id) {
            ("SOURCE", "stat-source")
        } else if roles.is_dest(self.id) {
            ("DEST", "stat-dest")
        } else {
            ("NONE", "")
        };
        self.role_badge.set_text(badge);

        // Update stat bar background color based on role.
        self.stat_bar.remove_css_class("stat-source");
        self.stat_bar.remove_css_class("stat-dest");
        if !stat_class.is_empty() {
            self.stat_bar.add_css_class(stat_class);
        }
        drop(roles);

        self.status.set_text(&format!(
            "D: [{}/{}]  F: [{}/{}]  B: [{}/{}]",
            sel_dirs, total_dirs,
            sel_files, total_files,
            sel_bytes, total_bytes
        ));
    }

    /// Handle a message from the hub.
    pub fn apply_inbound(self: &Rc<Self>, msg: ToLister) {
        match msg {
            ToLister::RoleChanged { source, dests, source_locked } => {
                *self.roles.borrow_mut() = Roles { source, dests, source_locked };
                self.update_status();
                crate::actions::sync_role_buttons(self);
            }
            ToLister::InvalidatePath(p) => {
                if p == *self.cwd.borrow() {
                    self.reload();
                }
            }
            ToLister::Present => self.window.present(),
            ToLister::Shutdown => self.window.close(),
            ToLister::JobProgress { job_id, bytes_done, bytes_total, current_file, .. } => {
                crate::actions::on_job_progress(self, job_id, bytes_done, bytes_total, &current_file);
            }
            ToLister::JobDone { job_id, outcome, errors } => {
                crate::actions::on_job_done(self, job_id, &outcome, &errors);
            }
            ToLister::ConflictPrompt { req_id, path, src, dst, .. } => {
                crate::actions::on_conflict(self, req_id, &path, src, dst);
            }
            ToLister::Welcome { .. } => {
                // Consumed synchronously in Client::connect(); never reaches here.
            }
            ToLister::SettingsUpdate(new_settings) => {
                *self.settings.borrow_mut() = new_settings;
                self.update_css_for_colors();
            }
            ToLister::Pong => {}
        }
    }

    /// Reload the settings-driven stylesheet in place (same provider, new
    /// content) instead of registering a fresh provider each time.
    fn update_css_for_colors(&self) {
        let s = self.settings.borrow();
        let css_data = crate::theme::generate_css(&s);
        self.css_provider.load_from_data(&css_data);
    }
}
