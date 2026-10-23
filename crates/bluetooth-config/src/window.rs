//! The window: the adapter switch and options at the top, the paired devices
//! on the left, the details of the selected one on the right, a message line
//! at the bottom. Every BlueZ call goes through the worker thread; the
//! watcher thread follows BlueZ and serves the agent of the pairings started
//! here.

use crate::agent_dialog::AgentHost;
use crate::pages::{device_row, status_text, DetailPage, MARGIN, SPACING};
use crate::pair_dialog::{PairCallbacks, PairDialog};
use crate::worker::{self, Done, Job};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_bluetooth::agent::MANAGER_AGENT_PATH;
use opusdm_bluetooth::{error, watch, AgentEvent, AgentSetup, BtState, Request, WatchMsg};
use opusdm_ui::EmbeddedTool;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::mpsc;

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-bluetooth-manager";
/// Default window size.
const WINDOW_W: i32 = 820;
const WINDOW_H: i32 = 560;
/// Width of the list side.
const LIST_W: i32 = 280;
/// The text of the right side with nothing selected.
const NOTHING_SELECTED: &str = "Choose a device, or add one";
/// Stack pages of the right side.
const PAGE_EMPTY: &str = "empty";
const PAGE_DEVICE: &str = "device";

/// The error style of the window.
pub const APP_CSS: &str = "
label.error { color: #ff7b63; }
";

/// What every callback of the window shares.
pub struct App {
    /// The window the dialogs belong to: the program's own, or the host's.
    window: gtk::Window,
    /// Feeds the worker; `None` until the watcher sent the backend.
    jobs: RefCell<Option<mpsc::Sender<Job>>>,
    done_tx: async_channel::Sender<Done>,
    state: RefCell<BtState>,
    /// Whether BlueZ answered the last time.
    available: Cell<bool>,
    agent: Rc<AgentHost>,
    power: gtk::Switch,
    status: gtk::Label,
    visible: gtk::CheckButton,
    adapter_name: gtk::Entry,
    rename_adapter: gtk::Button,
    add: gtk::Button,
    list_box: gtk::ListBox,
    /// The device of each row of the list.
    row_paths: RefCell<Vec<String>>,
    selected: RefCell<Option<String>>,
    detail: gtk::Stack,
    details: DetailPage,
    message: gtk::Label,
    /// Set while the code fills widgets, so their signals change nothing.
    quiet: Cell<bool>,
    /// The "Add device" dialog, while it is open.
    pairing: RefCell<Option<Rc<PairDialog>>>,
    /// Open "Add device" once the first state arrives (`--pair`).
    pair_on_start: Cell<bool>,
}

/// Open the window of `app`; with `pair`, "Add device" opens at once.
pub fn open(app: &gtk::Application, pair: bool) {
    let window = gtk::ApplicationWindow::builder().application(app).title("Bluetooth").default_width(WINDOW_W).default_height(WINDOW_H).build();
    let (state, layout) = build_real(window.upcast_ref());
    state.pair_on_start.set(pair);
    window.set_child(Some(&layout));
    keep_until_closed(window.upcast_ref(), state);
    window.present();
}

/// The Bluetooth tool inside another window (Advanced tab of Settings).
pub struct BluetoothTool {
    /// Owns the window state while Settings keeps the page: the loops and
    /// the handlers hold it weakly.
    _app: Rc<App>,
    root: gtk::Box,
}

/// Open the Bluetooth tool for the host `window`. The devices appear a
/// moment after the page, once the watcher has read BlueZ.
pub fn embedded(window: &gtk::Window) -> Rc<BluetoothTool> {
    install_css_once();
    let (app, root) = build_real(window);
    Rc::new(BluetoothTool { _app: app, root })
}

/// Add the error style to the display, once.
fn install_css_once() {
    thread_local! { static DONE: Cell<bool> = const { Cell::new(false) }; }
    if DONE.with(|d| d.replace(true)) { return; }
    let Some(display) = gtk::gdk::Display::default() else {
        opusdm_ipc::err_log!(LOG_TAG, "no display: the Bluetooth styles are not installed");
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(APP_CSS);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

/// Build on the real BlueZ: the watcher serves the agent of this window.
fn build_real(window: &gtk::Window) -> (Rc<App>, gtk::Box) {
    let (events, agent_rx) = async_channel::unbounded();
    let watch_rx = watch::spawn(Some(AgentSetup { path: MANAGER_AGENT_PATH, default: false, events }));
    build_with(window, watch_rx, agent_rx)
}

/// Build the window content for `window`, fed by `watch_rx` (the backend
/// and the states) and `agent_rx` (the pairing requests). The caller keeps
/// the app alive: everything else holds it weakly.
fn build_with(window: &gtk::Window, watch_rx: async_channel::Receiver<WatchMsg>, agent_rx: async_channel::Receiver<AgentEvent>) -> (Rc<App>, gtk::Box) {
    let (done_tx, done_rx) = async_channel::unbounded();
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
        jobs: RefCell::new(None),
        done_tx,
        state: RefCell::new(BtState::default()),
        available: Cell::new(false),
        agent: AgentHost::new(Some(window.clone())),
        power: gtk::Switch::new(),
        status: gtk::Label::new(Some("Reading Bluetooth...")),
        visible: gtk::CheckButton::with_label("Visible to other devices"),
        adapter_name: gtk::Entry::new(),
        rename_adapter: gtk::Button::with_label("Rename"),
        add: gtk::Button::with_label("Add device..."),
        list_box,
        row_paths: RefCell::new(Vec::new()),
        selected: RefCell::new(None),
        detail: gtk::Stack::new(),
        details: DetailPage::new(),
        message,
        quiet: Cell::new(false),
        pairing: RefCell::new(None),
        pair_on_start: Cell::new(false),
    });
    let layout = app.layout();
    app.wire();
    app.refresh();
    let weak = Rc::downgrade(&app);
    listen(&weak, watch_rx, "BlueZ watcher", App::on_watch);
    listen(&weak, done_rx, "worker reports", App::on_done);
    listen(&weak, agent_rx, "pairing agent", App::on_agent);
    (app, layout)
}

/// Hand every message of `rx` to `handle` on the app. The loop holds the app
/// weakly and ends, with a log line, once the window is gone.
fn listen<T: 'static>(weak: &Weak<App>, rx: async_channel::Receiver<T>, name: &'static str, handle: fn(&Rc<App>, T)) {
    let weak = weak.clone();
    glib::spawn_future_local(async move {
        while let Ok(message) = rx.recv().await {
            let Some(a) = weak.upgrade() else {
                opusdm_ipc::err_log!(LOG_TAG, "the Bluetooth window is gone: the {name} loop stops");
                return;
            };
            handle(&a, message);
        }
        opusdm_ipc::err_log!(LOG_TAG, "the {name} channel closed: the loop stops");
    });
}

/// Hold the only strong reference to `app` until `window` closes. Dropping
/// it ends the worker.
fn keep_until_closed(window: &gtk::Window, app: Rc<App>) {
    let slot = RefCell::new(Some(app));
    window.connect_close_request(move |_| {
        slot.borrow_mut().take();
        glib::Propagation::Proceed
    });
}

/// Run `action` on the app when `button` is clicked.
fn connect_button(button: &gtk::Button, weak: &Weak<App>, action: impl Fn(&Rc<App>) + 'static) {
    let weak = weak.clone();
    button.connect_clicked(move |_| match weak.upgrade() {
        Some(a) => action(&a),
        None => opusdm_ipc::err_log!(LOG_TAG, "a button was clicked after the Bluetooth window was gone"),
    });
}

/// Run `action` on the app when the user flips `switch` (not when the code does).
fn connect_switch(switch: &gtk::Switch, weak: &Weak<App>, action: impl Fn(&Rc<App>, bool) + 'static) {
    let weak = weak.clone();
    switch.connect_active_notify(move |s| match weak.upgrade() {
        Some(a) if !a.quiet.get() => action(&a, s.is_active()),
        Some(_) => {}
        None => opusdm_ipc::err_log!(LOG_TAG, "a switch was flipped after the Bluetooth window was gone"),
    });
}

impl App {
    /// The header, the list side, the details and the message line.
    fn layout(&self) -> gtk::Box {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        header.set_margin_top(MARGIN);
        header.set_margin_start(MARGIN);
        header.set_margin_end(MARGIN);
        self.power.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some("Bluetooth"));
        title.add_css_class("heading");
        self.status.set_xalign(0.0);
        self.status.set_hexpand(true);
        self.status.add_css_class("dim-label");
        header.append(&self.power);
        header.append(&title);
        header.append(&self.status);
        header.append(&self.visible);

        let name_row = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        name_row.set_margin_start(MARGIN);
        name_row.set_margin_end(MARGIN);
        let name_label = gtk::Label::new(Some("Name of this computer"));
        self.adapter_name.set_hexpand(true);
        name_row.append(&name_label);
        name_row.append(&self.adapter_name);
        name_row.append(&self.rename_adapter);

        let left = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        left.set_margin_bottom(MARGIN);
        left.set_margin_start(MARGIN);
        left.set_size_request(LIST_W, -1);
        let scroll = gtk::ScrolledWindow::builder().child(&self.list_box).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        scroll.add_css_class("frame");
        left.append(&scroll);
        left.append(&self.add);

        let empty = gtk::Label::new(Some(NOTHING_SELECTED));
        empty.add_css_class("dim-label");
        self.detail.add_named(&empty, Some(PAGE_EMPTY));
        self.detail.add_named(&self.details.root, Some(PAGE_DEVICE));
        self.detail.set_hexpand(true);
        let body = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        body.set_vexpand(true);
        body.append(&left);
        body.append(&self.detail);

        let root = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        root.append(&header);
        root.append(&name_row);
        root.append(&body);
        root.append(&self.message);
        root
    }

    /// Connect the widgets. The handlers hold the app weakly.
    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        connect_switch(&self.power, &weak, |a, on| a.send(Request::SetPowered(on)));
        {
            let weak = weak.clone();
            self.visible.connect_toggled(move |b| match weak.upgrade() {
                Some(a) if !a.quiet.get() => a.send(Request::SetDiscoverable(b.is_active())),
                Some(_) => {}
                None => opusdm_ipc::err_log!(LOG_TAG, "Visible was toggled after the Bluetooth window was gone"),
            });
        }
        connect_button(&self.rename_adapter, &weak, |a| {
            let name = a.adapter_name.text().trim().to_string();
            if !name.is_empty() { a.send(Request::SetAdapterAlias(name)); }
        });
        connect_button(&self.add, &weak, |a| a.open_pairing());
        {
            let weak = weak.clone();
            self.list_box.connect_selected_rows_changed(move |list| {
                let Some(a) = weak.upgrade() else { return opusdm_ipc::err_log!(LOG_TAG, "a device was selected after the Bluetooth window was gone") };
                if a.quiet.get() { return; }
                let path = list.selected_row().and_then(|r| a.row_paths.borrow().get(usize::try_from(r.index()).ok()?).cloned());
                *a.selected.borrow_mut() = path;
                a.show_details(true);
            });
        }
        connect_button(&self.details.rename, &weak, |a| {
            let name = a.details.name.text().trim().to_string();
            if let Some(dev) = a.selected.borrow().clone() { a.send(Request::SetAlias(dev, name)); }
        });
        connect_button(&self.details.connect, &weak, |a| {
            let Some(d) = a.selected_device() else { return };
            a.send(if d.connected { Request::Disconnect(d.path) } else { Request::Connect(d.path) });
            a.show_info(if d.connected { "Disconnecting..." } else { "Connecting..." });
        });
        connect_button(&self.details.remove, &weak, |a| a.ask_remove());
        connect_switch(&self.details.trusted, &weak, |a, on| {
            if let Some(dev) = a.selected.borrow().clone() { a.send(Request::SetTrusted(dev, on)); }
        });
        connect_switch(&self.details.blocked, &weak, |a, on| {
            if let Some(dev) = a.selected.borrow().clone() { a.send(Request::SetBlocked(dev, on)); }
        });
    }

    /// Hand a request to the worker.
    fn send(&self, request: Request) {
        self.send_job(Job::Run(request));
    }

    /// Hand a job to the worker; without one, say why.
    fn send_job(&self, job: Job) {
        let sent = self.jobs.borrow().as_ref().is_some_and(|tx| tx.send(job.clone()).is_ok());
        if sent { return; }
        opusdm_ipc::err_log!(LOG_TAG, "{job:?} not sent: no connection to BlueZ");
        self.show_error("The Bluetooth service is not reachable");
    }

    /// One message of the watcher.
    fn on_watch(self: &Rc<Self>, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(backend) => {
                // A new session (BlueZ came back): the old worker ends with its sender.
                *self.jobs.borrow_mut() = Some(worker::spawn(backend, self.done_tx.clone()));
                return;
            }
            WatchMsg::State(s) => {
                self.available.set(true);
                *self.state.borrow_mut() = s;
            }
            WatchMsg::Unavailable(why) => {
                opusdm_ipc::err_log!(LOG_TAG, "BlueZ is not available: {why}");
                self.available.set(false);
                *self.jobs.borrow_mut() = None;
                *self.state.borrow_mut() = BtState::default();
            }
        }
        self.refresh();
        self.agent.on_state(&self.state.borrow());
        if self.pair_on_start.get() && self.state.borrow().adapter.is_some() {
            self.pair_on_start.set(false);
            self.open_pairing();
        }
    }

    /// One report of the worker.
    fn on_done(self: &Rc<Self>, done: Done) {
        match done {
            Done::Failed(Request::StopDiscovery, _) => {}
            Done::Failed(_, e) => self.show_error(&error::text(&e)),
            Done::Paired(device, e) => {
                self.agent.close();
                let name = self.state.borrow().device_name(&device);
                let paired = self.state.borrow().device(&device).is_some_and(|d| d.paired);
                let dialog = self.pairing.borrow().clone();
                match e {
                    None => {
                        self.show_info(&format!("{name} is paired and connected"));
                        *self.selected.borrow_mut() = Some(device.clone());
                        if let Some(d) = dialog { d.close(); }
                    }
                    Some(e) if paired => {
                        self.show_error(&format!("{name} is paired, but did not connect: {}", error::text(&e)));
                        *self.selected.borrow_mut() = Some(device.clone());
                        if let Some(d) = dialog { d.close(); }
                    }
                    Some(e) => match dialog {
                        Some(d) => d.pairing_ended(&device, Some(error::text(&e))),
                        None => self.show_error(&format!("Pairing {name} failed: {}", error::text(&e))),
                    },
                }
                self.refresh();
            }
        }
    }

    /// One request of the pairing agent.
    fn on_agent(self: &Rc<Self>, event: AgentEvent) {
        let state = self.state.borrow().clone();
        self.agent.on_event(&state, event);
    }

    /// Open "Add device", or bring it to the front. It turns Bluetooth on,
    /// opens the adapter to pairing and starts a scan; closing it undoes
    /// the last two.
    fn open_pairing(self: &Rc<Self>) {
        if let Some(d) = self.pairing.borrow().as_ref() { return d.present(); }
        if !self.state.borrow().is_on() { self.send(Request::SetPowered(true)); }
        self.send(Request::SetPairable(true));
        self.send(Request::StartDiscovery);
        let weak = Rc::downgrade(self);
        let callbacks = PairCallbacks {
            pair: Box::new({
                let weak = weak.clone();
                move |device| match weak.upgrade() {
                    Some(a) => a.send_job(Job::PairAndConnect(device)),
                    None => opusdm_ipc::err_log!(LOG_TAG, "Pair asked after the Bluetooth window was gone"),
                }
            }),
            closed: Box::new(move |pairing| match weak.upgrade() {
                Some(a) => a.pairing_closed(pairing),
                None => opusdm_ipc::err_log!(LOG_TAG, "the pairing dialog closed after the Bluetooth window was gone"),
            }),
        };
        let dialog = PairDialog::open(&self.window, callbacks);
        dialog.refresh(&self.state.borrow());
        *self.pairing.borrow_mut() = Some(dialog);
    }

    /// "Add device" closed: stop the scan and the pairing in progress.
    fn pairing_closed(&self, pairing: Option<String>) {
        self.pairing.borrow_mut().take();
        if let Some(device) = pairing { self.send(Request::CancelPairing(device)); }
        self.send(Request::StopDiscovery);
        self.send(Request::SetPairable(false));
    }

    /// Ask, then forget the selected device.
    fn ask_remove(self: &Rc<Self>) {
        let Some(d) = self.selected_device() else { return };
        let weak = Rc::downgrade(self);
        opusdm_ui::requester(&self.window, "Remove device", &format!("Remove {}?", d.name), Some("To use it again it must be paired again."), &[("Remove", true), ("Cancel", false)], move |yes| {
            if !yes { return; }
            match weak.upgrade() {
                Some(a) => {
                    a.send(Request::Remove(d.path.clone()));
                    a.selected.borrow_mut().take();
                }
                None => opusdm_ipc::err_log!(LOG_TAG, "Remove confirmed after the Bluetooth window was gone"),
            }
        });
    }

    /// The selected device, if it is still known.
    fn selected_device(&self) -> Option<opusdm_bluetooth::Device> {
        let path = self.selected.borrow().clone()?;
        self.state.borrow().device(&path).cloned()
    }

    /// Show the state everywhere.
    fn refresh(&self) {
        let state = self.state.borrow().clone();
        let available = self.available.get();
        self.quiet.set(true);
        let adapter = state.adapter.clone();
        let usable = available && adapter.is_some();
        self.power.set_active(state.is_on());
        self.power.set_sensitive(usable && !state.rfkill.hard);
        self.status.set_text(status_text(available, &state));
        self.visible.set_active(adapter.as_ref().is_some_and(|a| a.discoverable));
        self.visible.set_sensitive(usable && state.is_on());
        if let Some(a) = &adapter {
            if !self.adapter_name.has_focus() { self.adapter_name.set_text(&a.alias); }
        }
        self.adapter_name.set_sensitive(usable);
        self.rename_adapter.set_sensitive(usable);
        self.add.set_sensitive(usable && !state.rfkill.hard);
        self.fill_list(&state);
        self.quiet.set(false);
        self.show_details(false);
        if let Some(d) = self.pairing.borrow().as_ref() { d.refresh(&state); }
    }

    /// The paired devices, keeping the selection.
    fn fill_list(&self, state: &BtState) {
        opusdm_ui::clear_children(&self.list_box);
        let paired = state.paired();
        *self.row_paths.borrow_mut() = paired.iter().map(|d| d.path.clone()).collect();
        for d in &paired { self.list_box.append(&device_row(d, true)); }
        let selected = self.selected.borrow().clone();
        let index = selected.and_then(|s| paired.iter().position(|d| d.path == s));
        match index.and_then(|i| self.list_box.row_at_index(i as i32)) {
            Some(row) => self.list_box.select_row(Some(&row)),
            None => { self.selected.borrow_mut().take(); }
        }
    }

    /// The details of the selected device; `edit_name` also resets the name entry.
    fn show_details(&self, edit_name: bool) {
        let Some(d) = self.selected_device() else { return self.detail.set_visible_child_name(PAGE_EMPTY) };
        self.quiet.set(true);
        let editing = self.details.name.has_focus();
        self.details.show(&d, edit_name || !editing);
        self.details.connect.set_sensitive(!d.blocked && self.state.borrow().is_on());
        self.quiet.set(false);
        self.detail.set_visible_child_name(PAGE_DEVICE);
    }

    /// Show an error on the message line.
    fn show_error(&self, text: &str) {
        self.message.set_text(text);
        self.message.add_css_class("error");
    }

    /// Show a plain line on the message line.
    fn show_info(&self, text: &str) {
        self.message.set_text(text);
        self.message.remove_css_class("error");
    }
}

impl EmbeddedTool for BluetoothTool {
    fn title(&self) -> &str {
        "Bluetooth"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Every change applies at once: nothing waits to be saved.
    fn is_dirty(&self) -> bool {
        false
    }

    fn save(&self, done: Box<dyn FnOnce(bool)>) {
        done(true);
    }

    fn revert(&self, done: Box<dyn FnOnce()>) {
        done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gtk_test;
    use opusdm_bluetooth::model::test_device;
    use opusdm_bluetooth::{Adapter, Backend, FakeBackend};
    use std::sync::Arc;
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

    /// A powered adapter with a paired keyboard and a phone found by a scan.
    fn state() -> BtState {
        BtState {
            adapter: Some(Adapter { path: "/hci0".into(), alias: "office".into(), powered: true, ..Default::default() }),
            devices: vec![test_device("/hci0/dev_k", "Keys", true, false), test_device("/hci0/dev_p", "Pixel", false, false)],
            ..Default::default()
        }
    }

    /// A window fed by `watch_tx` over the fake; the agent channel is returned too.
    fn build(window: &gtk::Window, fake: &Arc<FakeBackend>) -> (Rc<App>, gtk::Box, async_channel::Sender<WatchMsg>, async_channel::Sender<AgentEvent>) {
        let (watch_tx, watch_rx) = async_channel::unbounded();
        let (agent_tx, agent_rx) = async_channel::unbounded();
        let (app, root) = build_with(window, watch_rx, agent_rx);
        let backend: Arc<dyn Backend> = fake.clone();
        watch_tx.send_blocking(WatchMsg::Backend(backend)).unwrap();
        watch_tx.send_blocking(WatchMsg::State(state())).unwrap();
        (app, root, watch_tx, agent_tx)
    }

    #[test]
    fn the_program_window_owns_the_app() {
        gtk_test::run(|| {
            let fake = Arc::new(FakeBackend::default());
            let window = gtk::Window::new();
            // GTK ignores a close of a window that has no surface yet.
            WidgetExt::realize(&window);
            let (app, root, watch_tx, _agent) = build(&window, &fake);
            window.set_child(Some(&root));
            let weak = Rc::downgrade(&app);
            // Only the window holds the app now, as in the program.
            keep_until_closed(&window, app);
            for _ in 0..200 { glib::MainContext::default().iteration(false); }
            assert!(iterate_until(|| weak.upgrade().is_some_and(|a| a.row_paths.borrow().len() == 1)), "the window follows the watcher");
            let mut s = state();
            s.devices[1].paired = true;
            watch_tx.send_blocking(WatchMsg::State(s)).unwrap();
            assert!(iterate_until(|| weak.upgrade().is_some_and(|a| a.row_paths.borrow().len() == 2)), "still alive after many turns");
            window.close();
            assert!(iterate_until(|| weak.upgrade().is_none()), "a real close drops the app");
        });
    }

    #[test]
    fn connect_button_reaches_backend() {
        gtk_test::run(|| {
            let fake = Arc::new(FakeBackend::default());
            let window = gtk::Window::new();
            let (app, _root, _watch, _agent) = build(&window, &fake);
            assert!(iterate_until(|| app.row_paths.borrow().len() == 1));
            app.list_box.select_row(app.list_box.row_at_index(0).as_ref());
            assert_eq!(app.selected.borrow().as_deref(), Some("/hci0/dev_k"));
            app.details.connect.emit_clicked();
            assert!(iterate_until(|| fake.calls().contains(&Request::Connect("/hci0/dev_k".into()))));
        });
    }

    #[test]
    fn closing_pairing_stops_discovery() {
        gtk_test::run(|| {
            let fake = Arc::new(FakeBackend::default());
            let window = gtk::Window::new();
            let (app, _root, _watch, _agent) = build(&window, &fake);
            assert!(iterate_until(|| app.available.get()));
            app.open_pairing();
            let dialog = app.pairing.borrow().clone().unwrap();
            assert_eq!(dialog.rows(), vec!["/hci0/dev_p".to_string()]);
            dialog.pick_and_pair("/hci0/dev_p");
            dialog.close();
            // The pairing runs on its own thread: its calls mix with the others.
            assert!(iterate_until(|| fake.calls().len() == 8), "calls: {:?}", fake.calls());
            let calls = fake.calls();
            let at = |r: Request| calls.iter().position(|c| *c == r).unwrap_or_else(|| panic!("missing {r:?} in {calls:?}"));
            let dev = || "/hci0/dev_p".to_string();
            assert!(at(Request::SetPairable(true)) < at(Request::StartDiscovery));
            assert!(at(Request::Pair(dev())) < at(Request::SetTrusted(dev(), true)));
            assert!(at(Request::SetTrusted(dev(), true)) < at(Request::Connect(dev())));
            // Closed while pairing: cancel it, then stop the scan and the pairable window.
            assert!(at(Request::CancelPairing(dev())) < at(Request::StopDiscovery));
            assert!(at(Request::StopDiscovery) < at(Request::SetPairable(false)));
            assert!(app.pairing.borrow().is_none());
        });
    }

    #[test]
    fn an_agent_request_shows_a_dialog_and_cancel_closes_it() {
        gtk_test::run(|| {
            let fake = Arc::new(FakeBackend::default());
            let window = gtk::Window::new();
            let (app, _root, _watch, agent_tx) = build(&window, &fake);
            let (reply_tx, reply_rx) = async_channel::bounded(1);
            agent_tx.send_blocking(AgentEvent::Request { device: "/hci0/dev_p".into(), kind: opusdm_bluetooth::AgentKind::Confirm(7), reply: reply_tx }).unwrap();
            assert!(iterate_until(|| app.agent.is_open()));
            agent_tx.send_blocking(AgentEvent::Cancel).unwrap();
            assert!(iterate_until(|| !app.agent.is_open()));
            assert_eq!(reply_rx.recv_blocking().ok(), Some(opusdm_bluetooth::AgentReply::Reject), "closing rejects");
        });
    }
}
