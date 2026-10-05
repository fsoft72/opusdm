//! Constructing the Lister window: widgets and the signal wiring that
//! connects them to `Ui` behavior. Behavior itself lives in `window.rs`.

use crate::client::{Client, Inbound, Roles};
use crate::model::{dir_first_cmp, Row, SortCol};
use crate::window::{Caches, FormatState, HubConnection, StatusBar, Typeahead, Ui, ViewMode, ViewState};
use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::glib::{self, BoxedAnyObject};
use gtk::prelude::*;
use opusdm_ipc::{ListerId, Settings, ToHub};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// How long the path entry's "error" CSS class (invalid path flash) stays on.
const PATH_ENTRY_ERROR_FLASH_MS: u64 = 600;

/// Build and run the window application.
pub fn run(
    mut client: Client,
    id: ListerId,
    roles: Roles,
    settings: Settings,
    bindings: opusdm_ipc::Bindings,
    formats: opusdm_ipc::FormatStore,
    functions: Vec<opusdm_ipc::function::Function>,
    start: PathBuf,
) -> glib::ExitCode {
    let inbox = client.take_inbox();
    let client = Rc::new(client);
    let shared: Rc<RefCell<Option<StartState>>> = Rc::new(RefCell::new(Some(StartState {
        client,
        id,
        roles,
        settings,
        bindings,
        formats,
        functions,
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
        // GTK is only initialized once `connect_activate` actually fires,
        // so `gtk::Settings::default()` (used by `apply_theme_mode`) must
        // not be called any earlier - it panics with "GTK has not been
        // initialized" before this point.
        opusdm_ui::apply_theme_mode(st.settings.appearance.theme_mode);
        let ui = build_window(app, st.client.clone(), st.id, st.roles.clone(), st.settings.clone(), st.bindings.clone(), st.formats.clone(), st.functions.clone());
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
    bindings: opusdm_ipc::Bindings,
    formats: opusdm_ipc::FormatStore,
    functions: Vec<opusdm_ipc::function::Function>,
    start: PathBuf,
    inbox: Option<async_channel::Receiver<Inbound>>,
}

/// The top stat bar: D/F/B counts, gauge, "new Lister" star, role pin and the
/// SOURCE / DEST badge.
struct StatBarWidgets {
    container: gtk::Box,
    info: gtk::Label,
    gauge: gtk::LevelBar,
    star_btn: gtk::Button,
    pin_btn: gtk::ToggleButton,
    role_badge: gtk::Label,
}

/// Build the top stat bar: D/F/B counts (left) + pin + SOURCE/DEST badge
/// (right). Click the badge to cycle SOURCE -> DEST -> NONE.
fn build_stat_bar() -> StatBarWidgets {
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    // Constant padding regardless of role - the SOURCE/DEST classes only
    // recolour, so NONE must not sit tighter to the edge.
    container.add_css_class("stat-bar");
    let info = gtk::Label::new(Some("D: [0/0] F: [0/0] B: [0/0]"));
    info.set_xalign(0.0);
    info.set_hexpand(true);
    container.append(&info);
    let gauge = gtk::LevelBar::for_interval(0.0, 1.0);
    gauge.set_size_request(90, -1);
    gauge.set_valign(gtk::Align::Center);
    gauge.set_visible(false);
    container.append(&gauge);
    let star_btn = crate::icons::icon_button(crate::icons::SVG_STAR, "Open New Lister Here", 14);
    container.append(&star_btn);
    let pin_btn = crate::icons::icon_toggle_button(crate::icons::SVG_PIN, "Pin / Lock Role", 14);
    pin_btn.add_css_class("pin-btn");
    container.append(&pin_btn);
    let role_badge = gtk::Label::new(Some("NONE"));
    // Fixed pixel width so it never reflows between SOURCE/DEST/NONE -
    // width_chars is font-metric dependent and still shifted.
    role_badge.set_size_request(60, -1);
    role_badge.set_xalign(0.5); // center text within that fixed width
    container.append(&role_badge);
    StatBarWidgets { container, info, gauge, star_btn, pin_btn, role_badge }
}

/// The file area: the details list and the icon view in a stack, with the
/// models behind the details list.
struct FileViewWidgets {
    store: gio::ListStore,
    filter_bar: Rc<crate::filter::FilterBar>,
    filter_model: gtk::FilterListModel,
    selection: gtk::MultiSelection,
    column_view: gtk::ColumnView,
    col_map: Rc<Vec<(gtk::ColumnViewColumn, opusdm_ipc::FormatColumn)>>,
    separate: Rc<Cell<opusdm_ipc::Separate>>,
    model_sorter: gtk::CustomSorter,
    icon_field: opusdm_iconview::IconField,
    stack: gtk::Stack,
}

/// Build the model chain (store, filter, sorter, selection), the column view
/// with its six columns, and the icon view, and put both views in a stack.
/// `ui_getter` gives the columns their `Ui` once it exists.
fn build_file_view(settings: &Settings, ui_getter: impl Fn() -> Option<Rc<Ui>> + Clone + 'static) -> FileViewWidgets {
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

    let col_map: Rc<Vec<(gtk::ColumnViewColumn, opusdm_ipc::FormatColumn)>> = Rc::new(vec![
        (
            crate::columns::add_column(&column_view, "Name", crate::columns::ColKind::Name, ui_getter.clone()),
            opusdm_ipc::FormatColumn::Name,
        ),
        (
            crate::columns::add_column(&column_view, "Size", crate::columns::ColKind::Size, ui_getter.clone()),
            opusdm_ipc::FormatColumn::Size,
        ),
        (
            crate::columns::add_column(&column_view, "Date", crate::columns::ColKind::Modified, ui_getter.clone()),
            opusdm_ipc::FormatColumn::Date,
        ),
        (
            crate::columns::add_column(&column_view, "User", crate::columns::ColKind::User, ui_getter.clone()),
            opusdm_ipc::FormatColumn::User,
        ),
        (
            crate::columns::add_column(&column_view, "Group", crate::columns::ColKind::Group, ui_getter.clone()),
            opusdm_ipc::FormatColumn::Group,
        ),
        (
            crate::columns::add_column(&column_view, "Access", crate::columns::ColKind::Access, ui_getter),
            opusdm_ipc::FormatColumn::Access,
        ),
    ]);
    let name_col = col_map[0].0.clone();

    // The ColumnView's sorter tracks the active header (column + direction) and
    // draws the arrow indicator; the model sorter below only reads its state.
    let cv_sorter = column_view.sorter().and_downcast::<gtk::ColumnViewSorter>();

    let separate = Rc::new(Cell::new(opusdm_ipc::Separate::DirsFirst));
    let model_sorter = gtk::CustomSorter::new({
        let cv_sorter = cv_sorter.clone();
        let col_map = col_map.clone();
        let separate = separate.clone();
        move |a, b| {
            let ra = a.downcast_ref::<BoxedAnyObject>().unwrap().borrow::<Rc<Row>>();
            let rb = b.downcast_ref::<BoxedAnyObject>().unwrap().borrow::<Rc<Row>>();
            let (col, desc) = cv_sorter
                .as_ref()
                .and_then(|s| {
                    let active = s.primary_sort_column()?;
                    let col = col_map.iter().find(|(c, _)| *c == active).map(|(_, fc)| SortCol::from(*fc))?;
                    Some((col, s.primary_sort_order() == gtk::SortType::Descending))
                })
                .unwrap_or((SortCol::Name, false));
            dir_first_cmp(&ra, &rb, col, desc, separate.get()).into()
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

    let scroller = gtk::ScrolledWindow::builder().child(&column_view).vexpand(true).build();

    let icon_field = opusdm_iconview::IconField::new();
    icon_field.set_mode(opusdm_iconview::LayoutMode::Grid);
    icon_field.set_icon_size(opusdm_iconview::DEFAULT_ICON_SIZE);
    icon_field.set_icon_set(settings.theme.icon_set.clone());

    let stack = gtk::Stack::new();
    stack.add_named(&scroller, Some("details"));
    stack.add_named(icon_field.widget(), Some("icons"));
    stack.set_visible_child_name("details");
    stack.set_vexpand(true);
    stack.set_margin_start(4);
    stack.set_margin_end(4);

    FileViewWidgets {
        store,
        filter_bar,
        filter_model,
        selection,
        column_view,
        col_map,
        separate,
        model_sorter,
        icon_field,
        stack,
    }
}

/// Wire the filter entry: typing narrows the list and shows the match count,
/// Enter jumps to the first match, Escape dismisses the bar.
fn wire_filter_bar(view: &FileViewWidgets) {
    let filter_bar = &view.filter_bar;
    {
        let count_label = filter_bar.count_label.clone();
        let filter_model_for_count = view.filter_model.clone();
        let store_for_count = view.store.clone();
        let q = filter_bar.query.clone();
        let filt = filter_bar.filter.clone();
        filter_bar.entry.connect_search_changed(move |e| {
            // Pre-lowercase once per keystroke; the filter closure compares
            // it against each row's precomputed `name_lower` per row.
            *q.borrow_mut() = e.text().trim().to_lowercase();
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
        let selection_clone = view.selection.clone();
        let column_view_clone = view.column_view.clone();
        filter_bar.entry.connect_activate(move |_| {
            if selection_clone.n_items() > 0 {
                selection_clone.select_item(0, false);
                column_view_clone.grab_focus();
            }
        });
    }
    {
        let filter_bar_clone = filter_bar.clone();
        let column_view_clone = view.column_view.clone();
        filter_bar.entry.connect_stop_search(move |_| {
            filter_bar_clone.dismiss();
            column_view_clone.grab_focus();
        });
    }
}

/// Put the file area above the Quake-style dropdown terminal drawer, resized
/// via the standard GtkPaned handle (not a hand-rolled drag gesture).
fn build_terminal_pane(file_area: &gtk::Stack) -> (gtk::Paned, Rc<crate::terminal::TerminalDrawer>) {
    let terminal = crate::terminal::TerminalDrawer::new();
    let view_paned = gtk::Paned::new(gtk::Orientation::Vertical);
    view_paned.set_vexpand(true);
    view_paned.set_start_child(Some(file_area));
    view_paned.set_resize_start_child(true);
    view_paned.set_shrink_start_child(true);
    view_paned.set_end_child(Some(&terminal.container));
    view_paned.set_resize_end_child(false);
    view_paned.set_shrink_end_child(false);

    // The drawer starts hidden; give the paned a sane split the first time
    // it becomes visible instead of GTK's default 50/50.
    {
        let view_paned = view_paned.clone();
        terminal.container.connect_visible_notify(move |cont| {
            if cont.is_visible() {
                let total = view_paned.height();
                if total > 0 {
                    view_paned.set_position(total - crate::terminal::DEFAULT_DRAWER_HEIGHT);
                }
            }
        });
    }
    (view_paned, terminal)
}

/// The transient message line: spinner, busy label and job line.
struct MessageRowWidgets {
    container: gtk::Box,
    job_line: gtk::Label,
    busy_spinner: gtk::Spinner,
    busy_label: gtk::Label,
}

/// Build the transient message line.
fn build_message_row() -> MessageRowWidgets {
    let job_line = gtk::Label::new(None);
    job_line.set_xalign(0.0);
    job_line.set_vexpand(false);
    let busy_spinner = gtk::Spinner::new();
    busy_spinner.set_visible(false);
    let busy_label = gtk::Label::new(None);
    busy_label.set_visible(false);
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    container.set_vexpand(false);
    container.append(&busy_spinner);
    container.append(&busy_label);
    container.append(&job_line);
    MessageRowWidgets { container, job_line, busy_spinner, busy_label }
}

/// The bottom navigation bar: history, home, parent and the path entry.
#[derive(Clone)]
struct NavBarWidgets {
    container: gtk::Box,
    home_btn: gtk::Button,
    parent_btn: gtk::Button,
    path_entry: gtk::Entry,
    back_btn: gtk::Button,
    forward_btn: gtk::Button,
    history_list: gtk::ListBox,
    history_popover: gtk::Popover,
}

/// Build the bottom navigation bar.
fn build_nav_bar() -> NavBarWidgets {
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    container.set_vexpand(false);
    container.set_margin_bottom(4);
    container.set_margin_start(4);
    container.set_margin_end(4);
    let home_btn = crate::icons::icon_button(crate::icons::SVG_HOME, "Home Directory", 16);
    let parent_btn = crate::icons::icon_button(crate::icons::SVG_PARENT, "Parent Directory", 16);
    let path_entry = gtk::Entry::new();
    path_entry.set_hexpand(true);
    let back_btn = crate::icons::icon_button(crate::icons::SVG_BACK, "Back (Alt+Left)", 16);
    let forward_btn = crate::icons::icon_button(crate::icons::SVG_FORWARD, "Forward (Alt+Right)", 16);
    let history_list = gtk::ListBox::new();
    history_list.set_selection_mode(gtk::SelectionMode::None);
    let history_scroll = gtk::ScrolledWindow::builder().child(&history_list).min_content_width(360).max_content_height(320).propagate_natural_height(true).build();
    let history_popover = gtk::Popover::new();
    history_popover.set_child(Some(&history_scroll));
    let history_btn = gtk::MenuButton::new();
    history_btn.set_child(Some(&crate::icons::svg_image(crate::icons::SVG_HISTORY, 16)));
    history_btn.set_tooltip_text(Some("History"));
    history_btn.set_popover(Some(&history_popover));
    history_btn.add_css_class("opus-btn");
    container.append(&back_btn);
    container.append(&forward_btn);
    container.append(&history_btn);
    container.append(&home_btn);
    container.append(&parent_btn);
    container.append(&path_entry);
    NavBarWidgets { container, home_btn, parent_btn, path_entry, back_btn, forward_btn, history_list, history_popover }
}

/// Everything the `Ui` struct is made of besides the hub connection data.
struct Parts {
    window: gtk::ApplicationWindow,
    stat: StatBarWidgets,
    button_row: gtk::Box,
    view_toggle: gtk::ToggleButton,
    view: FileViewWidgets,
    terminal: Rc<crate::terminal::TerminalDrawer>,
    message: MessageRowWidgets,
    nav: NavBarWidgets,
}

/// Start the inotify watcher: one instance per Lister, started even when
/// auto-refresh is off so toggling the setting does not need a restart. A
/// kernel refusal (watch limit) is not fatal: the Lister just loses live
/// refresh.
fn start_watcher(settings: &Settings) -> (Option<crate::watcher::DirWatcher>, Option<async_channel::Receiver<PathBuf>>) {
    let debounce = std::time::Duration::from_millis(settings.behavior.debounce_ms());
    match crate::watcher::DirWatcher::start(debounce) {
        Ok((w, rx)) => (Some(w), Some(rx)),
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-lister", "auto-refresh unavailable: {e}");
            (None, None)
        }
    }
}

/// Assemble the `Ui` from the built widgets and the hub handshake data.
#[allow(clippy::too_many_arguments)]
fn assemble_ui(
    parts: Parts,
    client: Rc<Client>,
    id: ListerId,
    roles: Roles,
    settings: Settings,
    bindings: opusdm_ipc::Bindings,
    formats: opusdm_ipc::FormatStore,
    functions: Vec<opusdm_ipc::function::Function>,
    watcher: Option<crate::watcher::DirWatcher>,
) -> Rc<Ui> {
    // Registered once; update_css_for_colors() reloads its content in place.
    let css_provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no default GDK display: cannot run without one"),
        &css_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let user_menu_btn = gtk::MenuButton::new();
    user_menu_btn.set_label("Menu");
    user_menu_btn.set_tooltip_text(Some("User menu"));
    user_menu_btn.add_css_class("opus-btn");

    let Parts { window, stat, button_row, view_toggle, view, terminal, message, nav } = parts;
    let cache_capacity = settings.behavior.cache_capacity();
    Rc::new(Ui {
        window,
        hidden_by_hub: Cell::new(false),
        hub: HubConnection {
            client,
            id,
            roles: RefCell::new(roles),
            watcher,
            settings: RefCell::new(settings),
            hotkeys: RefCell::new(crate::hotkeys::HotkeyMap::from_hotkeys(&bindings.hotkeys)),
            bindings: RefCell::new(bindings),
            formats: RefCell::new(formats),
            functions: RefCell::new(functions),
        },
        cwd: RefCell::new(PathBuf::from("/")),
        view: ViewState {
            mode: Cell::new(ViewMode::Details),
            mode_toggle: view_toggle,
            stack: view.stack,
            icon_field: view.icon_field,
            icon_size: Cell::new(opusdm_iconview::DEFAULT_ICON_SIZE),
            column_view: view.column_view,
            selection: view.selection,
            store: view.store,
            cursor: Cell::new(0),
            filter_bar: view.filter_bar,
            show_hidden: Cell::new(false),
            size_format: Cell::new(crate::model::SizeFormat::default()),
            col_map: view.col_map,
            separate: view.separate,
            model_sorter: view.model_sorter,
        },
        status_bar: StatusBar {
            label: stat.info,
            container: stat.container,
            role_badge: stat.role_badge,
            role_lock_btn: stat.pin_btn,
            role_guard: Cell::new(false),
            job_line: message.job_line,
            totals: Cell::new((0, 0, 0)),
            gauge: stat.gauge,
            gauge_generation: Rc::new(Cell::new(0)),
            busy_spinner: message.busy_spinner,
            busy_label: message.busy_label,
        },
        path_entry: nav.path_entry,
        button_row,
        preview_win: RefCell::new(None),
        terminal,
        css_provider,
        nav_generation: Cell::new(0),
        typeahead: Typeahead { buffer: RefCell::new(String::new()), timer: RefCell::new(None) },
        hidden_btn: RefCell::new(None),
        format_state: FormatState { current: RefCell::new(opusdm_ipc::ListerFormat::builtin()), temp: RefCell::new(None) },
        caches: Caches {
            dirs: RefCell::new(crate::dircache::DirCache::new(cache_capacity)),
            sizes: RefCell::new(crate::sizecache::SizeCache::default()),
        },
        output_windows: RefCell::new(std::collections::HashMap::new()),
        busy: crate::busy::Busy::default(),
        remote: crate::remote_ui::RemoteState::default(),
        history: RefCell::new(crate::history::History::new()),
        nav_history: crate::window::NavHistory {
            back: nav.back_btn,
            forward: nav.forward_btn,
            list: nav.history_list,
        },
        last_selection: RefCell::new(Vec::new()),
        user_menu_btn,
    })
}

/// Star button (open a new Lister at this path) and the role badge (click to
/// cycle the role; see `Roles::next_role`).
fn wire_stat_bar(ui: &Rc<Ui>, star_btn: &gtk::Button) {
    {
        let ui = ui.clone();
        star_btn.connect_clicked(move |_| {
            let _ = ui.hub.client.send(&ToHub::SpawnLister { start_path: Some(ui.cwd_path()) });
        });
    }
    {
        let ui_for_click = ui.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            let next = ui_for_click.hub.roles.borrow().next_role(ui_for_click.hub.id);
            let _ = ui_for_click.hub.client.send(&ToHub::SetRole(next));
        });
        ui.status_bar.role_badge.add_controller(gesture);
    }
}

/// True when the tail of the entry is selected and equals `ghost`, that is,
/// the look-ahead suggestion is still on screen and untouched.
fn ghost_is_active(entry: &gtk::Entry, ghost: &str) -> bool {
    if ghost.is_empty() { return false; }
    let Some((start, end)) = entry.selection_bounds() else { return false };
    end == entry.text().chars().count() as i32 && entry.chars(start, end) == ghost
}

/// Look-ahead for the path bar. While the user types, the folder name that
/// completes the last component (see `pathcomplete::suggest`) is appended as
/// selected text. TAB accepts it (cursor to the end, focus stays in the bar),
/// typing on replaces it, Backspace removes it and Enter ignores it.
fn wire_path_completion(entry: &gtk::Entry) {
    let ghost = Rc::new(RefCell::new(String::new()));
    let typing = Rc::new(Cell::new(false));
    let applying = Rc::new(Cell::new(false));
    let key_ctrl = gtk::EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (entry, ghost, typing) = (entry.clone(), ghost.clone(), typing.clone());
        key_ctrl.connect_key_pressed(move |_, keyval, _, state| {
            let plain = !state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK);
            if keyval == gdk::Key::Tab && plain && ghost_is_active(&entry, &ghost.borrow()) {
                ghost.borrow_mut().clear();
                entry.set_position(-1);
                return glib::Propagation::Stop;
            }
            typing.set(plain && keyval.to_unicode().is_some_and(|c| !c.is_control()));
            glib::Propagation::Proceed
        });
    }
    {
        let typing = typing.clone();
        key_ctrl.connect_key_released(move |_, _, _, _| typing.set(false));
    }
    entry.add_controller(key_ctrl);
    {
        let ghost = ghost.clone();
        entry.connect_changed(move |entry| {
            if applying.get() || !typing.get() { return; }
            let typed = entry.text().to_string();
            // Changing the text from inside `changed` would re-enter the signal.
            let (entry, ghost, applying) = (entry.clone(), ghost.clone(), applying.clone());
            glib::idle_add_local_once(move || {
                let end = typed.chars().count() as i32;
                if entry.text() != typed || entry.position() != end { return; }
                let Some(suffix) = crate::pathcomplete::suggest(&typed) else { return };
                let mut pos = end;
                applying.set(true);
                entry.insert_text(&suffix, &mut pos);
                applying.set(false);
                entry.select_region(end, -1);
                *ghost.borrow_mut() = suffix;
            });
        });
    }
    // Registered before the navigation handler, so the suggestion is gone
    // from the text by the time Enter reads it.
    let entry_for_enter = entry.clone();
    entry.connect_activate(move |_| {
        if ghost_is_active(&entry_for_enter, &ghost.borrow()) {
            entry_for_enter.delete_selection();
        }
        ghost.borrow_mut().clear();
    });
}

/// Parent and home buttons, and the path entry (bookmarks, remote URLs,
/// archives, plain folders).
fn wire_navigation(ui: &Rc<Ui>, nav: &NavBarWidgets) {
    wire_path_completion(&nav.path_entry);
    {
        let ui = ui.clone();
        nav.parent_btn.connect_clicked(move |_| {
            let parent = crate::navpath::parent(&ui.cwd.borrow());
            if let Some(p) = parent {
                ui.navigate_to(&p);
            }
        });
    }
    {
        let ui = ui.clone();
        nav.home_btn.connect_clicked(move |_| {
            if let Ok(home) = std::env::var("HOME") {
                ui.navigate_to(Path::new(&home));
            }
        });
    }
    {
        let ui = ui.clone();
        nav.path_entry.connect_activate(move |entry| {
            let candidate = PathBuf::from(entry.text().to_string());
            // `name:` or `name:sub/dir` is a bookmark.
            if let Some(target) = crate::bookmarks_ui::resolve_text(entry.text().as_str()) {
                if target.is_dir() {
                    ui.navigate_to(&target);
                } else {
                    crate::actions::flash(&ui, &format!("{} is not a folder", target.display()));
                    ui.refresh_path_entry();
                }
                return;
            }
            if opusdm_ftp::is_remote(entry.text().as_str()) {
                match opusdm_ftp::RemoteUrl::parse(entry.text().as_str()) {
                    Ok(url) => ui.navigate_to(&crate::remote::path_of(&url)),
                    Err(e) => {
                        crate::actions::flash(&ui, &e);
                        ui.refresh_path_entry();
                    }
                }
                return;
            }
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
                glib::timeout_add_local_once(std::time::Duration::from_millis(PATH_ENTRY_ERROR_FLASH_MS), move || {
                    entry.remove_css_class("error");
                });
                ui.refresh_path_entry();
            }
        });
    }
}

/// Row activation (enter a directory, open an archive or a file) and the
/// selection-changed handler (status line, active preview).
fn wire_selection(ui: &Rc<Ui>) {
    {
        let ui = ui.clone();
        ui.view.column_view.clone().connect_activate(move |_, position| ui.activate_row(position));
    }
    {
        let ui = ui.clone();
        ui.view.selection.clone().connect_selection_changed(move |_, _, _| {
            ui.sync_cursor_from_selection();
            ui.update_status();
            ui.update_preview_if_open();
        });
    }
}

/// Window-level signals: focus notifies the hub, closing sends `Goodbye` so
/// the process can exit.
fn wire_window_signals(ui: &Rc<Ui>) {
    {
        let ui = ui.clone();
        let focus = gtk::EventControllerFocus::new();
        let window = ui.window.clone();
        focus.connect_enter(move |_| {
            let _ = ui.hub.client.send(&ToHub::FocusGained);
        });
        window.add_controller(focus);
    }
    {
        let ui = ui.clone();
        ui.window.clone().connect_close_request(move |_| {
            crate::dlog!("close request, sending Goodbye to hub");
            let _ = ui.hub.client.send(&ToHub::Goodbye);
            glib::Propagation::Proceed
        });
    }
}

/// Keyboard shortcuts. The binding table itself lives in `keys.rs`; this
/// only turns the resolved action into a call.
fn wire_keys(ui: &Rc<Ui>) {
    let ui_for_keys = ui.clone();
    let key_ctrl = gtk::EventControllerKey::new();
    key_ctrl.connect_key_pressed(move |_, keyval, _, state| {
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        // Single-letter bindings must not steal characters from a text
        // field, so where the focus sits decides how a key is read.
        let focus = if crate::window::is_editing(&ui_for_keys.window) {
            crate::keys::Focus::Editing
        } else {
            crate::keys::Focus::List
        };
        // An open type-to-find buffer takes every key first.
        let modified = ctrl || state.contains(gdk::ModifierType::ALT_MASK);
        if focus == crate::keys::Focus::List && ui_for_keys.typeahead_feed(keyval, modified) {
            return glib::Propagation::Stop;
        }
        // Alt+Left / Alt+Right walk the history (resolve() has no Alt).
        if focus == crate::keys::Focus::List && state.contains(gdk::ModifierType::ALT_MASK) {
            match keyval {
                gdk::Key::Left => {
                    ui_for_keys.history_back();
                    return glib::Propagation::Stop;
                }
                gdk::Key::Right => {
                    ui_for_keys.history_forward();
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }
        // A user hotkey (F-03) is looked up next, so it wins over a
        // built-in binding.
        let user_function = ui_for_keys.hub.hotkeys.borrow().lookup(keyval, state, focus).cloned();
        if let Some(id) = user_function {
            ui_for_keys.start_run(opusdm_ipc::ToHub::RunFunctionById { id });
            return glib::Propagation::Stop;
        }
        let Some(action) = crate::keys::resolve(keyval, ctrl, shift, focus) else {
            // Nothing is bound to this key: a name-like character starts
            // type-to-find.
            let starts = focus == crate::keys::Focus::List
                && !modified
                && ui_for_keys.typeahead_begin(keyval);
            return if starts { glib::Propagation::Stop } else { glib::Propagation::Proceed };
        };
        ui_for_keys.run_key_action(action);
        glib::Propagation::Stop
    });
    ui.window.add_controller(key_ctrl);
}

/// The details / icons toggle and the icon view's callbacks, which reuse the
/// column view's handlers where they exist.
fn wire_icon_view(ui: &Rc<Ui>, view_toggle: &gtk::ToggleButton) {
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

    crate::dnd::attach_field(ui, &ui.view.icon_field);
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
            if let Err(e) = opusdm_fsops::rename_noreplace(&entry.path, &new_path) {
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
}

/// Build the Lister window: lay out the widgets, create the `Ui` and wire
/// every signal to it.
#[allow(clippy::too_many_arguments)]
fn build_window(
    app: &gtk::Application,
    client: Rc<Client>,
    id: ListerId,
    roles: Roles,
    settings: Settings,
    bindings: opusdm_ipc::Bindings,
    formats: opusdm_ipc::FormatStore,
    functions: Vec<opusdm_ipc::function::Function>,
) -> Rc<Ui> {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Lister")
        .default_width(800)
        .default_height(600)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);

    let stat = build_stat_bar();
    root.append(&stat.container);

    // Button row (filled by actions::attach).
    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    button_row.set_margin_start(4);
    button_row.set_margin_end(4);
    let view_toggle = gtk::ToggleButton::new();
    view_toggle.set_child(Some(&gtk::Label::new(Some("\u{25a6}"))));
    view_toggle.set_tooltip_text(Some("Toggle icon view"));
    view_toggle.add_css_class("flat");
    root.append(&button_row);

    // The columns need the `Ui`, which is built after them.
    let ui_slot: Rc<RefCell<Option<Rc<Ui>>>> = Rc::new(RefCell::new(None));
    let ui_getter = {
        let slot = ui_slot.clone();
        move || slot.borrow().clone()
    };
    let view = build_file_view(&settings, ui_getter);
    wire_filter_bar(&view);
    root.append(&view.filter_bar.container);
    let (view_paned, terminal) = build_terminal_pane(&view.stack);
    root.append(&view_paned);

    let message = build_message_row();
    root.append(&message.container);
    let nav = build_nav_bar();
    root.append(&nav.container);
    window.set_child(Some(&root));

    let (watcher, watch_rx) = start_watcher(&settings);
    let star_btn = stat.star_btn.clone();
    let history_popover = nav.history_popover.clone();
    let column_view = view.column_view.clone();
    let nav_for_wiring = nav.clone();
    let parts = Parts { window, stat, button_row: button_row.clone(), view_toggle: view_toggle.clone(), view, terminal: terminal.clone(), message, nav };
    let ui = assemble_ui(parts, client, id, roles, settings, bindings, formats, functions, watcher);

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
    terminal.attach(&ui);
    ui.update_css_for_colors();

    wire_stat_bar(&ui, &star_btn);
    wire_navigation(&ui, &nav_for_wiring);
    wire_selection(&ui);
    wire_window_signals(&ui);
    wire_keys(&ui);

    crate::actions::attach(&ui);
    crate::user_menu::install(&ui);
    crate::nav_cache::install_history(&ui, &history_popover);
    button_row.append(&view_toggle);
    crate::dnd::attach(&ui, &column_view);
    wire_icon_view(&ui, &view_toggle);

    ui.update_status();
    ui
}
