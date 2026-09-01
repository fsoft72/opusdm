//! The Lister window: path bar, column view, status line, navigation.

use crate::client::{Client, Inbound, Roles};
use crate::model::{
    dir_first_cmp, format_mtime, format_size, local_tz_offset_secs, read_dir_rows, Row, SortCol,
};
use gtk4 as gtk;
use gtk::gio;
use gtk::gdk;
use gtk::glib::{self, BoxedAnyObject};
use gtk::prelude::*;
use opusdm_ipc::{ListerId, ToHub, ToLister};
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
    /// Active sort: column + descending flag.
    pub sort: Rc<Cell<(SortCol, bool)>>,
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
    /// Local timezone offset for date formatting.
    pub tz: i32,
    /// Role toggle buttons (wired in Task 14).
    pub role_src_btn: gtk::ToggleButton,
    /// Role toggle button.
    pub role_dst_btn: gtk::ToggleButton,
    /// Role lock toggle.
    pub role_lock_btn: gtk::ToggleButton,
    /// Guards against feedback while syncing toggle state from the hub.
    pub role_guard: Cell<bool>,
    /// In-window transient message line (errors, job status).
    pub job_line: gtk::Label,
    /// Empty box the button row is built into by `actions::attach`.
    pub button_row: gtk::Box,
    /// SOURCE role background color (hex).
    pub source_color: RefCell<String>,
    /// DEST role background color (hex).
    pub dest_color: RefCell<String>,
}

/// Build and run the window application.
pub fn run(mut client: Client, id: ListerId, roles: Roles, start: PathBuf) -> glib::ExitCode {
    let inbox = client.take_inbox();
    let client = Rc::new(client);
    let shared: Rc<RefCell<Option<StartState>>> = Rc::new(RefCell::new(Some(StartState {
        client,
        id,
        roles,
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
        let ui = build_window(app, st.client.clone(), st.id, st.roles.clone());
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
    start: PathBuf,
    inbox: Option<async_channel::Receiver<Inbound>>,
}

fn build_window(app: &gtk::Application, client: Rc<Client>, id: ListerId, roles: Roles) -> Rc<Ui> {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Lister")
        .default_width(800)
        .default_height(600)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_margin_top(4);
    root.set_margin_start(4);
    root.set_margin_end(4);

    // Top stat bar: D/F/B counts (left) + SOURCE/DEST/lock (right).
    let stat_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let stat_info = gtk::Label::new(Some("D: [0/0] F: [0/0] B: [0/0]"));
    stat_info.set_xalign(0.0);
    stat_info.set_hexpand(true);
    stat_bar.append(&stat_info);
    let role_badge = gtk::Label::new(None);
    stat_bar.append(&role_badge);
    root.append(&stat_bar);

    // Button row (filled by actions::attach).
    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    root.append(&button_row);

    // Model + sorter + selection + column view.
    let store = gio::ListStore::new::<BoxedAnyObject>();
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
    let sort_model = gtk::SortListModel::new(Some(store.clone()), Some(sorter.clone()));
    let selection = gtk::MultiSelection::new(Some(sort_model));
    let column_view = gtk::ColumnView::new(Some(selection.clone()));
    column_view.set_vexpand(true);

    add_column(&column_view, "Name", ColKind::Name);
    add_column(&column_view, "Size", ColKind::Size);
    add_column(&column_view, "Date", ColKind::Modified);
    add_column(&column_view, "User", ColKind::User);
    add_column(&column_view, "Group", ColKind::Group);
    add_column(&column_view, "Access", ColKind::Access);

    let scroller = gtk::ScrolledWindow::builder().child(&column_view).vexpand(true).build();
    root.append(&scroller);

    // Transient message line.
    let job_line = gtk::Label::new(None);
    job_line.set_xalign(0.0);
    job_line.set_vexpand(false);
    root.append(&job_line);

    // Bottom navigation: home, parent, path.
    let nav_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    nav_bar.set_vexpand(false);
    nav_bar.set_margin_bottom(4);
    let home_btn = gtk::Button::with_label("⌂");
    let parent_btn = gtk::Button::with_label("↑");
    let path_entry = gtk::Entry::new();
    path_entry.set_hexpand(true);
    nav_bar.append(&home_btn);
    nav_bar.append(&parent_btn);
    nav_bar.append(&path_entry);
    root.append(&nav_bar);

    window.set_child(Some(&root));

    // Add CSS for role colors.
    let css_provider = gtk::CssProvider::new();
    css_provider.load_from_data(
        ".stat-source { background-color: #90EE90; } .stat-dest { background-color: #FFB6C1; }"
    );
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
        selection: selection.clone(),
        sort: sort_cell.clone(),
        sorter: sorter.clone(),
        status: stat_info,
        stat_bar: stat_bar.clone(),
        role_badge,
        roles: RefCell::new(roles),
        path_entry: path_entry.clone(),
        tz: local_tz_offset_secs(),
        role_src_btn: gtk::ToggleButton::with_label("Source"),
        role_dst_btn: gtk::ToggleButton::with_label("Dest"),
        role_lock_btn: gtk::ToggleButton::with_label("Lock"),
        role_guard: Cell::new(false),
        job_line,
        button_row: button_row.clone(),
        source_color: RefCell::new("#90EE90".to_string()),
        dest_color: RefCell::new("#FFB6C1".to_string()),
    });

    // Navigation.
    {
        let ui = ui.clone();
        parent_btn.connect_clicked(move |_| {
            let parent = ui.cwd.borrow().parent().map(PathBuf::from);
            if let Some(p) = parent {
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
            if candidate.is_dir() {
                ui.navigate_to(&candidate);
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

    // Row activation: enter directory or open file.
    {
        let ui = ui.clone();
        column_view.connect_activate(move |_, position| {
            if let Some(row) = ui.row_at(position) {
                let target = ui.cwd.borrow().join(&row.name);
                if row.is_dir {
                    ui.navigate_to(&target);
                } else {
                    let uri = format!("file://{}", target.to_string_lossy());
                    let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
                }
            }
        });
    }

    // Selection changes update the status line.
    {
        let ui = ui.clone();
        selection.connect_selection_changed(move |_, _, _| ui.update_status());
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

    crate::actions::attach(&ui);
    crate::dnd::attach(&ui, &column_view);
    ui.update_status();
    ui
}

#[derive(Clone, Copy)]
enum ColKind {
    Name,
    Size,
    Modified,
    User,
    Group,
    Access,
}

fn add_column(view: &gtk::ColumnView, title: &str, kind: ColKind) {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, list_item| {
        let item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        if matches!(kind, ColKind::Access) {
            label.add_css_class("monospace");
        }
        item.set_child(Some(&label));
    });
    let tz = local_tz_offset_secs();
    factory.connect_bind(move |_, list_item| {
        let item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = item
            .item()
            .and_downcast::<BoxedAnyObject>()
            .map(|o| o.borrow::<Rc<Row>>().clone());
        let Some(row) = row else { return };
        let label = item.child().and_downcast::<gtk::Label>().unwrap();
        match kind {
            ColKind::Name => {
                let mark = if row.broken_link {
                    " (broken link)"
                } else if row.is_symlink {
                    " ->"
                } else {
                    ""
                };
                label.set_xalign(0.0);
                label.set_text(&format!("{}{}", row.name, mark));
            }
            ColKind::Size => {
                label.set_xalign(1.0);
                label.set_text(&format_size(row.is_dir, row.size));
            }
            ColKind::Modified => {
                label.set_xalign(0.0);
                label.set_text(&format_mtime(row.modified_unix, tz));
            }
            ColKind::User => {
                label.set_xalign(0.0);
                label.set_text(&crate::model::format_user(row.uid));
            }
            ColKind::Group => {
                label.set_xalign(0.0);
                label.set_text(&crate::model::format_group(row.gid));
            }
            ColKind::Access => {
                label.set_xalign(0.0);
                label.set_text(&crate::model::format_access(row.mode));
            }
        }
    });
    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_expand(matches!(kind, ColKind::Name));
    column.set_resizable(true);
    view.append_column(&column);
}

impl Ui {
    /// Read `dir`, repopulate the store, update title/entry/status, notify the hub.
    pub fn navigate_to(self: &Rc<Self>, dir: &Path) {
        let rows = match read_dir_rows(dir) {
            Ok(r) => r,
            Err(e) => {
                self.job_line.set_text(&format!("cannot open {}: {e}", dir.display()));
                return;
            }
        };
        *self.cwd.borrow_mut() = dir.to_path_buf();
        self.store.remove_all();
        for row in rows {
            self.store.append(&BoxedAnyObject::new(Rc::new(row)));
        }
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

    fn selected_names(&self) -> Vec<String> {
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

    fn row_at(&self, position: u32) -> Option<Rc<Row>> {
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
        let total = self.selection.n_items();
        let mut total_dirs = 0u32;
        let mut total_files = 0u32;
        let mut total_bytes = 0u64;
        let mut sel_dirs = 0u32;
        let mut sel_files = 0u32;
        let mut sel_bytes = 0u64;

        for i in 0..total {
            if let Some(row) = self.row_at(i) {
                if row.is_dir {
                    total_dirs += 1;
                } else {
                    total_files += 1;
                    total_bytes += row.size;
                }
                if self.selection.is_selected(i) {
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
            ("", "")
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
            ToLister::RoleChanged { source, dests } => {
                *self.roles.borrow_mut() = Roles { source, dests };
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
            ToLister::Welcome { source_color, dest_color, .. } => {
                *self.source_color.borrow_mut() = source_color;
                *self.dest_color.borrow_mut() = dest_color;
                self.update_css_for_colors();
            }
            ToLister::Pong => {}
        }
    }

    fn update_css_for_colors(&self) {
        let src = self.source_color.borrow();
        let dst = self.dest_color.borrow();
        let css_data = format!(
            ".stat-source {{ background-color: {}; }} .stat-dest {{ background-color: {}; }}",
            src, dst
        );
        let css_provider = gtk::CssProvider::new();
        css_provider.load_from_data(&css_data);
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().unwrap(),
            &css_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
