//! The window: the list of connections on the left, the editor on the right,
//! a message line at the bottom. Every NetworkManager call goes through the
//! worker thread; its reports come back here.

use crate::dialogs;
use crate::editor::{empty_view, EditorView};
use crate::form::{clear, EditorCtx, MARGIN, SPACING};
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gio, glib};
use opusdm_network::agent::MANAGER_IDENTIFIER;
use opusdm_network::import;
use opusdm_network::profile::{Profile, ProfileKind};
use opusdm_network::{watch, ProfileSummary, ProfilesMsg};
use opusdm_network_config::groups::{grouped, state_text};
use opusdm_network_config::session::EditSession;
use opusdm_network_config::worker::{self, Done, Job};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::mpsc;

/// Default window size.
const WINDOW_W: i32 = 920;
const WINDOW_H: i32 = 640;
/// Width of the list side.
const LIST_W: i32 = 280;
/// The text of the editor side with nothing selected.
const NOTHING_SELECTED: &str = "Choose a connection, or add one with +";

/// What runs once the user has decided about the unsaved changes.
type AfterLeave = Box<dyn Fn(&Rc<App>)>;

/// The buttons under the list.
struct ListButtons {
    add: gtk::Button,
    delete: gtk::Button,
    import: gtk::Button,
}

/// What every callback of the window shares.
pub struct App {
    window: gtk::ApplicationWindow,
    /// Feeds the worker thread; `None` while NetworkManager is not reachable.
    jobs: RefCell<Option<mpsc::Sender<Job>>>,
    done_tx: async_channel::Sender<Done>,
    list: RefCell<Vec<ProfileSummary>>,
    list_box: gtk::ListBox,
    /// The UUID shown by each row of the list; `None` for a section heading.
    row_uuids: RefCell<Vec<Option<String>>>,
    search: gtk::SearchEntry,
    editor_slot: gtk::Box,
    message: gtk::Label,
    view: RefCell<Option<EditorView>>,
    /// The profile in the editor, saved or new.
    selected: RefCell<Option<String>>,
    /// Set while the code changes the list selection.
    quiet: Cell<bool>,
    /// Close the window once the save in progress ends well.
    close_after_save: Cell<bool>,
    /// The user chose to close without saving.
    closing: Cell<bool>,
}

/// Open the window of `app`.
pub fn open(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder().application(app).title("Network Connections").default_width(WINDOW_W).default_height(WINDOW_H).build();
    let (done_tx, done_rx) = async_channel::unbounded();
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search"));
    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::Single);
    let editor_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    editor_slot.append(&empty_view(NOTHING_SELECTED));
    let message = gtk::Label::new(Some(" "));
    message.set_xalign(0.0);
    message.set_wrap(true);
    message.set_margin_start(MARGIN);
    message.set_margin_end(MARGIN);
    message.set_margin_bottom(SPACING);
    let app_state = Rc::new(App {
        window: window.clone(),
        jobs: RefCell::new(None),
        done_tx,
        list: RefCell::new(Vec::new()),
        list_box: list_box.clone(),
        row_uuids: RefCell::new(Vec::new()),
        search: search.clone(),
        editor_slot: editor_slot.clone(),
        message: message.clone(),
        view: RefCell::new(None),
        selected: RefCell::new(None),
        quiet: Cell::new(false),
        close_after_save: Cell::new(false),
        closing: Cell::new(false),
    });
    let (layout, list_buttons) = app_state.layout();
    window.set_child(Some(&layout));
    app_state.wire(&list_buttons);
    {
        let a = app_state.clone();
        glib::spawn_future_local(async move {
            while let Ok(done) = done_rx.recv().await {
                a.on_done(done);
            }
        });
    }
    let watch_rx = watch::spawn_profiles(MANAGER_IDENTIFIER);
    {
        let a = app_state.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = watch_rx.recv().await {
                a.on_watch(msg);
            }
        });
    }
    window.present();
}

impl App {
    /// The list side and the editor side, with the message line below, and
    /// the buttons under the list.
    fn layout(&self) -> (gtk::Box, ListButtons) {
        let left = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        left.set_margin_top(MARGIN);
        left.set_margin_bottom(MARGIN);
        left.set_margin_start(MARGIN);
        left.set_size_request(LIST_W, -1);
        left.append(&self.search);
        let scroll = gtk::ScrolledWindow::builder().child(&self.list_box).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        scroll.add_css_class("frame");
        left.append(&scroll);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, SPACING / 2);
        let button = |name: &str, tip: &str| {
            let b = gtk::Button::with_label(name);
            b.set_tooltip_text(Some(tip));
            buttons.append(&b);
            b
        };
        let list_buttons = ListButtons {
            add: button("+", "Add a connection"),
            delete: button("-", "Delete the connection"),
            import: button("Import...", "Import a WireGuard .conf or OpenVPN .ovpn file"),
        };
        left.append(&buttons);
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_start_child(Some(&left));
        paned.set_end_child(Some(&self.editor_slot));
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_vexpand(true);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&paned);
        root.append(&self.message);
        (root, list_buttons)
    }

    /// Connect the list, the search, the list buttons and the window close.
    fn wire(self: &Rc<Self>, buttons: &ListButtons) {
        let weak = Rc::downgrade(self);
        self.list_box.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                let (Some(a), Some(row)) = (weak.upgrade(), row) else { return };
                if a.quiet.get() { return; }
                let uuid = a.row_uuids.borrow().get(row.index() as usize).cloned().flatten();
                if let Some(uuid) = uuid { a.select(uuid); }
            }
        });
        self.search.connect_search_changed({
            let weak = weak.clone();
            move |_| if let Some(a) = weak.upgrade() { a.rebuild_list() }
        });
        connect_button(&buttons.add, &weak, App::add_new);
        connect_button(&buttons.delete, &weak, App::delete_selected);
        connect_button(&buttons.import, &weak, App::import);
        self.window.connect_close_request(move |_| {
            let Some(a) = weak.upgrade() else { return glib::Propagation::Proceed };
            if a.closing.get() || !a.is_dirty() { return glib::Propagation::Proceed; }
            a.ask_before_leaving(Box::new(|a: &Rc<App>| {
                a.closing.set(true);
                a.window.close();
            }));
            glib::Propagation::Stop
        });
    }

    /// Hand a job to the worker thread.
    fn send(&self, job: Job) {
        let sent = self.jobs.borrow().as_ref().is_some_and(|tx| tx.send(job).is_ok());
        if !sent { self.show_error("NetworkManager is not available"); }
    }

    /// Show an information line. The line always keeps its height, so the
    /// buttons above it never move.
    fn show_info(&self, text: &str) {
        self.message.remove_css_class("error");
        self.message.set_text(if text.is_empty() { " " } else { text });
    }

    /// Show an error line; it stays until the next message.
    fn show_error(&self, text: &str) {
        self.message.add_css_class("error");
        self.message.set_text(text);
    }

    /// One message of the profiles watcher.
    fn on_watch(self: &Rc<Self>, msg: ProfilesMsg) {
        match msg {
            ProfilesMsg::Backend(backend) => {
                *self.jobs.borrow_mut() = Some(worker::spawn(backend, self.done_tx.clone()));
                self.show_info("");
            }
            ProfilesMsg::Changed => self.send(Job::List),
            ProfilesMsg::AgentFailed(why) => {
                opusdm_ipc::err_log!("opusdm-network-manager", "secret agent: {why}");
                self.show_error(&format!("The keyring cannot answer NetworkManager ({why}): connections that need a password may fail"));
            }
            ProfilesMsg::Unavailable(why) => {
                *self.jobs.borrow_mut() = None;
                self.show_error(&format!("NetworkManager is not available: {why}"));
            }
        }
    }

    /// One report of the worker thread.
    fn on_done(self: &Rc<Self>, done: Done) {
        match done {
            Done::Listed(list) => self.on_listed(list),
            Done::Loaded(loaded) => {
                if self.selected.borrow().as_deref() != Some(loaded.profile.uuid.as_str()) { return; }
                self.open_editor(EditSession::loaded(*loaded));
            }
            Done::Saved(uuid) => {
                self.show_info("Saved");
                if self.close_after_save.get() {
                    self.closing.set(true);
                    self.window.close();
                    return;
                }
                *self.selected.borrow_mut() = Some(uuid.clone());
                self.send(Job::List);
                self.send(Job::Load(uuid));
            }
            Done::Deleted(uuid) => {
                self.show_info("Deleted");
                if self.selected.borrow().as_deref() == Some(uuid.as_str()) { self.close_editor(); }
                self.send(Job::List);
            }
            Done::Switched(_) => {
                self.show_info("");
                self.send(Job::List);
            }
            Done::Failed(what, e) => {
                self.close_after_save.set(false);
                self.show_error(&format!("{what}: {}", e.text()));
            }
        }
    }

    /// A new list of profiles: redraw it when it changed, and follow the
    /// state of the profile in the editor.
    fn on_listed(self: &Rc<Self>, list: Vec<ProfileSummary>) {
        if *self.list.borrow() == list {
            self.update_buttons();
            return;
        }
        *self.list.borrow_mut() = list;
        // A saved profile removed by another program leaves the editor.
        let gone = {
            let selected = self.selected.borrow();
            let view = self.view.borrow();
            let is_new = view.as_ref().is_some_and(|v| v.ctx.session.borrow().is_new());
            selected.as_ref().is_some_and(|u| !is_new && !self.list.borrow().iter().any(|p| &p.uuid == u))
        };
        if gone { self.close_editor(); }
        self.rebuild_list();
        self.update_buttons();
    }

    /// Fill the list from the profiles and the search text, keeping the selection.
    fn rebuild_list(&self) {
        self.quiet.set(true);
        clear_list(&self.list_box);
        let mut uuids = Vec::new();
        let list = self.list.borrow();
        let selected = self.selected.borrow().clone();
        for (section, rows) in grouped(&list, &self.search.text()) {
            let heading = gtk::Label::new(Some(section.label()));
            heading.set_xalign(0.0);
            heading.add_css_class("heading");
            heading.set_margin_top(SPACING);
            heading.set_margin_start(SPACING / 2);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&heading));
            row.set_selectable(false);
            row.set_activatable(false);
            self.list_box.append(&row);
            uuids.push(None);
            for p in rows {
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&profile_row(p)));
                self.list_box.append(&row);
                if selected.as_deref() == Some(p.uuid.as_str()) { self.list_box.select_row(Some(&row)); }
                uuids.push(Some(p.uuid.clone()));
            }
        }
        *self.row_uuids.borrow_mut() = uuids;
        self.quiet.set(false);
    }

    /// Select the row of the profile in the editor (or none), without loading it.
    fn reselect(&self) {
        self.quiet.set(true);
        let selected = self.selected.borrow().clone();
        let index = self.row_uuids.borrow().iter().position(|u| u.is_some() && *u == selected);
        match index.and_then(|i| self.list_box.row_at_index(i as i32)) {
            Some(row) => self.list_box.select_row(Some(&row)),
            None => self.list_box.unselect_all(),
        }
        self.quiet.set(false);
    }

    /// The user chose the profile `uuid` in the list.
    fn select(self: &Rc<Self>, uuid: String) {
        if self.selected.borrow().as_deref() == Some(uuid.as_str()) { return; }
        if !self.is_dirty() {
            return self.load(uuid);
        }
        // Until the user answers, the list shows the profile still edited.
        self.reselect();
        self.ask_before_leaving(Box::new(move |a: &Rc<App>| a.load(uuid.clone())));
    }

    /// Start loading the profile `uuid` into the editor.
    fn load(self: &Rc<Self>, uuid: String) {
        *self.selected.borrow_mut() = Some(uuid.clone());
        self.reselect();
        self.set_editor(empty_view("Loading..."));
        *self.view.borrow_mut() = None;
        self.send(Job::Load(uuid));
    }

    /// Whether the editor holds changes not saved.
    fn is_dirty(&self) -> bool {
        self.view.borrow().as_ref().is_some_and(|v| {
            let s = v.ctx.session.borrow();
            s.is_editable() && s.is_dirty()
        })
    }

    /// Ask what to do with the changes in the editor, then run `then` unless
    /// the user cancels. Save runs `then` at once: the save goes on in the
    /// worker and its result is reported in the message line.
    fn ask_before_leaving(self: &Rc<Self>, then: AfterLeave) {
        let name = self.view.borrow().as_ref().map(|v| v.ctx.read(|p| p.id.clone())).unwrap_or_default();
        let can_save = self.view.borrow().as_ref().is_some_and(|v| v.ctx.session.borrow().can_save());
        let mut buttons = vec![("Discard", 1), ("Cancel", 2)];
        if can_save { buttons.insert(0, ("Save", 0)); }
        let weak = Rc::downgrade(self);
        let detail = if can_save { None } else { Some("The changes have errors and cannot be saved.") };
        opusdm_ui::requester(&self.window, "Unsaved changes", &format!("{name} has changes that are not saved."), detail, &buttons, move |choice| {
            let Some(a) = weak.upgrade() else { return };
            match choice {
                0 => {
                    a.save();
                    then(&a);
                }
                1 => {
                    // The changes are dropped, so leaving asks nothing more.
                    if let Some(v) = a.view.borrow().as_ref() { v.ctx.session.borrow_mut().revert(); }
                    then(&a);
                }
                _ => {}
            }
        });
    }

    /// Put `widget` on the editor side.
    fn set_editor(&self, widget: gtk::Widget) {
        clear(&self.editor_slot);
        self.editor_slot.append(&widget);
    }

    /// Show `session` in the editor.
    fn open_editor(self: &Rc<Self>, session: EditSession) {
        let uuid = session.current.uuid.clone();
        let type_name = self.summary(&uuid).map(|p| p.type_name).unwrap_or_else(|| session.current.kind().label().to_string());
        let vpns: Vec<(String, String)> = self.list.borrow().iter().filter(|p| p.kind.is_vpn() && p.uuid != uuid).map(|p| (p.uuid.clone(), p.name.clone())).collect();
        let window: gtk::Window = self.window.clone().upcast();
        let ctx = EditorCtx::new(session, vpns, window);
        let view = EditorView::new(&ctx, &type_name);
        let weak = Rc::downgrade(self);
        ctx.set_on_change({
            let weak = weak.clone();
            move || if let Some(a) = weak.upgrade() { a.update_buttons() }
        });
        connect_button(&view.save, &weak, App::save);
        connect_button(&view.revert, &weak, App::revert);
        connect_button(&view.connect, &weak, App::toggle_connection);
        self.set_editor(view.root.clone().upcast());
        *self.selected.borrow_mut() = Some(uuid);
        *self.view.borrow_mut() = Some(view);
        self.reselect();
        // Marks the errors of a new profile; the widgets are in the window now.
        ctx.refresh();
    }

    /// Empty the editor side.
    fn close_editor(&self) {
        *self.view.borrow_mut() = None;
        *self.selected.borrow_mut() = None;
        self.set_editor(empty_view(NOTHING_SELECTED));
        self.reselect();
    }

    /// The summary of the profile `uuid` in the list.
    fn summary(&self, uuid: &str) -> Option<ProfileSummary> {
        self.list.borrow().iter().find(|p| p.uuid == uuid).cloned()
    }

    /// Update the editor buttons from the session and the list.
    fn update_buttons(&self) {
        let selected = self.selected.borrow().clone();
        let summary = selected.and_then(|u| self.summary(&u));
        if let Some(v) = self.view.borrow().as_ref() { v.update(summary.as_ref()); }
    }

    /// Save the profile in the editor.
    fn save(self: &Rc<Self>) {
        let job = {
            let view = self.view.borrow();
            let Some(v) = view.as_ref() else { return };
            let s = v.ctx.session.borrow();
            if !s.can_save() { return; }
            Job::Save { profile: Box::new(s.current.clone()), base: s.base.clone(), files: s.files.clone() }
        };
        self.show_info("Saving...");
        self.send(job);
    }

    /// Drop the changes in the editor and show the profile as saved.
    fn revert(self: &Rc<Self>) {
        let session = {
            let view = self.view.borrow();
            let Some(v) = view.as_ref() else { return };
            let mut s = v.ctx.session.borrow().clone();
            s.revert();
            s
        };
        self.open_editor(session);
    }

    /// Connect or disconnect the profile in the editor.
    fn toggle_connection(self: &Rc<Self>) {
        let Some(uuid) = self.selected.borrow().clone() else { return };
        let Some(summary) = self.summary(&uuid) else { return };
        if summary.active || summary.activating {
            self.show_info(&format!("Disconnecting {}...", summary.name));
            return self.send(Job::Deactivate(uuid));
        }
        if self.is_dirty() { return self.show_error("Save the changes before connecting"); }
        if !summary.ask_password {
            self.show_info(&format!("Connecting {}...", summary.name));
            return self.send(Job::Activate { uuid, password: None });
        }
        let weak = Rc::downgrade(self);
        let name = summary.name.clone();
        dialogs::ask_password(self.window.upcast_ref(), &summary.name, move |password| {
            let Some(a) = weak.upgrade() else { return };
            a.show_info(&format!("Connecting {name}..."));
            a.send(Job::Activate { uuid: uuid.clone(), password: Some(password) });
        });
    }

    /// Ask the kind of a new connection and open it in the editor.
    fn add_new(self: &Rc<Self>) {
        let start = |a: &Rc<App>| {
            let weak = Rc::downgrade(a);
            dialogs::choose_kind(a.window.upcast_ref(), move |kind| {
                let Some(a) = weak.upgrade() else { return };
                match Profile::new(kind) {
                    Ok(p) => a.open_editor(EditSession::created(p)),
                    Err(e) => a.show_error(&e.text()),
                }
            });
        };
        if self.is_dirty() { self.ask_before_leaving(Box::new(start)) } else { start(self) }
    }

    /// Delete the profile in the editor, after asking.
    fn delete_selected(self: &Rc<Self>) {
        let Some(uuid) = self.selected.borrow().clone() else { return };
        let (name, is_new) = match self.view.borrow().as_ref() {
            Some(v) => (v.ctx.read(|p| p.id.clone()), v.ctx.session.borrow().is_new()),
            None => (self.summary(&uuid).map(|p| p.name).unwrap_or_default(), false),
        };
        // A connection never saved is only dropped.
        if is_new { return self.close_editor(); }
        let weak = Rc::downgrade(self);
        opusdm_ui::requester(&self.window, "Delete connection", &format!("Delete {name}?"), Some("Its saved passwords and imported files are deleted too."), &[("Delete", true), ("Cancel", false)], move |yes| {
            let Some(a) = weak.upgrade() else { return };
            if yes { a.send(Job::Delete(uuid.clone())); }
        });
    }

    /// Choose a VPN file and open what it holds in the editor.
    fn import(self: &Rc<Self>) {
        let start = |a: &Rc<App>| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("VPN configurations (*.ovpn, *.conf)"));
            filter.add_pattern("*.ovpn");
            filter.add_pattern("*.conf");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder().title("Import a VPN").modal(true).filters(&filters).default_filter(&filter).build();
            let weak: Weak<App> = Rc::downgrade(a);
            dialog.open(Some(&a.window), gio::Cancellable::NONE, move |res| {
                let Some(a) = weak.upgrade() else { return };
                let file = match res {
                    Ok(file) => file,
                    Err(e) if e.matches(gtk::DialogError::Dismissed) => return,
                    Err(e) => return a.show_error(&format!("File chooser: {e}")),
                };
                let Some(path) = file.path() else { return a.show_error("Only local files can be imported") };
                match import::import_file(&path) {
                    Ok(imported) => {
                        a.show_info(&format!("Imported {}: check it and save it", path.display()));
                        a.open_editor(EditSession::imported(imported));
                    }
                    Err(e) => a.show_error(&format!("Import: {}", e.text())),
                }
            });
        };
        if self.is_dirty() { self.ask_before_leaving(Box::new(start)) } else { start(self) }
    }
}

/// Run `action` on the app when `button` is clicked.
fn connect_button(button: &gtk::Button, weak: &Weak<App>, action: fn(&Rc<App>)) {
    let weak = weak.clone();
    button.connect_clicked(move |_| if let Some(a) = weak.upgrade() { action(&a) });
}

/// Remove every row of `list`.
fn clear_list(list: &gtk::ListBox) {
    while let Some(row) = list.first_child() {
        list.remove(&row);
    }
}

/// One profile line of the list: the name and its state.
fn profile_row(p: &ProfileSummary) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
    line.set_margin_start(SPACING * 2);
    line.set_margin_end(SPACING);
    line.set_margin_top(SPACING / 2);
    line.set_margin_bottom(SPACING / 2);
    let name = gtk::Label::new(Some(&p.name));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.append(&name);
    let mut detail = state_text(p).to_string();
    if p.kind == ProfileKind::Other && detail.is_empty() { detail = p.type_name.clone(); }
    let state = gtk::Label::new(Some(&detail));
    state.add_css_class("dim-label");
    line.append(&state);
    line
}
