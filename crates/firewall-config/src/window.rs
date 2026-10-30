//! The Firewall page of the Advanced tab: the state, one table of rules per
//! direction and the queue of changes that Apply runs with one password. Every
//! run happens on its own thread, so the window never waits for the password.

use crate::backend::{Backend, PkexecBackend, Report, RunError};
use crate::model::{Direction, Op, Queue, Rule, Snapshot};
use crate::rule_dialog;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ui::EmbeddedTool;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-firewall";
/// Name of the tool in the question about unsaved changes.
const TITLE: &str = "Firewall";
/// Gap around the page and between its parts.
const MARGIN: i32 = 12;
const SPACING: i32 = 6;
/// Stack pages.
const PAGE_LOADING: &str = "loading";
const PAGE_BLOCKED: &str = "blocked";
const PAGE_MAIN: &str = "main";
/// The note of a rule waiting to be added.
const TO_ADD: &str = "to add";
/// The error style of the page.
const APP_CSS: &str = "
label.error { color: #ff7b63; }
";

/// The result of one run on the worker thread.
type RunResult = Result<Report, RunError>;
/// Told how an Apply asked by the host ended.
type SavedCallback = Box<dyn FnOnce(bool)>;

/// What a run was for, to say the right thing when it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunKind {
    Read,
    Apply,
}

/// The widgets of one direction.
struct DirPage {
    direction: Direction,
    tab: gtk::Label,
    policy: gtk::Label,
    scroll: gtk::ScrolledWindow,
}

impl DirPage {
    /// Build the empty page of `direction`.
    fn new(direction: Direction) -> Self {
        let policy = gtk::Label::new(None);
        policy.set_xalign(0.0);
        policy.set_wrap(true);
        let scroll = gtk::ScrolledWindow::builder().vexpand(true).hexpand(true).build();
        scroll.add_css_class("frame");
        Self { direction, tab: gtk::Label::new(None), policy, scroll }
    }

    /// The name of the direction on its tab.
    fn name(&self) -> &'static str {
        match self.direction {
            Direction::In => "IN",
            Direction::Out => "OUT",
        }
    }

    /// The column title of the other side of a rule.
    fn peer_title(&self) -> &'static str {
        match self.direction {
            Direction::In => "From",
            Direction::Out => "To",
        }
    }
}

/// What every callback of the page shares.
pub struct App {
    window: gtk::Window,
    backend: Arc<dyn Backend>,
    results: async_channel::Sender<(RunKind, RunResult)>,
    /// The last state read; `None` before the first good read.
    snapshot: RefCell<Option<Snapshot>>,
    queue: RefCell<Queue>,
    /// Set while a run is going.
    busy: Cell<bool>,
    /// Told how the Apply asked by the host ended.
    on_saved: RefCell<Option<SavedCallback>>,
    root: gtk::Box,
    stack: gtk::Stack,
    blocked_text: gtk::Label,
    state: gtk::Label,
    power: gtk::Button,
    notebook: gtk::Notebook,
    pages: [DirPage; 2],
    message: gtk::Label,
    add: gtk::Button,
    reload: gtk::Button,
    discard: gtk::Button,
    apply: gtk::Button,
    retry: gtk::Button,
    /// A weak handle to this app, for the buttons of the table rows.
    me: RefCell<Weak<App>>,
}

/// The Firewall tool inside another window (Advanced tab of Settings).
pub struct FirewallTool {
    /// Owns the page state while Settings keeps the page: the result loop and
    /// the handlers hold it weakly.
    app: Rc<App>,
}

/// Open the Firewall tool for the host `window`. This reads the firewall at
/// once, so the password is asked now, when the user picks the entry.
pub fn embedded(window: &gtk::Window) -> Rc<FirewallTool> {
    install_css_once();
    Rc::new(FirewallTool { app: build_with(window, Arc::new(PkexecBackend::new())) })
}

/// Add the error style to the display, once.
fn install_css_once() {
    thread_local! { static DONE: Cell<bool> = const { Cell::new(false) }; }
    if DONE.with(|d| d.replace(true)) { return; }
    let Some(display) = gtk::gdk::Display::default() else {
        opusdm_ipc::err_log!(LOG_TAG, "no display: the Firewall styles are not installed");
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(APP_CSS);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

/// Build the page on `backend` and start the first read. The caller keeps the
/// app alive: everything else holds it weakly.
fn build_with(window: &gtk::Window, backend: Arc<dyn Backend>) -> Rc<App> {
    let (results, results_rx) = async_channel::unbounded();
    let app = Rc::new(App {
        window: window.clone(),
        backend,
        results,
        snapshot: RefCell::new(None),
        queue: RefCell::new(Queue::default()),
        busy: Cell::new(false),
        on_saved: RefCell::new(None),
        root: gtk::Box::new(gtk::Orientation::Vertical, 0),
        stack: gtk::Stack::new(),
        blocked_text: gtk::Label::new(None),
        state: gtk::Label::new(None),
        power: gtk::Button::new(),
        notebook: gtk::Notebook::new(),
        pages: [DirPage::new(Direction::In), DirPage::new(Direction::Out)],
        message: gtk::Label::new(None),
        add: gtk::Button::with_label("Add rule..."),
        reload: gtk::Button::with_label("Reload"),
        discard: gtk::Button::with_label("Discard changes"),
        apply: gtk::Button::with_label("Apply"),
        retry: gtk::Button::with_label("Retry"),
        me: RefCell::new(Weak::new()),
    });
    *app.me.borrow_mut() = Rc::downgrade(&app);
    app.layout();
    app.wire();
    let weak = Rc::downgrade(&app);
    glib::spawn_future_local(async move {
        while let Ok((kind, result)) = results_rx.recv().await {
            let Some(a) = weak.upgrade() else {
                opusdm_ipc::err_log!(LOG_TAG, "the Firewall page is gone: the result loop stops");
                return;
            };
            a.on_result(kind, result);
        }
        opusdm_ipc::err_log!(LOG_TAG, "the results channel closed: the loop stops");
    });
    app.start();
    app
}

/// Run `action` on the app when `button` is clicked.
fn connect_button(button: &gtk::Button, weak: &Weak<App>, action: impl Fn(&Rc<App>) + 'static) {
    let weak = weak.clone();
    button.connect_clicked(move |_| match weak.upgrade() {
        Some(a) => action(&a),
        None => opusdm_ipc::err_log!(LOG_TAG, "a button was clicked after the Firewall page was gone"),
    });
}

/// A left aligned cell of a table.
fn cell(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label
}

/// A cell struck through: the rule is going to be removed.
fn struck_cell(text: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_markup(&format!("<s>{}</s>", glib::markup_escape_text(text)));
    label.set_xalign(0.0);
    label
}

/// What the default policy means, for the line above the table.
fn policy_text(policy: &str) -> String {
    let meaning = match policy {
        "allow" => "traffic without a matching rule passes",
        "deny" => "traffic without a matching rule is blocked",
        "reject" => "traffic without a matching rule is refused",
        _ => "the effect on traffic without a matching rule is not known",
    };
    format!("Default policy: {policy} ({meaning})")
}

/// The text of the state of the firewall with the queued change, if any.
fn state_text(active: bool, power: Option<bool>) -> &'static str {
    match (active, power) {
        (true, Some(false)) => "active, will be turned off",
        (false, Some(true)) => "not active, will be turned on",
        (true, _) => "active",
        (false, _) => "not active",
    }
}

impl App {
    /// Build the page.
    fn layout(&self) {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, SPACING * 2);
        let title = gtk::Label::new(Some(TITLE));
        title.add_css_class("heading");
        self.state.set_xalign(0.0);
        self.state.set_hexpand(true);
        header.append(&title);
        header.append(&self.state);
        header.append(&self.power);

        for page in &self.pages {
            let body = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
            body.set_margin_top(SPACING);
            body.set_margin_bottom(SPACING);
            body.set_margin_start(SPACING);
            body.set_margin_end(SPACING);
            body.append(&page.policy);
            body.append(&page.scroll);
            self.notebook.append_page(&body, Some(&page.tab));
        }
        self.notebook.set_vexpand(true);

        self.message.set_xalign(0.0);
        self.message.set_wrap(true);
        self.message.set_visible(false);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        buttons.append(&self.add);
        buttons.append(&self.reload);
        buttons.append(&spacer);
        buttons.append(&self.discard);
        buttons.append(&self.apply);

        let main = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        main.append(&header);
        main.append(&self.notebook);
        main.append(&self.message);
        main.append(&buttons);

        let loading = gtk::Label::new(Some("Waiting for the authorization to read the firewall..."));
        loading.add_css_class("dim-label");

        let blocked = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        blocked.set_valign(gtk::Align::Center);
        self.blocked_text.set_wrap(true);
        self.retry.set_halign(gtk::Align::Center);
        blocked.append(&self.blocked_text);
        blocked.append(&self.retry);

        self.stack.add_named(&loading, Some(PAGE_LOADING));
        self.stack.add_named(&blocked, Some(PAGE_BLOCKED));
        self.stack.add_named(&main, Some(PAGE_MAIN));
        self.stack.set_vexpand(true);
        self.stack.set_hexpand(true);
        self.root.set_margin_top(MARGIN);
        self.root.set_margin_bottom(MARGIN);
        self.root.set_margin_end(MARGIN);
        self.root.append(&self.stack);
    }

    /// Connect the buttons. The closures hold the app weakly.
    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        connect_button(&self.retry, &weak, |a| a.start());
        connect_button(&self.reload, &weak, |a| a.start());
        connect_button(&self.apply, &weak, |a| a.apply_queue());
        connect_button(&self.discard, &weak, |a| a.discard_queue());
        connect_button(&self.power, &weak, |a| a.toggle_power());
        connect_button(&self.add, &weak, |a| a.open_add_dialog());
    }

    /// First read and Retry: say what is missing, or read the firewall.
    fn start(self: &Rc<Self>) {
        if !self.backend.available() {
            self.blocked_text.set_text("ufw is not installed, so there is no firewall to configure.");
            self.retry.set_visible(false);
            self.stack.set_visible_child_name(PAGE_BLOCKED);
            opusdm_ipc::err_log!(LOG_TAG, "ufw not found: the Firewall page shows nothing");
            return;
        }
        if self.snapshot.borrow().is_none() { self.stack.set_visible_child_name(PAGE_LOADING); }
        self.run(RunKind::Read, Vec::new());
    }

    /// Run `ops` on a thread of its own; the result comes back to the loop.
    fn run(&self, kind: RunKind, ops: Vec<Op>) {
        if self.busy.replace(true) {
            opusdm_ipc::err_log!(LOG_TAG, "a {kind:?} was asked while a run is going: ignored");
            return;
        }
        self.show_message(match kind {
            RunKind::Read => "Reading the firewall...",
            RunKind::Apply => "Applying the changes...",
        }, false);
        self.sync_buttons();
        let backend = self.backend.clone();
        let results = self.results.clone();
        let spawned = std::thread::Builder::new().name("opusdm-firewall-run".into()).spawn(move || {
            let result = backend.run(&ops);
            if results.send_blocking((kind, result)).is_err() { opusdm_ipc::err_log!(LOG_TAG, "a run ended after the Firewall page was gone"); }
        });
        if let Err(e) = spawned {
            opusdm_ipc::err_log!(LOG_TAG, "cannot start the firewall run: {e}");
            self.on_result(kind, Err(RunError::Failed(format!("Cannot start the run: {e}"))));
        }
    }

    /// A run ended: take the new state and say how it went.
    fn on_result(&self, kind: RunKind, result: RunResult) {
        self.busy.set(false);
        let ok = match result {
            Ok(report) => self.on_report(kind, report),
            Err(e) => {
                self.on_run_error(&e);
                false
            }
        };
        self.render();
        let saved = self.on_saved.borrow_mut().take();
        if let Some(done) = saved { done(ok); }
    }

    /// The run happened: forget what ran and show the state. `true` when
    /// every command worked.
    fn on_report(&self, kind: RunKind, report: Report) -> bool {
        self.queue.borrow_mut().drop_done(report.done);
        *self.snapshot.borrow_mut() = Some(report.snapshot);
        self.stack.set_visible_child_name(PAGE_MAIN);
        match (&report.failure, kind) {
            (Some(output), _) => {
                opusdm_ipc::err_log!(LOG_TAG, "a ufw command failed: {output}");
                self.show_message(&format!("A change failed, the ones after it did not run and are still queued. ufw said: {output}"), true);
                false
            }
            (None, RunKind::Apply) => {
                self.show_message("The changes were applied.", false);
                true
            }
            (None, RunKind::Read) => {
                self.message.set_visible(false);
                true
            }
        }
    }

    /// The run did not happen or cannot be read: nothing is shown as if it
    /// were the state of the firewall.
    fn on_run_error(&self, error: &RunError) {
        opusdm_ipc::err_log!(LOG_TAG, "firewall run: {}", error.text());
        if self.snapshot.borrow().is_none() {
            self.blocked_text.set_text(&format!("The firewall could not be read. {}", error.text()));
            self.retry.set_visible(true);
            self.stack.set_visible_child_name(PAGE_BLOCKED);
            return;
        }
        let kept = if self.queue.borrow().is_empty() { "" } else { " The queued changes are kept." };
        self.show_message(&format!("{}{kept}", error.text()), true);
    }

    /// Show `text` in the message line.
    fn show_message(&self, text: &str, is_error: bool) {
        self.message.set_text(text);
        self.message.set_visible(true);
        if is_error { self.message.add_css_class("error"); } else { self.message.remove_css_class("error"); }
    }

    /// Enable the buttons that make sense now.
    fn sync_buttons(&self) {
        let idle = !self.busy.get();
        let queued = !self.queue.borrow().is_empty();
        self.apply.set_sensitive(idle && queued);
        self.discard.set_sensitive(idle && queued);
        self.reload.set_sensitive(idle && !queued);
        self.retry.set_sensitive(idle);
        self.add.set_sensitive(idle);
        self.power.set_sensitive(idle);
        self.notebook.set_sensitive(idle);
    }

    /// Show the state and the queue.
    fn render(&self) {
        self.sync_buttons();
        let snapshot = self.snapshot.borrow();
        let Some(snapshot) = snapshot.as_ref() else { return };
        let queue = self.queue.borrow();
        self.state.set_text(&format!("Status: {}", state_text(snapshot.active, queue.power)));
        self.power.set_label(match (queue.power.is_some(), snapshot.active) {
            (true, _) => "Undo",
            (false, true) => "Disable",
            (false, false) => "Enable",
        });
        for page in &self.pages {
            let policy = match page.direction {
                Direction::In => &snapshot.default_in,
                Direction::Out => &snapshot.default_out,
            };
            page.policy.set_text(&policy_text(policy));
            let rules: Vec<&Rule> = snapshot.rules.iter().filter(|r| r.direction == page.direction).collect();
            let added: Vec<Rule> = queue.added.iter().filter(|r| r.direction == page.direction).map(|r| r.preview()).collect();
            let staying = rules.iter().filter(|r| !queue.removed.contains(&r.words)).count();
            page.tab.set_text(&format!("{} ({})", page.name(), staying + added.len()));
            page.scroll.set_child(Some(&self.table(page, &rules, &added, &queue)));
        }
    }

    /// Build the table of one direction.
    fn table(&self, page: &DirPage, rules: &[&Rule], added: &[Rule], queue: &Queue) -> gtk::Widget {
        if rules.is_empty() && added.is_empty() {
            let empty = gtk::Label::new(Some("No rules in this direction: only the default policy applies."));
            empty.add_css_class("dim-label");
            return empty.upcast();
        }
        let grid = gtk::Grid::new();
        grid.set_column_spacing(MARGIN as u32 * 2);
        grid.set_row_spacing(SPACING as u32);
        grid.set_margin_top(SPACING);
        grid.set_margin_bottom(SPACING);
        grid.set_margin_start(SPACING);
        grid.set_margin_end(SPACING);
        for (col, title) in ["Port", "Proto", "Action", page.peer_title(), "Notes"].into_iter().enumerate() {
            let label = cell(title);
            label.add_css_class("heading");
            grid.attach(&label, col as i32, 0, 1, 1);
        }
        let mut row = 1;
        for rule in rules {
            let removing = queue.removed.contains(&rule.words);
            self.table_row(&grid, row, rule, &rule.notes, removing);
            let button = gtk::Button::with_label(if removing { "Undo" } else { "Remove" });
            let words = rule.words.clone();
            connect_button(&button, &self.weak_self(), move |a| a.toggle_removed(&words));
            grid.attach(&button, 5, row, 1, 1);
            row += 1;
        }
        for (rule, new_rule) in added.iter().zip(queue.added.iter().filter(|r| r.direction == page.direction)) {
            self.table_row(&grid, row, rule, TO_ADD, false);
            let button = gtk::Button::with_label("Undo");
            let new_rule = new_rule.clone();
            connect_button(&button, &self.weak_self(), move |a| a.drop_added(&new_rule));
            grid.attach(&button, 5, row, 1, 1);
            row += 1;
        }
        grid.upcast()
    }

    /// Fill the text cells of one row.
    fn table_row(&self, grid: &gtk::Grid, row: i32, rule: &Rule, notes: &str, struck: bool) {
        let make = if struck { struck_cell } else { cell };
        for (col, text) in [rule.port.as_str(), rule.proto.as_str(), rule.action.as_str(), rule.peer.as_str()].into_iter().enumerate() {
            grid.attach(&make(text), col as i32, row, 1, 1);
        }
        let notes_cell = make(if struck && notes.is_empty() { " " } else { notes });
        notes_cell.set_hexpand(true);
        grid.attach(&notes_cell, 4, row, 1, 1);
    }

    /// A weak handle to this app.
    fn weak_self(&self) -> Weak<App> {
        self.me.borrow().clone()
    }

    /// Queue the removal of a rule, or take it back.
    fn toggle_removed(self: &Rc<Self>, words: &[String]) {
        self.queue.borrow_mut().toggle_removed(words);
        self.message.set_visible(false);
        self.render();
    }

    /// Take a queued addition back.
    fn drop_added(self: &Rc<Self>, rule: &crate::model::NewRule) {
        self.queue.borrow_mut().added.retain(|r| r != rule);
        self.message.set_visible(false);
        self.render();
    }

    /// Queue turning the firewall on or off, or take it back.
    fn toggle_power(self: &Rc<Self>) {
        let Some(active) = self.snapshot.borrow().as_ref().map(|s| s.active) else {
            opusdm_ipc::err_log!(LOG_TAG, "the power button was clicked before the first read");
            return;
        };
        self.queue.borrow_mut().toggle_power(active);
        self.message.set_visible(false);
        self.render();
    }

    /// Open the form on the direction of the tab in front.
    fn open_add_dialog(self: &Rc<Self>) {
        let direction = if self.notebook.current_page() == Some(1) { Direction::Out } else { Direction::In };
        let weak = Rc::downgrade(self);
        rule_dialog::open(&self.window, direction, move |rule| match weak.upgrade() {
            Some(a) => {
                a.queue.borrow_mut().add(rule);
                a.message.set_visible(false);
                a.render();
            }
            None => opusdm_ipc::err_log!(LOG_TAG, "a rule was added after the Firewall page was gone"),
        });
    }

    /// Run the queue.
    fn apply_queue(self: &Rc<Self>) {
        let ops = self.queue.borrow().ops();
        if ops.is_empty() { return; }
        self.run(RunKind::Apply, ops);
    }

    /// Forget the queue.
    fn discard_queue(&self) {
        *self.queue.borrow_mut() = Queue::default();
        self.message.set_visible(false);
        self.render();
    }
}

impl EmbeddedTool for FirewallTool {
    fn title(&self) -> &str {
        TITLE
    }

    fn widget(&self) -> gtk::Widget {
        self.app.root.clone().upcast()
    }

    fn is_dirty(&self) -> bool {
        !self.app.queue.borrow().is_empty()
    }

    fn save(&self, done: Box<dyn FnOnce(bool)>) {
        if self.app.busy.get() {
            self.app.show_message("A run is going: wait for it to end, then try again.", true);
            return done(false);
        }
        let ops = self.app.queue.borrow().ops();
        if ops.is_empty() { return done(true); }
        *self.app.on_saved.borrow_mut() = Some(done);
        self.app.run(RunKind::Apply, ops);
    }

    fn revert(&self, done: Box<dyn FnOnce()>) {
        self.app.discard_queue();
        done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gtk_test;
    use crate::model::{parse_rule, split_words, Action, NewRule, Protocol};
    use std::sync::Mutex;

    /// Answers from a list and records the commands it was given.
    #[derive(Default)]
    struct FakeBackend {
        missing: bool,
        answers: Mutex<Vec<RunResult>>,
        calls: Mutex<Vec<Vec<Op>>>,
    }

    impl Backend for FakeBackend {
        fn available(&self) -> bool {
            !self.missing
        }

        fn run(&self, ops: &[Op]) -> RunResult {
            self.calls.lock().unwrap().push(ops.to_vec());
            self.answers.lock().unwrap().remove(0)
        }
    }

    /// A state with one rule per direction.
    fn snapshot() -> Snapshot {
        let rule = |line: &str| parse_rule(split_words(line));
        Snapshot { active: true, default_in: "deny".into(), default_out: "allow".into(), rules: vec![rule("allow 22/tcp"), rule("deny out 25/tcp")] }
    }

    /// A good answer after `done` commands.
    fn good(done: usize) -> RunResult {
        Ok(Report { done, failure: None, snapshot: snapshot() })
    }

    /// Run the main loop until `ready`, failing after a while.
    fn pump_until(ready: impl Fn() -> bool) {
        let context = glib::MainContext::default();
        for _ in 0..2000 {
            if ready() { return; }
            while context.iteration(false) {}
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the page did not reach the expected state");
    }

    /// Run the main loop for a while with nothing to wait for.
    fn pump_some() {
        let context = glib::MainContext::default();
        for _ in 0..50 { context.iteration(false); }
    }

    /// The page on a fake, with these answers.
    fn page(backend: FakeBackend, answers: Vec<RunResult>) -> (Rc<App>, Arc<FakeBackend>) {
        *backend.answers.lock().unwrap() = answers;
        let backend = Arc::new(backend);
        (build_with(&gtk::Window::new(), backend.clone()), backend)
    }

    /// A new IN rule on `port`.
    fn new_rule(port: &str) -> NewRule {
        NewRule::from_form(Direction::In, port, Protocol::Tcp, Action::Allow, "").unwrap()
    }

    #[test]
    fn the_page_stays_alive_and_apply_still_works_after_the_first_read() {
        gtk_test::run(|| {
            let (app, backend) = page(FakeBackend::default(), vec![good(0), good(2), good(0)]);
            pump_until(|| !app.busy.get());
            assert_eq!(app.stack.visible_child_name().as_deref(), Some(PAGE_MAIN));
            assert_eq!(app.state.text(), "Status: active");
            assert_eq!(app.pages[0].tab.text(), "IN (1)");
            assert_eq!(app.pages[1].tab.text(), "OUT (1)");
            assert!(!app.apply.is_sensitive());
            pump_some();

            // Queue through the real handlers: the page is still there.
            app.power.emit_clicked();
            assert_eq!(app.state.text(), "Status: active, will be turned off");
            app.power.emit_clicked();
            assert!(!app.apply.is_sensitive());
            app.queue.borrow_mut().add(new_rule("80"));
            app.toggle_removed(&split_words("allow 22/tcp"));
            app.render();
            assert_eq!(app.pages[0].tab.text(), "IN (1)");
            assert!(app.apply.is_sensitive());
            assert!(!app.reload.is_sensitive());
            app.apply.emit_clicked();
            assert!(!app.apply.is_sensitive(), "a run is going");
            pump_until(|| !app.busy.get());

            let calls = backend.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), 2, "the read and the Apply, no second read");
            assert!(calls[0].is_empty());
            let args: Vec<String> = calls[1].iter().map(|o| o.args.join(" ")).collect();
            assert_eq!(args, ["--force delete allow 22/tcp", "allow in 80/tcp"]);
            assert!(app.queue.borrow().is_empty());
            assert_eq!(app.message.text(), "The changes were applied.");
            assert!(app.reload.is_sensitive());

            app.reload.emit_clicked();
            pump_until(|| !app.busy.get());
            assert_eq!(backend.calls.lock().unwrap().len(), 3);
        });
    }

    #[test]
    fn a_cancelled_first_read_shows_no_rules_and_can_be_retried() {
        gtk_test::run(|| {
            let (app, backend) = page(FakeBackend::default(), vec![Err(RunError::Cancelled), good(0)]);
            pump_until(|| !app.busy.get());
            assert_eq!(app.stack.visible_child_name().as_deref(), Some(PAGE_BLOCKED));
            assert!(app.blocked_text.text().contains("cancelled"));
            assert!(app.retry.is_visible());
            app.retry.emit_clicked();
            pump_until(|| !app.busy.get());
            assert_eq!(app.stack.visible_child_name().as_deref(), Some(PAGE_MAIN));
            assert_eq!(backend.calls.lock().unwrap().len(), 2);
        });
    }

    #[test]
    fn a_missing_ufw_is_said_and_asks_no_password() {
        gtk_test::run(|| {
            let (app, backend) = page(FakeBackend { missing: true, ..Default::default() }, vec![]);
            pump_some();
            assert_eq!(app.stack.visible_child_name().as_deref(), Some(PAGE_BLOCKED));
            assert!(app.blocked_text.text().contains("not installed"));
            assert!(!app.retry.is_visible());
            assert!(backend.calls.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn a_failed_or_cancelled_apply_keeps_what_did_not_run() {
        gtk_test::run(|| {
            let failed = Ok(Report { done: 1, failure: Some("ERROR: Bad port".to_string()), snapshot: snapshot() });
            let (app, _backend) = page(FakeBackend::default(), vec![good(0), Err(RunError::Cancelled), failed]);
            pump_until(|| !app.busy.get());
            app.queue.borrow_mut().add(new_rule("80"));
            app.queue.borrow_mut().add(new_rule("81"));
            app.render();

            app.apply.emit_clicked();
            pump_until(|| !app.busy.get());
            assert_eq!(app.queue.borrow().added.len(), 2);
            assert_eq!(app.stack.visible_child_name().as_deref(), Some(PAGE_MAIN));
            assert!(app.message.text().contains("cancelled"));
            assert!(app.message.text().contains("kept"));

            app.apply.emit_clicked();
            pump_until(|| !app.busy.get());
            assert_eq!(app.queue.borrow().added, [new_rule("81")]);
            assert!(app.message.text().contains("Bad port"));
            assert!(app.apply.is_sensitive());
        });
    }

    #[test]
    fn the_host_saves_and_discards_through_the_tool() {
        gtk_test::run(|| {
            let (app, backend) = page(FakeBackend::default(), vec![good(0), good(1)]);
            let tool = FirewallTool { app: app.clone() };
            pump_until(|| !app.busy.get());
            assert!(!tool.is_dirty());

            app.queue.borrow_mut().add(new_rule("80"));
            assert!(tool.is_dirty());
            let reverted = Rc::new(Cell::new(false));
            let flag = reverted.clone();
            tool.revert(Box::new(move || flag.set(true)));
            assert!(reverted.get() && !tool.is_dirty());

            app.queue.borrow_mut().add(new_rule("80"));
            let saved = Rc::new(Cell::new(None));
            let flag = saved.clone();
            tool.save(Box::new(move |ok| flag.set(Some(ok))));
            pump_until(|| saved.get().is_some());
            assert_eq!(saved.get(), Some(true));
            assert!(!tool.is_dirty());
            assert_eq!(backend.calls.lock().unwrap().len(), 2);
        });
    }
}
