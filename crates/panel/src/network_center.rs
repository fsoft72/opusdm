//! The panel side of the network tray: one shared state, the requests sent
//! to NetworkManager, and the buttons that show it. Owned by `attach`, so a
//! monitor-hotplug rebuild of the bars never loses the state.

use crate::network_model::{coalesce, IconState, Model, Request};
use crate::widgets::network::NetworkButton;
use gtk4 as gtk;
use gtk::glib;
use opusdm_network::{watch, Backend, NetError, NetState, WatchMsg};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{mpsc, Arc};

/// How long an error line stays in the popover.
const MESSAGE_SECS: u32 = 5;

/// What the worker thread reports back.
enum Done {
    /// A request failed.
    Failed(NetError),
    /// A saved network has no stored key: ask for it and connect again.
    NeedsPassword(String),
    /// A connect to this network ended, well or not.
    Connected(String),
}

/// Shared state of the network tray.
pub struct NetworkCenter {
    model: RefCell<Model>,
    /// Strong, because nothing else owns a `NetworkButton`; pruned once its
    /// widget leaves the bar (a rebuild closes the old bar window).
    buttons: RefCell<Vec<Rc<NetworkButton>>>,
    /// Feeds the worker thread; `None` while NetworkManager is not reachable.
    requests: RefCell<Option<mpsc::Sender<Request>>>,
    message: RefCell<Option<String>>,
    message_gen: Cell<u32>,
    /// The network a connect is in progress for.
    connecting: RefCell<Option<String>>,
    /// Where the worker thread reports back.
    done_tx: async_channel::Sender<Done>,
}

impl NetworkCenter {
    /// Start the watcher and the result pump.
    pub fn new() -> Rc<Self> {
        let (done_tx, done_rx) = async_channel::unbounded();
        let center = Rc::new(Self {
            model: RefCell::new(Model::default()),
            buttons: RefCell::new(Vec::new()),
            requests: RefCell::new(None),
            message: RefCell::new(None),
            message_gen: Cell::new(0),
            connecting: RefCell::new(None),
            done_tx,
        });
        let watch_rx = watch::spawn();
        {
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(msg) = watch_rx.recv().await {
                    center.on_watch(msg);
                }
            });
        }
        {
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(done) = done_rx.recv().await {
                    center.on_done(done);
                }
            });
        }
        center
    }

    /// The tray widget for one bar. It follows the shared state while it is alive.
    pub fn button(self: &Rc<Self>) -> gtk::Widget {
        let button = NetworkButton::new(self);
        self.buttons.borrow_mut().push(button.clone());
        button.refresh(self);
        button.widget()
    }

    /// Whether NetworkManager is reachable.
    pub fn is_available(&self) -> bool {
        self.model.borrow().is_available()
    }

    /// The current state.
    pub fn state(&self) -> NetState {
        self.model.borrow().state().clone()
    }

    /// The icon symbol.
    pub fn icon_state(&self) -> IconState {
        self.model.borrow().icon_state()
    }

    /// The icon tooltip.
    pub fn tooltip(&self) -> String {
        self.model.borrow().tooltip()
    }

    /// The error line to show, if any.
    pub fn message(&self) -> Option<String> {
        self.message.borrow().clone()
    }

    /// The network a connect is in progress for, if any.
    pub fn connecting(&self) -> Option<String> {
        self.connecting.borrow().clone()
    }

    /// Hand a request to the worker thread; the GTK thread never waits for it.
    pub fn send(self: &Rc<Self>, request: Request) {
        let ssid = match &request {
            Request::Connect { ssid, .. } => Some(ssid.clone()),
            _ => None,
        };
        let sent = self.requests.borrow().as_ref().is_some_and(|tx| tx.send(request).is_ok());
        if !sent {
            self.show_message("NetworkManager is not ready".to_string());
            return;
        }
        if ssid.is_some() {
            *self.connecting.borrow_mut() = ssid;
            self.notify();
        }
    }

    /// One message of the watcher: the backend or a new state.
    fn on_watch(self: &Rc<Self>, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(backend) => {
                *self.requests.borrow_mut() = Some(spawn_worker(backend, self.done_tx.clone()));
            }
            WatchMsg::Unavailable(why) => {
                opusdm_ipc::err_log!("opusdm-panel", "network tray: {why}");
                *self.requests.borrow_mut() = None;
                *self.connecting.borrow_mut() = None;
                self.model.borrow_mut().apply(WatchMsg::Unavailable(why));
            }
            other => self.model.borrow_mut().apply(other),
        }
        self.notify();
    }

    /// One report of the worker thread.
    fn on_done(self: &Rc<Self>, done: Done) {
        match done {
            Done::Failed(e) => self.show_message(e.text()),
            Done::NeedsPassword(ssid) => self.ask_password(&ssid),
            Done::Connected(ssid) => {
                if self.connecting.borrow().as_deref() == Some(ssid.as_str()) {
                    *self.connecting.borrow_mut() = None;
                }
                self.notify();
            }
        }
    }

    /// Ask for the password of a saved network from the first live button.
    fn ask_password(self: &Rc<Self>, ssid: &str) {
        let Some(ap) = self.state().aps.into_iter().find(|a| a.ssid == ssid) else {
            return self.show_message(format!("{ssid} is out of range"));
        };
        let button = self.buttons.borrow().iter().find(|b| b.is_attached()).cloned();
        match button {
            Some(button) => button.prompt_password(self, &ap.ssid, ap.security),
            None => self.show_message("A password is needed".to_string()),
        }
    }

    /// Show an error line for a few seconds (for the widgets).
    pub fn show_error(self: &Rc<Self>, text: String) {
        self.show_message(text);
    }

    /// Show an error line for a few seconds.
    fn show_message(self: &Rc<Self>, text: String) {
        *self.message.borrow_mut() = Some(text);
        let gen = self.message_gen.get() + 1;
        self.message_gen.set(gen);
        self.notify();
        let center = self.clone();
        glib::timeout_add_seconds_local_once(MESSAGE_SECS, move || {
            if center.message_gen.get() != gen {
                return;
            }
            *center.message.borrow_mut() = None;
            center.notify();
        });
    }

    /// Redraw every live button.
    fn notify(self: &Rc<Self>) {
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<NetworkButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
    }
}

/// Run requests on their own thread so a slow connect never freezes the panel.
/// Requests that queued up meanwhile are merged: a flurry of switch clicks
/// sends only the last value. Results go back to the GTK thread through `done`.
fn spawn_worker(backend: Arc<dyn Backend>, done: async_channel::Sender<Done>) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            for request in coalesce(batch) {
                if !run_request(&*backend, &request, &done) {
                    return;
                }
            }
        }
    });
    tx
}

/// Run one request and report. Returns false when the GTK side is gone.
fn run_request(backend: &dyn Backend, request: &Request, done: &async_channel::Sender<Done>) -> bool {
    let result = match request {
        Request::SetWifi(on) => backend.set_wifi_enabled(*on),
        Request::SetEth(on) => backend.set_eth_enabled(*on),
        Request::DisconnectWifi => backend.disconnect_wifi(),
        Request::Connect { ssid, password } => backend.connect(ssid, password.as_deref()),
        Request::Scan => {
            // NetworkManager refuses a scan that follows another too closely;
            // that is not worth a line in the popover.
            if let Err(e) = backend.scan() {
                opusdm_ipc::err_log!("opusdm-panel", "network tray: scan: {e}");
            }
            return true;
        }
    };
    if let Err(e) = result {
        let report = match request {
            Request::Connect { ssid, .. } if e.needs_password => Done::NeedsPassword(ssid.clone()),
            _ => Done::Failed(e),
        };
        if done.send_blocking(report).is_err() {
            return false;
        }
    }
    match request {
        Request::Connect { ssid, .. } => done.send_blocking(Done::Connected(ssid.clone())).is_ok(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_network::FakeBackend;

    /// Every report of one request, in order.
    fn reports(backend: &FakeBackend, request: &Request) -> Vec<Done> {
        let (tx, rx) = async_channel::unbounded();
        assert!(run_request(backend, request, &tx));
        drop(tx);
        let mut out = Vec::new();
        while let Ok(d) = rx.try_recv() {
            out.push(d);
        }
        out
    }

    fn connect(ssid: &str, password: Option<&str>) -> Request {
        Request::Connect { ssid: ssid.to_string(), password: password.map(str::to_string) }
    }

    #[test]
    fn a_saved_network_without_a_key_asks_for_the_password() {
        let backend = FakeBackend::default();
        *backend.fail_connect.lock().unwrap() = Some(NetError::password_needed());
        let done = reports(&backend, &connect("os3", None));
        assert!(matches!(done.as_slice(), [Done::NeedsPassword(s), Done::Connected(c)] if s == "os3" && c == "os3"));
    }

    #[test]
    fn any_other_failure_is_shown_as_an_error_line() {
        let backend = FakeBackend::default();
        *backend.fail_connect.lock().unwrap() = Some(NetError::other("Could not connect"));
        let done = reports(&backend, &connect("os3", None));
        assert!(matches!(done.as_slice(), [Done::Failed(e), Done::Connected(_)] if e.text() == "Could not connect"));
    }
}
