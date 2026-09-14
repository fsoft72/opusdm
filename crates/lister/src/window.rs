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

/// Whether the keyboard focus currently sits in a text field, where a plain
/// letter is a character rather than a shortcut.
fn is_editing(window: &gtk::ApplicationWindow) -> bool {
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
    /// Filter list model wrapping store.
    #[allow(dead_code)]
    pub filter_model: gtk::FilterListModel,
    /// Whether dotfiles are shown in the file list. Off by default.
    pub show_hidden: Cell<bool>,
}

/// Everything the window's callbacks need. Shared by `Rc` across Tasks 13-15.
pub struct Ui {
    /// The top-level window.
    pub window: gtk::ApplicationWindow,
    /// Hub connection and role/settings state.
    pub hub: HubConnection,
    /// Current directory.
    pub cwd: RefCell<PathBuf>,
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
        crate::dlog!("activate");
        let mut guard = shared.borrow_mut();
        let Some(st) = guard.as_mut() else { return };
        // `connect_activate` should fire once for a NON_UNIQUE application,
        // but a stray re-activation must not crash an already-running window.
        let Some(inbox) = st.inbox.take() else {
            crate::dlog!("activate: ignoring re-activation, window already built");
            return;
        };
        let ui = build_window(app, st.client.clone(), st.id, st.roles.clone(), st.settings.clone());
        ui.navigate_to(&st.start);

        let ui_for_pump = ui.clone();
        glib::spawn_future_local(async move {
            while let Ok(inb) = inbox.recv().await {
                match inb {
                    Inbound::Msg(m) => ui_for_pump.apply_inbound(m),
                    Inbound::HubGone => {
                        crate::dlog!("hub connection lost, closing window");
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
    // Constant padding regardless of role - the SOURCE/DEST classes only
    // recolour, so NONE must not sit tighter to the edge.
    stat_bar.add_css_class("stat-bar");
    let stat_info = gtk::Label::new(Some("D: [0/0] F: [0/0] B: [0/0]"));
    stat_info.set_xalign(0.0);
    stat_info.set_hexpand(true);
    stat_bar.append(&stat_info);
    let star_btn = crate::icons::icon_button(crate::icons::SVG_STAR, "Open New Lister Here", 14);
    stat_bar.append(&star_btn);
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
    let view_toggle = gtk::ToggleButton::new();
    view_toggle.set_child(Some(&gtk::Label::new(Some("\u{25a6}"))));
    view_toggle.set_tooltip_text(Some("Toggle icon view"));
    view_toggle.add_css_class("flat");
    root.append(&button_row);

    // Model + filter + sorter + selection + column view.
    let store = gio::ListStore::new::<BoxedAnyObject>();
    let filter_bar = Rc::new(crate::filter::FilterBar::new());
    let filter_model = gtk::FilterListModel::new(Some(store.clone()), Some(filter_bar.filter.clone()));

    // Sorting is driven by the column headers, but the model does NOT use the
    // ColumnView's own sorter directly: that one reverses the whole comparator
    // for a descending sort, which would drop directories below files. Instead
    // the header state (which column, which direction) is read back and fed to
    // `dir_first_cmp`, which keeps directories as a fixed leading block and
    // reverses only the key within each block. Wired up once the columns exist.
    let sort_model = gtk::SortListModel::new(Some(filter_model.clone()), None::<gtk::Sorter>);
    let selection = gtk::MultiSelection::new(Some(sort_model.clone()));
    let column_view = gtk::ColumnView::new(Some(selection.clone()));
    column_view.set_vexpand(true);
    // Compact rows: default theme pads cells 8px 6px, data-table cuts it to 2px.
    column_view.add_css_class("data-table");

    let ui_slot: Rc<RefCell<Option<Rc<Ui>>>> = Rc::new(RefCell::new(None));
    let ui_getter = {
        let slot = ui_slot.clone();
        move || slot.borrow().clone()
    };

    let col_map: Rc<Vec<(gtk::ColumnViewColumn, SortCol)>> = Rc::new(vec![
        (
            crate::columns::add_column(&column_view, "Name", crate::columns::ColKind::Name, ui_getter.clone()),
            SortCol::Name,
        ),
        (
            crate::columns::add_column(&column_view, "Size", crate::columns::ColKind::Size, ui_getter.clone()),
            SortCol::Size,
        ),
        (
            crate::columns::add_column(&column_view, "Date", crate::columns::ColKind::Modified, ui_getter.clone()),
            SortCol::Modified,
        ),
        (
            crate::columns::add_column(&column_view, "User", crate::columns::ColKind::User, ui_getter.clone()),
            SortCol::User,
        ),
        (
            crate::columns::add_column(&column_view, "Group", crate::columns::ColKind::Group, ui_getter.clone()),
            SortCol::Group,
        ),
        (
            crate::columns::add_column(&column_view, "Access", crate::columns::ColKind::Access, ui_getter),
            SortCol::Access,
        ),
    ]);
    let name_col = col_map[0].0.clone();

    // The ColumnView's sorter tracks the active header (column + direction) and
    // draws the arrow indicator; the model sorter below only reads its state.
    let cv_sorter = column_view.sorter().and_downcast::<gtk::ColumnViewSorter>();

    let model_sorter = gtk::CustomSorter::new({
        let cv_sorter = cv_sorter.clone();
        let col_map = col_map.clone();
        move |a, b| {
            let ra = a.downcast_ref::<BoxedAnyObject>().unwrap().borrow::<Rc<Row>>();
            let rb = b.downcast_ref::<BoxedAnyObject>().unwrap().borrow::<Rc<Row>>();
            let (col, desc) = cv_sorter
                .as_ref()
                .and_then(|s| {
                    let active = s.primary_sort_column()?;
                    let col = col_map.iter().find(|(c, _)| *c == active).map(|(_, sc)| *sc)?;
                    Some((col, s.primary_sort_order() == gtk::SortType::Descending))
                })
                .unwrap_or((SortCol::Name, false));
            dir_first_cmp(&ra, &rb, col, desc).into()
        }
    });
    sort_model.set_sorter(Some(&model_sorter));

    // A header click changes column/direction without touching the row data,
    // so the model sorter has to be told its criteria moved.
    if let Some(cv_sorter) = &cv_sorter {
        let model_sorter = model_sorter.clone();
        cv_sorter.connect_changed(move |_, _| {
            model_sorter.changed(gtk::SorterChange::Different);
        });
    }
    column_view.sort_by_column(Some(&name_col), gtk::SortType::Ascending);

    root.append(&filter_bar.container);

    let scroller = gtk::ScrolledWindow::builder().child(&column_view).vexpand(true).build();

    let icon_field = opusdm_iconview::IconField::new();
    icon_field.set_mode(opusdm_iconview::LayoutMode::Grid);
    icon_field.set_icon_size(opusdm_iconview::DEFAULT_ICON_SIZE);
    icon_field.set_icon_set(settings.theme.icon_set.clone());

    let view_stack = gtk::Stack::new();
    view_stack.add_named(&scroller, Some("details"));
    view_stack.add_named(icon_field.widget(), Some("icons"));
    view_stack.set_visible_child_name("details");
    view_stack.set_vexpand(true);
    view_stack.set_margin_start(4);
    view_stack.set_margin_end(4);
    root.append(&view_stack);

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
        &gdk::Display::default().expect("no default GDK display: cannot run without one"),
        &css_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // One inotify instance per Lister, started even when auto-refresh is off so
    // toggling the setting does not need a restart. A kernel refusal (watch
    // limit) is not fatal: the Lister just loses live refresh.
    let debounce = std::time::Duration::from_millis(settings.behavior.debounce_ms());
    let (watcher, watch_rx) = match crate::watcher::DirWatcher::start(debounce) {
        Ok((w, rx)) => (Some(w), Some(rx)),
        Err(e) => {
            eprintln!("opusdm-lister: auto-refresh unavailable: {e}");
            (None, None)
        }
    };

    let ui = Rc::new(Ui {
        window: window.clone(),
        hub: HubConnection {
            client,
            id,
            roles: RefCell::new(roles),
            watcher,
            settings: RefCell::new(settings),
        },
        cwd: RefCell::new(PathBuf::from("/")),
        view: ViewState {
            mode: Cell::new(ViewMode::Details),
            stack: view_stack.clone(),
            icon_field: icon_field.clone(),
            icon_size: Cell::new(opusdm_iconview::DEFAULT_ICON_SIZE),
            column_view: column_view.clone(),
            selection: selection.clone(),
            store,
            cursor: Cell::new(0),
            filter_bar: filter_bar.clone(),
            filter_model,
            show_hidden: Cell::new(false),
        },
        status_bar: StatusBar {
            label: stat_info,
            container: stat_bar.clone(),
            role_badge,
            role_lock_btn: pin_btn,
            role_guard: Cell::new(false),
            job_line,
            totals: Cell::new((0, 0, 0)),
        },
        path_entry: path_entry.clone(),
        button_row: button_row.clone(),
        preview_win: RefCell::new(None),
        terminal: terminal.clone(),
        css_provider,
    });

    // Drain coalesced watch notices on the GTK thread.
    if let Some(watch_rx) = watch_rx {
        let ui = ui.clone();
        glib::spawn_future_local(async move {
            while let Ok(dir) = watch_rx.recv().await {
                ui.on_directory_changed(&dir);
            }
        });
    }
    *ui_slot.borrow_mut() = Some(ui.clone());
    terminal.attach_events(&ui);
    ui.update_css_for_colors();

    // Star button: open a new Lister at this Lister's current path.
    {
        let ui = ui.clone();
        star_btn.connect_clicked(move |_| {
            let _ = ui.hub.client.send(&ToHub::SpawnLister { start_path: Some(ui.cwd_path()) });
        });
    }

    // Click the role badge to cycle SOURCE -> DEST -> NONE. If SOURCE is
    // pinned by another Lister, a SetRole(Source) from here would just be
    // rejected every time (see roles.rs) - skip straight to DEST instead of
    // retrying a doomed request and looking stuck.
    {
        let ui_for_click = ui.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            let roles = ui_for_click.hub.roles.borrow();
            let is_source = roles.source == Some(ui_for_click.hub.id);
            let is_dest = roles.is_dest(ui_for_click.hub.id);
            let source_unreachable =
                roles.source_locked && roles.source != Some(ui_for_click.hub.id);
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
            let _ = ui_for_click.hub.client.send(&ToHub::SetRole(next));
        });
        ui.status_bar.role_badge.add_controller(gesture);
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
        column_view.connect_activate(move |_, position| ui.activate_row(position));
    }

    // Selection changes update the status line and active preview if open.
    {
        let ui = ui.clone();
        selection.connect_selection_changed(move |_, _, _| {
            ui.sync_cursor_from_selection();
            ui.update_status();
            ui.update_preview_if_open();
        });
    }

    // Focus -> notify the hub.
    {
        let ui = ui.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(move |_| {
            let _ = ui.hub.client.send(&ToHub::FocusGained);
        });
        window.add_controller(focus);
    }

    // Window close -> Goodbye, let the process exit.
    {
        let ui = ui.clone();
        window.connect_close_request(move |_| {
            crate::dlog!("close request, sending Goodbye to hub");
            let _ = ui.hub.client.send(&ToHub::Goodbye);
            glib::Propagation::Proceed
        });
    }

    // Keyboard shortcuts. The binding table itself lives in `keys.rs`; this
    // only turns the resolved action into a call.
    {
        let ui_for_keys = ui.clone();
        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.connect_key_pressed(move |_, keyval, _, state| {
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            // Single-letter bindings must not steal characters from a text
            // field, so where the focus sits decides how a key is read.
            let focus = if is_editing(&ui_for_keys.window) {
                crate::keys::Focus::Editing
            } else {
                crate::keys::Focus::List
            };
            let Some(action) = crate::keys::resolve(keyval, ctrl, shift, focus) else {
                return glib::Propagation::Proceed;
            };
            ui_for_keys.run_key_action(action);
            glib::Propagation::Stop
        });
        window.add_controller(key_ctrl);
    }

    crate::actions::attach(&ui);
    button_row.append(&view_toggle);
    crate::dnd::attach(&ui, &column_view);

    // Icon-view toggle: swap the stack child and rebuild entries on the way in.
    {
        let ui = ui.clone();
        view_toggle.connect_toggled(move |b| {
            let on = b.is_active();
            ui.view.mode.set(if on { ViewMode::Icons } else { ViewMode::Details });
            ui.view.stack
                .set_visible_child_name(if on { "icons" } else { "details" });
            if on {
                ui.refresh_icon_view();
            }
        });
    }

    // Icon-view callbacks: reuse the column view's handlers where they exist.
    crate::dnd::attach_field(&ui, &ui.view.icon_field);
    let icon_field = ui.view.icon_field.clone();
    {
        let ui = ui.clone();
        icon_field.connect_activate(move |i| ui.activate_row(i as u32));
    }
    {
        let ui = ui.clone();
        let field = icon_field.clone();
        icon_field.connect_rename_requested(move |i, new_name| {
            let Some(entry) = field.entry(i) else { return };
            let new_path = ui.cwd_path().join(&new_name);
            if let Err(e) = std::fs::rename(&entry.path, &new_path) {
                ui.status_bar.job_line.set_text(&format!("rename failed: {e}"));
            } else {
                ui.rename_row(&entry.name, &new_name);
            }
        });
    }
    {
        let ui = ui.clone();
        let field = icon_field.clone();
        icon_field.connect_context_menu(move |idx, x, y| {
            if idx < 0 {
                return;
            }
            let Some(row) = ui.row_at(idx as u32) else { return };
            let target = ui.cwd_path().join(&row.name);
            crate::context_menu::show_context_menu(&ui, field.widget(), &row, target, x, y);
        });
    }
    {
        let ui = ui.clone();
        let field = icon_field.clone();
        icon_field.connect_zoom_requested(move |delta| {
            use opusdm_iconview::ICON_ZOOM_STEPS as STEPS;
            let cur = ui.view.icon_size.get();
            // If icon_size was ever set to a value outside STEPS, snap to the
            // nearest step instead of silently resetting to a fixed index.
            let idx = STEPS
                .iter()
                .position(|&s| s == cur)
                .unwrap_or_else(|| {
                    STEPS
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, &s)| (s - cur).abs())
                        .map(|(i, _)| i)
                        .unwrap_or(0)
                }) as i32;
            let next = (idx + delta).clamp(0, STEPS.len() as i32 - 1) as usize;
            ui.view.icon_size.set(STEPS[next]);
            field.set_icon_size(STEPS[next]);
        });
    }
    ui.update_status();
    ui
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
        crate::dlog!("navigate_to {}", dir.display());
        let rows = if let Some((arch, inner)) = crate::archive::parse_archive_path(dir) {
            match crate::archive::read_archive_rows(&arch, &inner) {
                Ok(r) => r,
                Err(e) => {
                    self.status_bar.job_line.set_text(&format!("cannot read archive {}: {e}", arch.display()));
                    return;
                }
            }
        } else {
            match read_dir_rows(dir) {
                Ok(r) => r,
                Err(e) => {
                    self.status_bar.job_line.set_text(&format!("cannot open {}: {e}", dir.display()));
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
        self.refresh_icon_view();
        let _ = self.hub.client.send(&ToHub::PathChanged(dir.to_path_buf()));
    }

    /// Rebuild the icon view's entries from the current sorted model. A no-op
    /// unless the icon view is the active one.
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
        self.view.icon_field.set_entries(&entries);
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

    /// Re-read the current directory, keeping the selection by name.
    pub fn reload(self: &Rc<Self>) {
        let selected = self.selected_names();
        let cwd = self.cwd.borrow().clone();
        self.navigate_to(&cwd);
        if selected.is_empty() {
            return;
        }
        for i in 0..self.view.selection.n_items() {
            if let Some(row) = self.row_at(i) {
                if selected.contains(&row.name) {
                    self.view.selection.select_item(i, false);
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

    fn refresh_path_entry(&self) {
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

        self.status_bar.label.set_text(&format!(
            "D: [{}/{}]  F: [{}/{}]  B: [{}/{}]",
            sel_dirs, total_dirs,
            sel_files, total_files,
            sel_bytes, total_bytes
        ));
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
                if p == *self.cwd.borrow() {
                    self.reload();
                }
            }
            ToLister::Present => self.window.present(),
            ToLister::Shutdown => self.window.close(),
            // Progress is shown centrally by the hub's progress window now.
            ToLister::JobProgress { .. } => {}
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
                let names_changed = {
                    let old = self.hub.settings.borrow();
                    old.theme.icon_set != new_settings.theme.icon_set
                        || old.appearance.type_indicators != new_settings.appearance.type_indicators
                };
                *self.hub.settings.borrow_mut() = new_settings;
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
        }
    }

    /// Carry out a binding resolved by [`crate::keys::resolve`].
    pub fn run_key_action(self: &Rc<Self>, action: crate::keys::Action) {
        use crate::keys::Action;
        match action {
            Action::Command(id) => crate::palette::execute_command(self, id),
            Action::Move(motion) => self.move_cursor(motion),
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
            self.view.selection.unselect_all();
            return;
        }
        self.view.column_view.grab_focus();
    }

    /// Open the row at `position`: descend into a directory, step into an
    /// archive, or hand the file to its default application.
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
        if row.is_dir {
            self.navigate_to(&target);
        } else if crate::archive::is_archive_file(&target) {
            self.navigate_to(&crate::archive::make_archive_path(&target, ""));
        } else {
            let uri = format!("file://{}", target.to_string_lossy());
            let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
        }
    }

    /// Move the keyboard cursor and make the row it lands on the only
    /// selection, the way arrow keys already behave.
    pub fn move_cursor(&self, motion: crate::keys::Motion) {
        use crate::keys::Motion;
        let n = self.view.selection.n_items();
        if n == 0 {
            return;
        }
        let last = n - 1;
        let here = self.view.cursor.get().min(last);
        let pos = match motion {
            Motion::Down => (here + 1).min(last),
            Motion::Up => here.saturating_sub(1),
            Motion::First => 0,
            Motion::Last => last,
        };
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
    fn sync_cursor_from_selection(&self) {
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
        if enabled && crate::archive::parse_archive_path(dir).is_none() {
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
    fn on_directory_changed(self: &Rc<Self>, dir: &Path) {
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
    fn update_css_for_colors(&self) {
        let s = self.hub.settings.borrow();
        let css_data = crate::theme::generate_css(&s);
        self.css_provider.load_from_data(&css_data);
    }
}
