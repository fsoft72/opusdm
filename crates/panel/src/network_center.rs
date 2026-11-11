//! The panel side of the network tray: one shared state, the requests sent
//! to NetworkManager, and the buttons that show it. Owned by `attach`, so a
//! monitor-hotplug rebuild of the bars never loses the state.

use crate::network_model::{coalesce, Attempt, IconState, Model, Outcome, Request};
use crate::transient_message::TransientMessage;
use crate::widgets::network::NetworkButton;
use gtk4 as gtk;
use gtk::glib;
use opusdm_network::{watch, Backend, ConnectStep, NetError, NetState, WatchMsg};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-panel";
/// The status section keeps a connect that worked for this long.
const SHOW_CONNECTED_SECS: u32 = 5;
/// The status section keeps a connect that failed for this long.
const SHOW_FAILED_SECS: u32 = 20;
/// A connect with no end after this long is given up: the backend waits
/// 50 s at most, so the worker must have lost it.
const ATTEMPT_LIMIT: Duration = Duration::from_secs(90);
/// The notifications carry this app name and icon.
const NOTIFY_APP_NAME: &str = "Network";
const NOTIFY_ICON: &str = "network-wireless-symbolic";
/// `urgency` hint of a normal notification.
const URGENCY_NORMAL: u8 = 1;

/// What the worker thread reports back.
enum Done {
    /// A request other than a connect failed.
    Failed(NetError),
    /// A connect to this network reached a step.
    Step(String, ConnectStep),
    /// A connect to this network ended, well or not.
    ConnectEnded(String, Result<(), NetError>),
}

/// Shared state of the network tray.
pub struct NetworkCenter {
    model: RefCell<Model>,
    /// Strong, because nothing else owns a `NetworkButton`; pruned once its
    /// widget leaves the bar (a rebuild closes the old bar window).
    buttons: RefCell<Vec<Rc<NetworkButton>>>,
    /// Feeds the worker thread; `None` while NetworkManager is not reachable.
    requests: RefCell<Option<mpsc::Sender<Request>>>,
    /// The error line shown for a few seconds.
    message: TransientMessage,
    /// The connect in progress, or the last one for a while after its end:
    /// what the status section shows.
    attempt: RefCell<Option<Attempt>>,
    /// Tells the end of the current attempt from an older one, for the
    /// timer that hides the status section.
    attempt_generation: Cell<u32>,
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
            message: TransientMessage::default(),
            attempt: RefCell::new(None),
            attempt_generation: Cell::new(0),
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

    /// Whether a VPN is up.
    pub fn vpn_active(&self) -> bool {
        self.model.borrow().vpn_active()
    }

    /// The icon tooltip, with the connect in progress or just ended.
    pub fn tooltip(&self) -> String {
        let link = self.model.borrow().tooltip();
        match self.attempt.borrow().as_ref() {
            Some(a) => format!("{}\n{link}", a.summary(Instant::now())),
            None => link,
        }
    }

    /// The error line to show, if any.
    pub fn message(&self) -> Option<String> {
        self.message.get()
    }

    /// The network a connect is in progress for, if any.
    pub fn connecting(&self) -> Option<String> {
        self.attempt.borrow().as_ref().filter(|a| a.is_running()).map(|a| a.ssid.clone())
    }

    /// The connect the status section shows, if any.
    pub fn attempt(&self) -> Option<Attempt> {
        self.attempt.borrow().clone()
    }

    /// Whether the icon spins: a connect of the tray, or a device that is
    /// negotiating (also a connect started elsewhere, for example `nmcli`).
    pub fn is_busy(&self) -> bool {
        let state = self.model.borrow();
        self.connecting().is_some() || state.state().wifi_busy || state.state().eth_busy
    }

    /// Hand a request to the worker thread; the GTK thread never waits for it.
    pub fn send(self: &Rc<Self>, request: Request) {
        let ssid = match &request {
            Request::Connect { ssid, .. } => Some(ssid.clone()),
            _ => None,
        };
        let sent = self.requests.borrow().as_ref().is_some_and(|tx| tx.send(request).is_ok());
        if !sent {
            opusdm_ipc::err_log!(LOG_TAG, "network tray: request not sent, NetworkManager is not ready");
            self.show_message("NetworkManager is not ready".to_string());
            return;
        }
        if let Some(ssid) = ssid {
            self.start_attempt(&ssid);
        }
    }

    /// Show a new connect in the status section and keep its time running.
    fn start_attempt(self: &Rc<Self>, ssid: &str) {
        *self.attempt.borrow_mut() = Some(Attempt::new(ssid, Instant::now()));
        self.attempt_generation.set(self.attempt_generation.get().wrapping_add(1));
        self.notify();
        // Owned by the timer, which stops once no connect is running; the
        // center itself lives for the whole session (see `attach`).
        let center = self.clone();
        glib::timeout_add_seconds_local(1, move || {
            if !center.tick_attempt() { return glib::ControlFlow::Break; }
            glib::ControlFlow::Continue
        });
    }

    /// One second of a running connect: update the time shown, and give up a
    /// connect the worker never ended. Returns false once none is running.
    fn tick_attempt(self: &Rc<Self>) -> bool {
        let overdue = {
            let attempt = self.attempt.borrow();
            let Some(a) = attempt.as_ref().filter(|a| a.is_running()) else { return false };
            a.is_overdue(ATTEMPT_LIMIT, Instant::now()).then(|| a.ssid.clone())
        };
        if let Some(ssid) = overdue {
            opusdm_ipc::err_log!(LOG_TAG, "network tray: connect to {ssid} had no answer in {} s, given up", ATTEMPT_LIMIT.as_secs());
            self.end_attempt(&ssid, Err("No answer from NetworkManager".to_string()));
            return false;
        }
        self.refresh_status();
        true
    }

    /// The connect to `ssid` ended: show how, then hide the status section
    /// after a while unless a newer connect replaced it. The end of an older
    /// connect is ignored.
    fn end_attempt(self: &Rc<Self>, ssid: &str, result: Result<(), String>) {
        {
            let mut attempt = self.attempt.borrow_mut();
            let Some(a) = attempt.as_mut().filter(|a| a.ssid == ssid && a.is_running()) else { return };
            a.finish(result, Instant::now());
        }
        let failed = matches!(self.attempt.borrow().as_ref().map(|a| &a.outcome), Some(Outcome::Failed(_)));
        let generation = self.attempt_generation.get();
        let center = self.clone();
        glib::timeout_add_seconds_local_once(if failed { SHOW_FAILED_SECS } else { SHOW_CONNECTED_SECS }, move || {
            if center.attempt_generation.get() != generation { return; }
            if center.attempt.borrow().as_ref().is_some_and(|a| a.is_running()) { return; }
            *center.attempt.borrow_mut() = None;
            center.notify();
        });
        self.notify();
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
                if let Some(ssid) = self.connecting() {
                    self.end_attempt(&ssid, Err("NetworkManager is not available".to_string()));
                }
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
            Done::Step(ssid, step) => {
                let changed = {
                    let mut attempt = self.attempt.borrow_mut();
                    match attempt.as_mut().filter(|a| a.ssid == ssid) {
                        Some(a) => {
                            a.step(step);
                            true
                        }
                        None => false,
                    }
                };
                if changed { self.refresh_status(); }
            }
            Done::ConnectEnded(ssid, Ok(())) => self.end_attempt(&ssid, Ok(())),
            Done::ConnectEnded(ssid, Err(e)) => self.on_connect_failed(&ssid, e),
        }
    }

    /// A connect failed: say why in the status section and in a
    /// notification, and ask for the password again when it is the problem.
    fn on_connect_failed(self: &Rc<Self>, ssid: &str, e: NetError) {
        let why = e.text();
        opusdm_ipc::err_log!(LOG_TAG, "network tray: connect to {ssid} failed: {why}");
        if e.needs_password && !e.password_refused {
            // A saved network with no key yet: not a failure, just ask.
            let ours = self.attempt.borrow().as_ref().is_some_and(|a| a.ssid == ssid);
            if ours {
                *self.attempt.borrow_mut() = None;
                self.notify();
            }
            self.ask_password(ssid, None);
            return;
        }
        self.end_attempt(ssid, Err(why.clone()));
        send_note(format!("Could not connect to {ssid}"), why.clone());
        if e.password_refused {
            self.ask_password(ssid, Some(&why));
        }
    }

    /// Ask for the password of `ssid` from the first live button; `error`
    /// says why it is asked again.
    fn ask_password(self: &Rc<Self>, ssid: &str, error: Option<&str>) {
        let Some(ap) = self.state().aps.into_iter().find(|a| a.ssid == ssid) else {
            opusdm_ipc::err_log!(LOG_TAG, "network tray: cannot ask the password of {ssid}, out of range");
            return self.show_message(format!("{ssid} is out of range"));
        };
        let button = self.buttons.borrow().iter().find(|b| b.is_attached()).cloned();
        match button {
            Some(button) => button.prompt_password(self, &ap.ssid, ap.security, error),
            None => {
                opusdm_ipc::err_log!(LOG_TAG, "network tray: no bar to ask the password of {ssid}");
                self.show_message("A password is needed".to_string());
            }
        }
    }

    /// Show an error line for a few seconds (for the widgets).
    pub fn show_error(self: &Rc<Self>, text: String) {
        self.show_message(text);
    }

    /// Show an error line for a few seconds.
    fn show_message(self: &Rc<Self>, text: String) {
        TransientMessage::show(self, |c| &c.message, Self::notify, text);
    }

    /// Redraw every live button.
    fn notify(self: &Rc<Self>) {
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<NetworkButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
    }

    /// Redraw only the status section, the icon and the tooltip of every
    /// live button: the network list is not rebuilt under the pointer.
    fn refresh_status(self: &Rc<Self>) {
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<NetworkButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh_status(self);
        }
    }
}

/// Send a desktop notification, on a short thread; a failure is logged.
fn send_note(summary: String, body: String) {
    let spawned = std::thread::Builder::new().name("opusdm-panel-network-note".into()).spawn(move || {
        if let Err(e) = notify(&summary, &body) {
            opusdm_ipc::err_log!(LOG_TAG, "network tray: notification {summary:?} not sent: {e}");
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(LOG_TAG, "network tray: cannot start a thread for a notification: {e}");
    }
}

/// `org.freedesktop.Notifications.Notify` with a normal urgency.
fn notify(summary: &str, body: &str) -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    let actions: Vec<&str> = Vec::new();
    let mut hints: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
    hints.insert("urgency", zbus::zvariant::Value::U8(URGENCY_NORMAL));
    let args = (NOTIFY_APP_NAME, 0u32, NOTIFY_ICON, summary, body, actions, hints, -1i32);
    conn.call_method(Some("org.freedesktop.Notifications"), "/org/freedesktop/Notifications", Some("org.freedesktop.Notifications"), "Notify", &args)?;
    Ok(())
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
        Request::Connect { ssid, password } => {
            // A step that cannot be reported is only a lost line on screen;
            // the end below finds out whether the GTK side is gone.
            let progress = |step: ConnectStep| {
                let _ = done.send_blocking(Done::Step(ssid.clone(), step));
            };
            let result = backend.connect(ssid, password.as_deref(), &progress);
            return done.send_blocking(Done::ConnectEnded(ssid.clone(), result)).is_ok();
        }
        Request::VpnUp { uuid, password } => backend.activate_vpn(uuid, password.as_deref()),
        Request::VpnDown(uuid) => backend.deactivate_vpn(uuid),
        Request::Scan => {
            // NetworkManager refuses a scan that follows another too closely;
            // that is not worth a line in the popover.
            if let Err(e) = backend.scan() {
                opusdm_ipc::err_log!("opusdm-panel", "network tray: scan: {e}");
            }
            return true;
        }
    };
    match result {
        Ok(()) => true,
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "network tray: {request:?} failed: {}", e.text());
            done.send_blocking(Done::Failed(e)).is_ok()
        }
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
        assert!(matches!(done.as_slice(), [Done::ConnectEnded(s, Err(e))] if s == "os3" && e.needs_password && !e.password_refused));
    }

    #[test]
    fn the_steps_of_a_connect_come_before_its_end() {
        let backend = FakeBackend::default();
        *backend.connect_steps.lock().unwrap() = vec![ConnectStep::Searching, ConnectStep::Authenticating];
        let done = reports(&backend, &connect("os3", Some("pw")));
        assert!(matches!(
            done.as_slice(),
            [Done::Step(a, ConnectStep::Searching), Done::Step(b, ConnectStep::Authenticating), Done::ConnectEnded(c, Ok(()))] if a == "os3" && b == "os3" && c == "os3"
        ));
    }

    #[test]
    fn a_refused_password_ends_the_connect_with_the_reason() {
        let backend = FakeBackend::default();
        *backend.fail_connect.lock().unwrap() = Some(NetError::password_refused("The password was refused"));
        let done = reports(&backend, &connect("os3", Some("bad")));
        assert!(matches!(done.as_slice(), [Done::ConnectEnded(_, Err(e))] if e.password_refused && e.text() == "The password was refused"));
    }

    #[test]
    fn a_vpn_request_reaches_the_backend_and_its_failure_is_shown() {
        let backend = FakeBackend::default();
        assert!(reports(&backend, &Request::VpnUp { uuid: "u".into(), password: Some("pw".into()) }).is_empty());
        assert!(reports(&backend, &Request::VpnDown("u".into())).is_empty());
        assert_eq!(backend.calls(), vec!["vpn up u password=true".to_string(), "vpn down u".to_string()]);
        *backend.fail_connect.lock().unwrap() = Some(NetError::other("Could not connect"));
        let done = reports(&backend, &Request::VpnUp { uuid: "u".into(), password: None });
        assert!(matches!(done.as_slice(), [Done::Failed(e)] if e.text() == "Could not connect"));
    }

    #[test]
    fn any_other_failure_is_shown_as_an_error_line() {
        let backend = FakeBackend::default();
        *backend.fail_connect.lock().unwrap() = Some(NetError::other("Could not connect"));
        let done = reports(&backend, &connect("os3", None));
        assert!(matches!(done.as_slice(), [Done::ConnectEnded(_, Err(e))] if e.text() == "Could not connect"));
    }
}
