//! The window: the printers on the left, the General, Options and Queue tabs
//! of the selected one on the right, a message line at the bottom. Every
//! CUPS call goes through the worker thread; the watcher thread follows the
//! printers and the queue.

use crate::pages::{job_row, GeneralPage, OptionsPage, QueuePage, MARGIN, SPACING};
use crate::session::EditSession;
use crate::wizard_ui::{self, WizardUi};
use crate::worker::{self, Done, Job};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_printing::backend::PrintBackend;
use opusdm_printing::cups::CupsBackend;
use opusdm_printing::model::{JobState, Printer, PrinterOption, PrinterState};
use opusdm_printing::tray::is_owner_paused;
use opusdm_printing::watch::{self, Snapshot, WatchMsg};
use opusdm_ui::EmbeddedTool;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::{mpsc, Arc};

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-printer-manager";
/// Default window size.
const WINDOW_W: i32 = 920;
const WINDOW_H: i32 = 640;
/// Width of the list side.
const LIST_W: i32 = 260;
/// The text of the right side with nothing selected.
const NOTHING_SELECTED: &str = "Choose a printer, or add one with +";
/// Stack pages of the right side.
const PAGE_EMPTY: &str = "empty";
const PAGE_PRINTER: &str = "printer";

/// The error and warning styles of the window.
pub const APP_CSS: &str = "
label.error { color: #ff7b63; }
label.warning { color: #f6d32d; }
";

/// Told whether a save asked by the host worked.
type SaveDone = Box<dyn FnOnce(bool)>;

/// What runs once the user has decided about the unsaved changes; it gets
/// whether a save was sent.
type AfterLeave = Box<dyn Fn(&Rc<App>, bool)>;

/// An action of the Queue tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueAction {
    Cancel,
    Hold,
    Release,
    Retry,
    CancelAll,
}

/// What every callback of the window shares.
pub struct App {
    /// The window the dialogs belong to: the program's own, or the host's.
    window: gtk::Window,
    jobs: mpsc::Sender<Job>,
    /// Asks the watcher to read CUPS at once.
    poke: mpsc::Sender<()>,
    snapshot: RefCell<Snapshot>,
    list_box: gtk::ListBox,
    /// The printer of each row of the list.
    pub(crate) row_names: RefCell<Vec<String>>,
    selected: RefCell<Option<String>>,
    session: RefCell<Option<EditSession>>,
    /// The options of the selected printer; `None` while they load.
    options: RefCell<Option<Vec<PrinterOption>>>,
    /// The job of each row of the queue.
    pub(crate) queue_ids: RefCell<Vec<i32>>,
    selected_job: Cell<Option<i32>>,
    detail: gtk::Stack,
    tabs: gtk::Notebook,
    queue_label: gtk::Label,
    general: GeneralPage,
    opts: OptionsPage,
    queue: QueuePage,
    add: gtk::Button,
    remove: gtk::Button,
    message: gtk::Label,
    /// Set while the code fills widgets, so their signals change nothing.
    quiet: Cell<bool>,
    /// The user chose Save while closing: close once the save ends well.
    close_after_save: Cell<bool>,
    /// The window may close without asking again.
    closing: Cell<bool>,
    save_done: RefCell<Option<SaveDone>>,
    /// The add printer dialog, while it is open.
    wizard: RefCell<Option<Weak<WizardUi>>>,
}

/// Open the window of `app`.
pub fn open(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder().application(app).title("Printers").default_width(WINDOW_W).default_height(WINDOW_H).build();
    let (state, layout) = build_with(window.upcast_ref(), Arc::new(CupsBackend::new()));
    window.set_child(Some(&layout));
    state.guard_close();
    keep_until_closed(window.upcast_ref(), state);
    window.present();
}

/// The printers of the Printers program inside another window (Advanced tab
/// of Settings): the same list and tabs, with their own Save and Revert.
pub struct PrinterTool {
    app: Rc<App>,
    root: gtk::Box,
}

/// Open the Printers tool for the host `window`. The list appears a moment
/// after the page, once the watcher has read CUPS.
pub fn embedded(window: &gtk::Window) -> Rc<PrinterTool> {
    install_css_once();
    let (app, root) = build_with(window, Arc::new(CupsBackend::new()));
    Rc::new(PrinterTool { app, root })
}

/// Add the error and warning styles to the display, once.
fn install_css_once() {
    thread_local! { static DONE: Cell<bool> = const { Cell::new(false) }; }
    if DONE.with(|d| d.replace(true)) { return; }
    let Some(display) = gtk::gdk::Display::default() else {
        opusdm_ipc::err_log!(LOG_TAG, "no display: the Printers styles are not installed");
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(APP_CSS);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

/// Build the window content for `window` on `backend` and start the worker
/// and the watcher. The caller keeps the app alive: everything else holds it
/// weakly.
fn build_with(window: &gtk::Window, backend: Arc<dyn PrintBackend>) -> (Rc<App>, gtk::Box) {
    let (done_tx, done_rx) = async_channel::unbounded();
    let jobs = worker::spawn(backend.clone(), done_tx);
    let (watch_rx, poke) = watch::spawn(backend);
    let message = gtk::Label::new(Some(" "));
    message.set_xalign(0.0);
    message.set_wrap(true);
    message.set_margin_start(MARGIN);
    message.set_margin_end(MARGIN);
    message.set_margin_bottom(SPACING);
    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::Single);
    let app = Rc::new(App {
        window: window.clone(),
        jobs,
        poke,
        snapshot: RefCell::new(Snapshot::default()),
        list_box,
        row_names: RefCell::new(Vec::new()),
        selected: RefCell::new(None),
        session: RefCell::new(None),
        options: RefCell::new(None),
        queue_ids: RefCell::new(Vec::new()),
        selected_job: Cell::new(None),
        detail: gtk::Stack::new(),
        tabs: gtk::Notebook::new(),
        queue_label: gtk::Label::new(Some("Queue")),
        general: GeneralPage::new(),
        opts: OptionsPage::new(),
        queue: QueuePage::new(),
        add: gtk::Button::with_label("+"),
        remove: gtk::Button::with_label("-"),
        message,
        quiet: Cell::new(false),
        close_after_save: Cell::new(false),
        closing: Cell::new(false),
        save_done: RefCell::new(None),
        wizard: RefCell::new(None),
    });
    let layout = app.layout();
    app.wire();
    app.show_info("Reading the printers...");
    let weak = Rc::downgrade(&app);
    listen(&weak, done_rx, "worker reports", App::on_done);
    listen(&weak, watch_rx, "CUPS watcher", App::on_watch);
    (app, layout)
}

/// Hand every message of `rx` to `handle` on the app. The loop holds the app
/// weakly and ends, with a log line, once the window is gone.
fn listen<T: 'static>(weak: &Weak<App>, rx: async_channel::Receiver<T>, name: &'static str, handle: fn(&Rc<App>, T)) {
    let weak = weak.clone();
    glib::spawn_future_local(async move {
        while let Ok(message) = rx.recv().await {
            let Some(a) = weak.upgrade() else {
                opusdm_ipc::err_log!(LOG_TAG, "the printer window is gone: the {name} loop stops");
                return;
            };
            handle(&a, message);
        }
        opusdm_ipc::err_log!(LOG_TAG, "the {name} channel closed: the loop stops");
    });
}

/// Hold the only strong reference to `app` until `window` really closes
/// (after the unsaved-changes question). Dropping it ends the worker.
fn keep_until_closed(window: &gtk::Window, app: Rc<App>) {
    let slot = RefCell::new(Some(app));
    // Connected after `App::guard_close`: it runs only when that handler lets
    // the close go on.
    window.connect_close_request(move |_| {
        slot.borrow_mut().take();
        glib::Propagation::Proceed
    });
}

/// The app behind `weak`, or `None` with a log line naming `what` could
/// not be done because the window is gone.
fn alive(weak: &Weak<App>, what: &str) -> Option<Rc<App>> {
    let app = weak.upgrade();
    if app.is_none() { opusdm_ipc::err_log!(LOG_TAG, "the printer window is gone: {what} ignored"); }
    app
}

/// Run `action` on the app when `button` is clicked.
fn connect_button(button: &gtk::Button, weak: &Weak<App>, action: impl Fn(&Rc<App>) + 'static) {
    let weak = weak.clone();
    button.connect_clicked(move |_| match weak.upgrade() {
        Some(a) => action(&a),
        None => opusdm_ipc::err_log!(LOG_TAG, "a button was clicked after the printer window was gone"),
    });
}

impl App {
    /// The list side, the tabs and the message line.
    fn layout(&self) -> gtk::Box {
        let left = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        left.set_margin_top(MARGIN);
        left.set_margin_bottom(MARGIN);
        left.set_margin_start(MARGIN);
        left.set_size_request(LIST_W, -1);
        let scroll = gtk::ScrolledWindow::builder().child(&self.list_box).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        scroll.add_css_class("frame");
        left.append(&scroll);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, SPACING / 2);
        self.add.set_tooltip_text(Some("Add a printer"));
        self.remove.set_tooltip_text(Some("Remove the printer"));
        buttons.append(&self.add);
        buttons.append(&self.remove);
        left.append(&buttons);

        self.tabs.set_vexpand(true);
        self.tabs.set_margin_top(MARGIN);
        self.tabs.set_margin_end(MARGIN);
        self.tabs.append_page(&self.general.root, Some(&gtk::Label::new(Some("General"))));
        self.tabs.append_page(&self.opts.root, Some(&gtk::Label::new(Some("Options"))));
        self.tabs.append_page(&self.queue.root, Some(&self.queue_label));
        let empty = gtk::Label::new(Some(NOTHING_SELECTED));
        empty.add_css_class("dim-label");
        self.detail.add_named(&empty, Some(PAGE_EMPTY));
        self.detail.add_named(&self.tabs, Some(PAGE_PRINTER));
        self.detail.set_visible_child_name(PAGE_EMPTY);
        self.detail.set_hexpand(true);

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_start_child(Some(&left));
        paned.set_end_child(Some(&self.detail));
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_vexpand(true);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&paned);
        root.append(&self.message);
        root
    }

    /// Connect the list, the entries and the buttons.
    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list_box.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                let Some(a) = alive(&weak, "a printer selection") else { return };
                let Some(row) = row else { return };
                if a.quiet.get() { return; }
                let name = a.row_names.borrow().get(row.index() as usize).cloned();
                if let Some(name) = name { a.select(name); }
            }
        });
        self.queue.list.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                let Some(a) = alive(&weak, "a job selection") else { return };
                if a.quiet.get() { return; }
                let id = row.and_then(|r| a.queue_ids.borrow().get(r.index() as usize).copied());
                a.selected_job.set(id);
                a.update_buttons();
            }
        });
        for (entry, is_info) in [(&self.general.info, true), (&self.general.location, false)] {
            let weak = weak.clone();
            entry.connect_changed(move |e| {
                let Some(a) = alive(&weak, "an edit of the description or location") else { return };
                if a.quiet.get() { return; }
                if let Some(s) = a.session.borrow_mut().as_mut() {
                    if is_info { s.info = e.text().to_string() } else { s.location = e.text().to_string() }
                }
                a.update_buttons();
            });
        }
        connect_button(&self.add, &weak, |a| { a.open_wizard(); });
        connect_button(&self.remove, &weak, App::remove_selected);
        connect_button(&self.general.set_default, &weak, |a| a.printer_job(|p| Job::SetDefault(p.name.clone())));
        connect_button(&self.general.enable, &weak, |a| a.printer_job(|p| Job::SetEnabled(p.name.clone(), p.is_paused())));
        connect_button(&self.general.test_page, &weak, |a| a.printer_job(|p| Job::TestPage(p.name.clone())));
        connect_button(&self.general.save, &weak, |a| { a.save(); });
        connect_button(&self.general.revert, &weak, App::revert);
        connect_button(&self.queue.cancel, &weak, |a| a.queue_action(QueueAction::Cancel));
        connect_button(&self.queue.hold, &weak, |a| a.queue_action(QueueAction::Hold));
        connect_button(&self.queue.release, &weak, |a| a.queue_action(QueueAction::Release));
        connect_button(&self.queue.retry, &weak, |a| a.queue_action(QueueAction::Retry));
        connect_button(&self.queue.cancel_all, &weak, |a| a.queue_action(QueueAction::CancelAll));
        self.update_buttons();
    }

    /// Ask before the program's own window closes with changes not saved.
    /// Not used inside a host window: the host asks, see `PrinterTool`.
    fn guard_close(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            let Some(a) = alive(&weak, "the unsaved changes check of the close") else { return glib::Propagation::Proceed };
            if a.closing.get() || !a.is_dirty() { return glib::Propagation::Proceed; }
            if a.close_after_save.get() { return glib::Propagation::Stop; }
            a.ask_before_leaving(Box::new(|a: &Rc<App>, saved| {
                if saved {
                    a.close_after_save.set(true);
                    return;
                }
                a.closing.set(true);
                a.window.close();
            }));
            glib::Propagation::Stop
        });
    }

    /// Hand a job to the worker thread. Returns whether it was sent.
    pub(crate) fn send(&self, job: Job) -> bool {
        let sent = self.jobs.send(job).is_ok();
        if !sent {
            opusdm_ipc::err_log!(LOG_TAG, "the CUPS worker is gone: a job was not sent");
            self.show_error("The CUPS worker stopped: close and open the window again");
        }
        sent
    }

    /// Show an information line; the line keeps its height.
    fn show_info(&self, text: &str) {
        self.message.remove_css_class("error");
        self.message.set_text(if text.is_empty() { " " } else { text });
    }

    /// Show an error line; it stays until the next message.
    fn show_error(&self, text: &str) {
        self.message.add_css_class("error");
        self.message.set_text(text);
    }

    /// Tell the host window how the save it asked for ended, if it asked.
    fn finish_save(&self, saved: bool) {
        let done = self.save_done.borrow_mut().take();
        if let Some(done) = done { done(saved); }
    }

    /// One message of the watcher.
    fn on_watch(self: &Rc<Self>, msg: WatchMsg) {
        match msg {
            WatchMsg::Snapshot(s) => {
                if self.message.text() == "Reading the printers..." { self.show_info(""); }
                self.apply_snapshot(s);
            }
            WatchMsg::Unavailable(why) => self.show_error(&format!("CUPS is not available: {why}")),
        }
    }

    /// One report of the worker thread.
    fn on_done(self: &Rc<Self>, done: Done) {
        if done.is_failed_save() {
            self.close_after_save.set(false);
            self.finish_save(false);
        }
        match done {
            Done::Refreshed(s) => self.apply_snapshot(s),
            Done::Options(printer, options) => self.on_options(printer, options),
            Done::Saved(printer) => {
                self.show_info(&format!("{printer} saved"));
                // The save may be of a printer left meanwhile (Save while switching).
                if let Some(s) = self.session.borrow_mut().as_mut().filter(|s| s.printer == printer) { s.mark_saved(); }
                if self.close_after_save.replace(false) {
                    self.closing.set(true);
                    self.window.close();
                    return;
                }
                self.finish_save(true);
                self.update_buttons();
                self.send(Job::LoadOptions(printer));
            }
            Done::Acted(text) => self.show_info(&text),
            Done::Failed { what, error, .. } => {
                opusdm_ipc::err_log!(LOG_TAG, "{what}: {error}");
                if what == worker::ADDING {
                    if let Some(w) = self.wizard() { w.on_add_failed(&error.0); }
                }
                self.show_error(&format!("{what}: {error}"));
            }
            Done::Devices(devices) => match self.wizard() {
                Some(w) => w.on_devices(devices),
                None => opusdm_ipc::err_log!(LOG_TAG, "the printers found came after the add printer dialog closed"),
            },
            Done::Drivers(drivers) => match self.wizard() {
                Some(w) => w.on_drivers(drivers),
                None => opusdm_ipc::err_log!(LOG_TAG, "the drivers came after the add printer dialog closed"),
            },
            Done::Added(name) => {
                self.show_info(&format!("{name} added"));
                match self.wizard() {
                    Some(w) => w.on_added(&name),
                    None => opusdm_ipc::err_log!(LOG_TAG, "{name} was added after the add printer dialog closed"),
                }
            }
        }
        // A change may have moved the queue: read it again soon.
        if self.poke.send(()).is_err() { opusdm_ipc::err_log!(LOG_TAG, "the CUPS watcher is gone: the list may be old"); }
    }

    /// The options of `printer` arrived.
    fn on_options(self: &Rc<Self>, printer: String, options: Vec<PrinterOption>) {
        if self.selected.borrow().as_deref() != Some(printer.as_str()) { return; }
        if let Some(s) = self.session.borrow_mut().as_mut() { s.set_options(&options); }
        *self.options.borrow_mut() = Some(options);
        self.show_options();
        self.update_buttons();
    }

    /// A new view of CUPS: redraw the list and follow the selected printer.
    fn apply_snapshot(self: &Rc<Self>, s: Snapshot) {
        if *self.snapshot.borrow() == s { return; }
        *self.snapshot.borrow_mut() = s;
        let gone = self.selected.borrow().as_ref().is_some_and(|n| self.printer(n).is_none());
        if gone { self.close_detail(); }
        self.rebuild_list();
        self.refresh_detail();
    }

    /// The printer `name` of the last snapshot.
    fn printer(&self, name: &str) -> Option<Printer> {
        self.snapshot.borrow().printers.iter().find(|p| p.name == name).cloned()
    }

    /// The selected printer.
    fn selected_printer(&self) -> Option<Printer> {
        let name = self.selected.borrow().clone()?;
        self.printer(&name)
    }

    /// Fill the list from the snapshot, keeping the selection.
    fn rebuild_list(&self) {
        self.quiet.set(true);
        opusdm_ui::clear_children(&self.list_box);
        let snapshot = self.snapshot.borrow();
        let selected = self.selected.borrow().clone();
        let mut names = Vec::new();
        for p in &snapshot.printers {
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&printer_row(p, snapshot.jobs_of(&p.name).count())));
            self.list_box.append(&row);
            if selected.as_deref() == Some(p.name.as_str()) { self.list_box.select_row(Some(&row)); }
            names.push(p.name.clone());
        }
        *self.row_names.borrow_mut() = names;
        self.quiet.set(false);
    }

    /// Select the row of the selected printer (or none), without loading it.
    fn reselect(&self) {
        self.quiet.set(true);
        let selected = self.selected.borrow().clone();
        let index = self.row_names.borrow().iter().position(|n| Some(n) == selected.as_ref());
        match index.and_then(|i| self.list_box.row_at_index(i as i32)) {
            Some(row) => self.list_box.select_row(Some(&row)),
            None => self.list_box.unselect_all(),
        }
        self.quiet.set(false);
    }

    /// The user chose the printer `name`.
    pub(crate) fn select(self: &Rc<Self>, name: String) {
        if self.selected.borrow().as_deref() == Some(name.as_str()) { return; }
        if !self.is_dirty() { return self.load(name); }
        // Until the user answers, the list shows the printer still edited.
        self.reselect();
        self.ask_before_leaving(Box::new(move |a: &Rc<App>, _| a.load(name.clone())));
    }

    /// Show the printer `name` on the right side and read its options.
    fn load(self: &Rc<Self>, name: String) {
        let Some(p) = self.printer(&name) else { return self.show_error(&format!("{name} is not in the list any more")) };
        *self.selected.borrow_mut() = Some(name.clone());
        *self.session.borrow_mut() = Some(EditSession::new(&p));
        *self.options.borrow_mut() = None;
        self.selected_job.set(None);
        self.reselect();
        self.fill_entries();
        self.opts.show_text("Loading options...");
        self.detail.set_visible_child_name(PAGE_PRINTER);
        let lpoptions = dirs::home_dir().is_some_and(|h| h.join(".cups/lpoptions").exists());
        self.general.lpoptions_note.set_visible(lpoptions);
        self.refresh_detail();
        self.send(Job::LoadOptions(name));
    }

    /// Empty the right side.
    fn close_detail(&self) {
        *self.selected.borrow_mut() = None;
        *self.session.borrow_mut() = None;
        *self.options.borrow_mut() = None;
        self.selected_job.set(None);
        self.queue_ids.borrow_mut().clear();
        self.detail.set_visible_child_name(PAGE_EMPTY);
        self.reselect();
        self.update_buttons();
    }

    /// Put the description and location of the session in the entries.
    fn fill_entries(&self) {
        let (info, location) = match self.session.borrow().as_ref() {
            Some(s) => (s.info.clone(), s.location.clone()),
            None => return,
        };
        self.quiet.set(true);
        self.general.info.set_text(&info);
        self.general.location.set_text(&location);
        self.quiet.set(false);
    }

    /// Show the options of the selected printer with the session values.
    fn show_options(self: &Rc<Self>) {
        let Some(options) = self.options.borrow().clone() else { return };
        let weak = Rc::downgrade(self);
        let session = self.session.borrow().clone();
        self.opts.show_options(&options, |key| session.as_ref().and_then(|s| s.option(key).map(str::to_string)), move |key, value| {
            let Some(a) = weak.upgrade() else {
                opusdm_ipc::err_log!(LOG_TAG, "an option changed after the printer window was gone");
                return;
            };
            if let Some(s) = a.session.borrow_mut().as_mut() { s.set_option(key, value); }
            a.update_buttons();
        });
    }

    /// Follow the selected printer: its facts and its queue.
    fn refresh_detail(&self) {
        let Some(p) = self.selected_printer() else { return self.update_buttons() };
        self.general.show(&p);
        // A session with no change follows what another program saved.
        let follow = self.session.borrow().as_ref().is_some_and(|s| !s.is_dirty() && (s.info != p.info || s.location != p.location));
        if follow {
            let mut s = EditSession::new(&p);
            if let Some(o) = self.options.borrow().as_ref() { s.set_options(o); }
            *self.session.borrow_mut() = Some(s);
            self.fill_entries();
        }
        self.fill_queue(&p);
        self.update_buttons();
    }

    /// Fill the Queue tab with the jobs of `p`, keeping the selected job.
    fn fill_queue(&self, p: &Printer) {
        self.quiet.set(true);
        opusdm_ui::clear_children(&self.queue.list);
        let snapshot = self.snapshot.borrow();
        let mut ids = Vec::new();
        for j in snapshot.jobs_of(&p.name) {
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&job_row(j)));
            self.queue.list.append(&row);
            if self.selected_job.get() == Some(j.id) { self.queue.list.select_row(Some(&row)); }
            ids.push(j.id);
        }
        if !ids.contains(&self.selected_job.get().unwrap_or(-1)) { self.selected_job.set(None); }
        self.queue_label.set_text(&if ids.is_empty() { "Queue".to_string() } else { format!("Queue ({})", ids.len()) });
        let text = if ids.is_empty() { "No jobs in the queue.".to_string() } else { crate::pages::state_text(p) };
        self.queue.message.set_text(&text);
        *self.queue_ids.borrow_mut() = ids;
        self.quiet.set(false);
    }

    /// Select the job `id` in the queue, as a click does (for the tests).
    #[cfg(test)]
    pub(crate) fn select_job(&self, id: i32) {
        let Some(index) = self.queue_ids.borrow().iter().position(|j| *j == id) else { return };
        if let Some(row) = self.queue.list.row_at_index(index as i32) { self.queue.list.select_row(Some(&row)); }
        self.selected_job.set(Some(id));
        self.update_buttons();
    }

    /// The state of the selected job.
    fn selected_job_state(&self) -> Option<JobState> {
        let id = self.selected_job.get()?;
        self.snapshot.borrow().jobs.iter().find(|j| j.id == id).map(|j| j.state)
    }

    /// Update which buttons can be used.
    fn update_buttons(&self) {
        let printer = self.selected_printer();
        let dirty = self.is_dirty();
        self.remove.set_sensitive(printer.is_some());
        self.general.save.set_sensitive(dirty);
        self.general.revert.set_sensitive(dirty);
        let job = self.selected_job_state();
        let has_jobs = !self.queue_ids.borrow().is_empty();
        let stopped = printer.as_ref().is_some_and(|p| p.state == PrinterState::Stopped && !is_owner_paused(p));
        self.queue.cancel.set_sensitive(job.is_some());
        self.queue.hold.set_sensitive(job == Some(JobState::Pending));
        self.queue.release.set_sensitive(job == Some(JobState::Held));
        self.queue.retry.set_sensitive(job == Some(JobState::Stopped) || stopped);
        self.queue.cancel_all.set_sensitive(has_jobs);
    }

    /// Whether the session holds changes not saved.
    fn is_dirty(&self) -> bool {
        self.session.borrow().as_ref().is_some_and(EditSession::is_dirty)
    }

    /// Send the job `make` builds for the selected printer.
    fn printer_job(&self, make: impl Fn(&Printer) -> Job) {
        let Some(p) = self.selected_printer() else { return self.show_error("Choose a printer first") };
        self.send(make(&p));
    }

    /// Run a Queue tab action on the selected job (or printer).
    pub(crate) fn queue_action(self: &Rc<Self>, action: QueueAction) {
        let Some(p) = self.selected_printer() else { return self.show_error("Choose a printer first") };
        let job = self.selected_job.get();
        let job_action = |make: fn(i32) -> Job| match job {
            Some(id) => { self.send(make(id)); }
            None => self.show_error("Choose a job in the queue first"),
        };
        match action {
            QueueAction::Cancel => job_action(Job::Cancel),
            QueueAction::Hold => job_action(Job::Hold),
            QueueAction::Release => job_action(Job::Release),
            QueueAction::Retry => {
                let stopped_job = job.filter(|_| self.selected_job_state() == Some(JobState::Stopped));
                self.send(Job::Retry(p.name.clone(), stopped_job));
            }
            QueueAction::CancelAll => {
                let weak = Rc::downgrade(self);
                let name = p.name.clone();
                opusdm_ui::requester(&self.window, "Cancel all jobs", &format!("Cancel every job of {name}?"), None, &[("Cancel all", true), ("Keep them", false)], move |yes| {
                    let Some(a) = alive(&weak, "the Cancel all answer") else { return };
                    if yes { a.send(Job::CancelAll(name.clone())); }
                });
            }
        }
    }

    /// The open add printer dialog, if any.
    fn wizard(&self) -> Option<Rc<WizardUi>> {
        self.wizard.borrow().as_ref().and_then(Weak::upgrade)
    }

    /// Open the add printer dialog (or raise the open one).
    pub(crate) fn open_wizard(self: &Rc<Self>) -> Rc<WizardUi> {
        if let Some(w) = self.wizard() {
            w.present();
            return w;
        }
        let (taken, uris) = {
            let s = self.snapshot.borrow();
            (s.printers.iter().map(|p| p.name.clone()).collect(), s.printers.iter().map(|p| p.device_uri.clone()).collect())
        };
        let weak = Rc::downgrade(self);
        let send = Box::new(move |job: Job| match weak.upgrade() {
            Some(a) => a.send(job),
            None => {
                opusdm_ipc::err_log!(LOG_TAG, "the add printer dialog outlived the printer window");
                false
            }
        });
        let w = wizard_ui::open(&self.window, taken, uris, send);
        *self.wizard.borrow_mut() = Some(Rc::downgrade(&w));
        w
    }

    /// Remove the selected printer, after asking.
    fn remove_selected(self: &Rc<Self>) {
        let Some(p) = self.selected_printer() else { return };
        let weak = Rc::downgrade(self);
        let name = p.name.clone();
        opusdm_ui::requester(&self.window, "Remove printer", &format!("Remove {name}?"), Some("Its waiting jobs are canceled."), &[("Remove", true), ("Cancel", false)], move |yes| {
            let Some(a) = alive(&weak, "the Remove answer") else { return };
            if !yes { return; }
            // The changes of a removed printer have nothing to go to.
            if let Some(s) = a.session.borrow_mut().as_mut() { s.revert(); }
            a.send(Job::Remove(name.clone()));
        });
    }

    /// Save the session. Returns whether the save was sent.
    fn save(&self) -> bool {
        let job = match self.session.borrow().as_ref() {
            Some(s) if s.is_dirty() => Job::Save { printer: s.printer.clone(), edit: s.edit() },
            _ => return false,
        };
        self.show_info("Saving...");
        self.send(job)
    }

    /// Drop the changes and show the saved values.
    fn revert(self: &Rc<Self>) {
        if let Some(s) = self.session.borrow_mut().as_mut() { s.revert(); }
        self.fill_entries();
        self.show_options();
        self.update_buttons();
    }

    /// Ask what to do with the changes, then run `then` with whether a save
    /// was sent, unless the user cancels or the save cannot be sent.
    fn ask_before_leaving(self: &Rc<Self>, then: AfterLeave) {
        let name = self.session.borrow().as_ref().map(|s| s.printer.clone()).unwrap_or_default();
        let weak = Rc::downgrade(self);
        opusdm_ui::requester(&self.window, "Unsaved changes", &format!("{name} has changes that are not saved."), None, &[("Save", 0), ("Discard", 1), ("Cancel", 2)], move |choice| {
            let Some(a) = alive(&weak, "the unsaved changes answer") else { return };
            match choice {
                0 => {
                    if a.save() { then(&a, true); }
                }
                1 => {
                    if let Some(s) = a.session.borrow_mut().as_mut() { s.revert(); }
                    then(&a, false);
                }
                _ => {}
            }
        });
    }
}

/// One printer line of the list: the name (a star for the default) and a
/// dim line with the state and the jobs.
fn printer_row(p: &Printer, jobs: usize) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    b.set_margin_start(SPACING);
    b.set_margin_end(SPACING);
    b.set_margin_top(SPACING / 2);
    b.set_margin_bottom(SPACING / 2);
    let title = if p.is_default { format!("\u{2605} {}", p.name) } else { p.name.clone() };
    let name = gtk::Label::new(Some(&title));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    b.append(&name);
    let state = if is_owner_paused(p) { "Disabled" } else { p.state.label() };
    let detail = match jobs {
        0 => state.to_string(),
        1 => format!("{state} - 1 job"),
        n => format!("{state} - {n} jobs"),
    };
    let detail = gtk::Label::new(Some(&detail));
    detail.set_xalign(0.0);
    detail.add_css_class("dim-label");
    if p.worst() == opusdm_printing::model::Severity::Error { detail.add_css_class("error"); }
    b.append(&detail);
    b
}

impl EmbeddedTool for PrinterTool {
    fn title(&self) -> &str {
        "Printers"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    fn is_dirty(&self) -> bool {
        self.app.is_dirty()
    }

    /// The result arrives from the worker: `done` runs then.
    fn save(&self, done: Box<dyn FnOnce(bool)>) {
        if !self.app.is_dirty() { return done(true); }
        *self.app.save_done.borrow_mut() = Some(done);
        if !self.app.save() { self.app.finish_save(false); }
    }

    fn revert(&self, done: Box<dyn FnOnce()>) {
        self.app.revert();
        done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gtk_test;
    use opusdm_printing::backend::{test_job, test_printer, FakeBackend};
    use std::time::{Duration, Instant};

    /// Run the main loop until `done` holds, at most 3 s. Returns whether it held.
    fn iterate_until(done: impl Fn() -> bool) -> bool {
        let ctx = glib::MainContext::default();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            while ctx.iteration(false) {}
            if done() { return true; }
            std::thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    /// A fake with two printers and job 41 on the first.
    fn fake() -> Arc<FakeBackend> {
        let f = FakeBackend::default();
        f.printers.lock().unwrap().extend([test_printer("p1"), test_printer("p2")]);
        f.jobs.lock().unwrap().push(test_job(41, "p1"));
        Arc::new(f)
    }

    #[test]
    fn the_window_stays_alive_and_follows_the_fake() {
        gtk_test::run(|| {
            let f = fake();
            let window = gtk::Window::new();
            let (app, _root) = build_with(&window, f.clone());
            assert!(iterate_until(|| app.row_names.borrow().len() == 2), "the list shows both printers");
            // Many main loop turns later the loops still follow the backend.
            for _ in 0..200 { glib::MainContext::default().iteration(false); }
            f.printers.lock().unwrap().push(test_printer("p3"));
            app.send(crate::worker::Job::Refresh);
            assert!(iterate_until(|| app.row_names.borrow().len() == 3), "a new printer appears");
        });
    }

    #[test]
    fn wizard_adds_a_printer_through_the_backend() {
        gtk_test::run(|| {
            let f = fake();
            f.devices.lock().unwrap().push(opusdm_printing::model::Device { uri: "ipp://10.0.20.7/ipp/print".into(), info: "Brother HL".into(), make_model: "Brother HL-L2350DW".into(), ..Default::default() });
            let window = gtk::Window::new();
            let (app, _root) = build_with(&window, f.clone());
            assert!(iterate_until(|| app.row_names.borrow().len() == 2));
            let wizard = app.open_wizard();
            assert!(iterate_until(|| wizard.device_count() == 1), "the search lists the device");
            wizard.pick_device(0);
            wizard.press_next();
            assert_eq!(wizard.step(), crate::wizard::Step::Driver);
            wizard.press_next();
            assert_eq!(wizard.step(), crate::wizard::Step::Name);
            wizard.press_next();
            assert!(iterate_until(|| f.calls().iter().any(|c| c == "add Brother_HL-L2350DW")), "{:?}", f.calls());
            assert!(iterate_until(|| wizard.step() == crate::wizard::Step::Done));
            assert!(iterate_until(|| app.row_names.borrow().iter().any(|n| n == "Brother_HL-L2350DW")));
            wizard.close();
        });
    }

    #[test]
    fn the_program_window_owns_the_app() {
        gtk_test::run(|| {
            let f = fake();
            let window = gtk::Window::new();
            // GTK ignores a close of a window that has no surface yet.
            WidgetExt::realize(&window);
            let (app, _root) = build_with(&window, f.clone());
            app.guard_close();
            keep_until_closed(&window, app.clone());
            let weak = Rc::downgrade(&app);
            drop(app);
            // Only the window holds the app now, as in the program.
            assert!(iterate_until(|| weak.upgrade().is_some_and(|a| a.row_names.borrow().len() == 2)));
            for _ in 0..200 { glib::MainContext::default().iteration(false); }
            f.printers.lock().unwrap().push(test_printer("p3"));
            weak.upgrade().expect("the window keeps the app").send(crate::worker::Job::Refresh);
            assert!(iterate_until(|| weak.upgrade().is_some_and(|a| a.row_names.borrow().len() == 3)), "the loops still follow the backend");
            // A close with unsaved changes asks and keeps the app.
            {
                let a = weak.upgrade().unwrap();
                a.select("p1".to_string());
                a.general.info.set_text("Desk");
                assert!(a.is_dirty());
            }
            window.close();
            assert!(weak.upgrade().is_some(), "a close waiting for the answer keeps the app");
            // Without changes the close drops it.
            weak.upgrade().unwrap().revert();
            window.close();
            assert!(iterate_until(|| weak.upgrade().is_none()), "a real close drops the app");
        });
    }

    #[test]
    fn a_save_answer_for_another_printer_keeps_the_edit() {
        gtk_test::run(|| {
            let f = fake();
            let window = gtk::Window::new();
            let (app, _root) = build_with(&window, f.clone());
            assert!(iterate_until(|| app.row_names.borrow().len() == 2));
            app.select("p2".to_string());
            app.general.info.set_text("Desk");
            assert!(app.is_dirty());
            // The answer of a save of p1, sent while leaving it, comes after p2 was opened.
            app.on_done(Done::Saved("p1".to_string()));
            assert!(app.is_dirty(), "the edit of p2 is not marked saved");
        });
    }

    #[test]
    fn cancel_from_the_queue_reaches_the_backend() {
        gtk_test::run(|| {
            let f = fake();
            let window = gtk::Window::new();
            let (app, _root) = build_with(&window, f.clone());
            assert!(iterate_until(|| app.row_names.borrow().len() == 2));
            app.select("p1".to_string());
            assert!(iterate_until(|| app.queue_ids.borrow().len() == 1), "the queue of p1 shows job 41");
            app.select_job(41);
            app.queue_action(QueueAction::Cancel);
            assert!(iterate_until(|| f.calls().iter().any(|c| c == "cancel 41")));
            assert!(iterate_until(|| app.queue_ids.borrow().is_empty()), "the queue follows the cancel");
        });
    }
}
