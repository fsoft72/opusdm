//! The panel side of the Bluetooth tray: the state read by the BlueZ
//! watcher, the requests run on a worker thread, the notifications, and the
//! default pairing agent of the session (its dialogs pop up here, so a phone
//! can pair to the PC while the adapter is visible). Owned by `attach`, so a
//! monitor-hotplug rebuild of the bars never loses the state.

use crate::bluetooth_model::{self, Look, Note, Notifier};
use crate::transient_message::TransientMessage;
use crate::widgets::bluetooth::BluetoothButton;
use gtk4 as gtk;
use gtk::glib;
use opusdm_bluetooth::agent::PANEL_AGENT_PATH;
use opusdm_bluetooth::{error, watch, AgentSetup, BtState, Request, WatchMsg};
use opusdm_bluetooth_config::agent_dialog::AgentHost;
use opusdm_bluetooth_config::worker::{self, Done, Job};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-panel";
/// The notifications carry this app name and icon.
const NOTIFY_APP_NAME: &str = "Bluetooth";
const NOTIFY_ICON: &str = "bluetooth-active-symbolic";
/// `urgency` hint of a normal notification.
const URGENCY_NORMAL: u8 = 1;

/// Shared state of the Bluetooth tray.
pub struct BluetoothCenter {
    state: RefCell<BtState>,
    /// Whether BlueZ answered the last time.
    available: Cell<bool>,
    notifier: RefCell<Notifier>,
    /// Strong, because nothing else owns a `BluetoothButton`; pruned once its
    /// widget leaves the bar (a rebuild closes the old bar window).
    buttons: RefCell<Vec<Rc<BluetoothButton>>>,
    /// Feeds the worker; `None` while BlueZ is not reachable.
    jobs: RefCell<Option<mpsc::Sender<Job>>>,
    done_tx: async_channel::Sender<Done>,
    /// The pairing dialogs of the default agent.
    agent: Rc<AgentHost>,
    /// The error line shown for a few seconds.
    message: TransientMessage,
}

impl BluetoothCenter {
    /// Start the watcher (which serves the default agent) and the pumps.
    pub fn new() -> Rc<Self> {
        let (done_tx, done_rx) = async_channel::unbounded();
        let (events, agent_rx) = async_channel::unbounded();
        let watch_rx = watch::spawn(Some(AgentSetup { path: PANEL_AGENT_PATH, default: true, events }));
        let center = Rc::new(Self {
            state: RefCell::new(BtState::default()),
            available: Cell::new(false),
            notifier: RefCell::new(Notifier::default()),
            buttons: RefCell::new(Vec::new()),
            jobs: RefCell::new(None),
            done_tx,
            agent: AgentHost::new(None),
            message: TransientMessage::default(),
        });
        {
            // The loops hold the center strongly, like the network and
            // printer trays: it lives as long as the panel.
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(msg) = watch_rx.recv().await {
                    center.on_watch(msg);
                }
                opusdm_ipc::err_log!(LOG_TAG, "bluetooth tray: the BlueZ watcher stopped");
            });
        }
        {
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(done) = done_rx.recv().await {
                    center.on_done(done);
                }
                opusdm_ipc::err_log!(LOG_TAG, "bluetooth tray: the worker reports stopped");
            });
        }
        {
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(event) = agent_rx.recv().await {
                    let state = center.state.borrow().clone();
                    center.agent.on_event(&state, event);
                }
                opusdm_ipc::err_log!(LOG_TAG, "bluetooth tray: the pairing agent stopped");
            });
        }
        center
    }

    /// The tray widget for one bar. It follows the shared state while it is alive.
    pub fn button(self: &Rc<Self>) -> gtk::Widget {
        let button = BluetoothButton::new(self);
        self.buttons.borrow_mut().push(button.clone());
        button.refresh(self);
        button.widget()
    }

    /// The last state of BlueZ.
    pub fn state(&self) -> BtState {
        self.state.borrow().clone()
    }

    /// What the icon shows.
    pub fn look(&self) -> Look {
        bluetooth_model::look(self.available.get(), &self.state.borrow())
    }

    /// The error line to show, if any.
    pub fn message(&self) -> Option<String> {
        self.message.get()
    }

    /// Hand a request to the worker thread; the GTK thread never waits for it.
    pub fn send(self: &Rc<Self>, request: Request) {
        let sent = self.jobs.borrow().as_ref().is_some_and(|tx| tx.send(Job::Run(request.clone())).is_ok());
        if sent { return; }
        opusdm_ipc::err_log!(LOG_TAG, "bluetooth tray: {request:?} not sent: BlueZ is not reachable");
        self.show_message("The Bluetooth service is not reachable".to_string());
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
                let notes = self.notifier.borrow_mut().update(&s);
                for note in notes { send_note(note); }
                self.agent.on_state(&s);
                *self.state.borrow_mut() = s;
            }
            // The watcher logs the reason once.
            WatchMsg::Unavailable(_) => {
                self.available.set(false);
                *self.jobs.borrow_mut() = None;
                self.agent.close();
            }
        }
        self.notify();
    }

    /// One report of the worker: a failed request shows its error.
    fn on_done(self: &Rc<Self>, done: Done) {
        match done {
            Done::Failed(_, e) => self.show_message(error::text(&e)),
            // The tray never pairs: "Add device..." opens the manager.
            Done::Paired(device, _) => opusdm_ipc::err_log!(LOG_TAG, "bluetooth tray: unexpected pairing report for {device}"),
        }
    }

    /// Show an error line for a few seconds.
    fn show_message(self: &Rc<Self>, text: String) {
        TransientMessage::show(self, |c| &c.message, Self::notify, text);
    }

    /// Redraw every live button.
    fn notify(self: &Rc<Self>) {
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<BluetoothButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
    }
}

/// Send `note` as a desktop notification, on a short thread; a failure is logged.
fn send_note(note: Note) {
    let spawned = std::thread::Builder::new().name("opusdm-panel-bluetooth-note".into()).spawn(move || {
        if let Err(e) = notify(&note.summary, &note.body) {
            opusdm_ipc::err_log!(LOG_TAG, "bluetooth tray: notification {:?} not sent: {e}", note.body);
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(LOG_TAG, "bluetooth tray: cannot start a thread for a notification: {e}");
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
