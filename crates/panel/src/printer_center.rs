//! The panel side of the printer tray: the state of the print queue read by
//! the CUPS watcher, the queue actions run on a worker thread, and the
//! notifications of printer errors. Owned by `attach`, so a monitor-hotplug
//! rebuild of the bars never loses the state.

use crate::transient_message::TransientMessage;
use crate::widgets::printer::PrinterButton;
use gtk4 as gtk;
use gtk::glib;
use opusdm_printing::backend::PrintBackend;
use opusdm_printing::cups::CupsBackend;
use opusdm_printing::tray::{self, Note, Notifier, TrayView};
use opusdm_printing::watch::{self, Snapshot, WatchMsg};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{mpsc, Arc};

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-panel";
/// The notifications carry this app name and icon.
const NOTIFY_APP_NAME: &str = "Printers";
const NOTIFY_ICON: &str = "printer";
/// `urgency` hint of a normal notification.
const URGENCY_NORMAL: u8 = 1;
/// Seconds between two looks at a printer that does not answer: the watcher
/// sends nothing while the state stays the same.
const OFFLINE_CHECK_SECS: u32 = 10;

/// A queue action of the popover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Cancel(i32),
    Hold(i32),
    Release(i32),
    /// Start the printer again and, for a stopped job, the job.
    Retry(String, Option<i32>),
}

/// Run `action` on `backend`; the error text when it failed.
pub fn run_action(backend: &dyn PrintBackend, action: &Action) -> Result<(), String> {
    let result = match action {
        Action::Cancel(id) => backend.cancel(*id),
        Action::Hold(id) => backend.hold(*id),
        Action::Release(id) => backend.release(*id),
        Action::Retry(printer, job) => backend.retry(printer, *job),
    };
    result.map_err(|e| e.0)
}

/// Shared state of the printer tray.
pub struct PrinterCenter {
    snapshot: RefCell<Snapshot>,
    view: RefCell<TrayView>,
    notifier: RefCell<Notifier>,
    /// Whether CUPS could be read the last time.
    available: Cell<bool>,
    /// Strong, because nothing else owns a `PrinterButton`; pruned once its
    /// widget leaves the bar (a rebuild closes the old bar window).
    buttons: RefCell<Vec<Rc<PrinterButton>>>,
    actions: mpsc::Sender<Action>,
    /// The error line shown for a few seconds.
    message: TransientMessage,
}

impl PrinterCenter {
    /// Start the CUPS watcher, the action worker and their pumps.
    pub fn new() -> Rc<Self> {
        let backend: Arc<dyn PrintBackend> = Arc::new(CupsBackend::new());
        let (watch_rx, poke) = watch::spawn(backend.clone());
        let (failed_tx, failed_rx) = async_channel::unbounded();
        let center = Rc::new(Self {
            snapshot: RefCell::new(Snapshot::default()),
            view: RefCell::new(tray::view(&Snapshot::default())),
            notifier: RefCell::new(Notifier::default()),
            available: Cell::new(false),
            buttons: RefCell::new(Vec::new()),
            actions: spawn_worker(backend, poke, failed_tx),
            message: TransientMessage::default(),
        });
        {
            // The loops hold the center strongly, like the network tray: it
            // lives as long as the panel.
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(msg) = watch_rx.recv().await {
                    center.on_watch(msg);
                }
                opusdm_ipc::err_log!(LOG_TAG, "printer tray: the CUPS watcher stopped");
            });
        }
        {
            let center = center.clone();
            glib::timeout_add_seconds_local(OFFLINE_CHECK_SECS, move || {
                center.check_time();
                glib::ControlFlow::Continue
            });
        }
        {
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(text) = failed_rx.recv().await {
                    center.show_message(text);
                }
                opusdm_ipc::err_log!(LOG_TAG, "printer tray: the action worker stopped");
            });
        }
        center
    }

    /// The tray widget for one bar. It follows the shared state while it is alive.
    pub fn button(self: &Rc<Self>) -> gtk::Widget {
        let button = PrinterButton::new(self);
        self.buttons.borrow_mut().push(button.clone());
        button.refresh(self);
        button.widget()
    }

    /// The last view of CUPS.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.borrow().clone()
    }

    /// What the icon shows; hidden while CUPS cannot be read.
    pub fn view(&self) -> TrayView {
        let mut v = self.view.borrow().clone();
        if !self.available.get() { v.look = tray::Look::Hidden; }
        v
    }

    /// The error line to show, if any.
    pub fn message(&self) -> Option<String> {
        self.message.get()
    }

    /// Hand an action to the worker thread; the GTK thread never waits for it.
    pub fn send(self: &Rc<Self>, action: Action) {
        if self.actions.send(action).is_err() {
            opusdm_ipc::err_log!(LOG_TAG, "printer tray: the action worker is gone");
            self.show_message("The printer actions stopped working".to_string());
        }
    }

    /// One message of the watcher.
    fn on_watch(self: &Rc<Self>, msg: WatchMsg) {
        match msg {
            WatchMsg::Snapshot(s) => {
                self.available.set(true);
                let notes = self.notifier.borrow_mut().update(&s, now_secs());
                for note in notes { send_note(note); }
                *self.view.borrow_mut() = tray::view(&s);
                *self.snapshot.borrow_mut() = s;
            }
            // The watcher logs the reason once.
            WatchMsg::Unavailable(_) => self.available.set(false),
        }
        self.notify();
    }

    /// Send the notes that are due by time only (a printer that has not
    /// answered for long enough); cheap when every printer answers.
    fn check_time(&self) {
        let snapshot = self.snapshot.borrow();
        if !self.available.get() || !tray::needs_timer(&snapshot) { return; }
        let notes = self.notifier.borrow_mut().update(&snapshot, now_secs());
        for note in notes { send_note(note); }
    }

    /// Show an error line for a few seconds.
    fn show_message(self: &Rc<Self>, text: String) {
        TransientMessage::show(self, |c| &c.message, Self::notify, text);
    }

    /// Redraw every live button.
    fn notify(self: &Rc<Self>) {
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<PrinterButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
    }
}

/// Seconds since the epoch.
fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Run actions on their own thread so a slow CUPS never freezes the panel.
/// After each action the watcher is asked to read CUPS again; failures go
/// back to the GTK thread as text.
fn spawn_worker(backend: Arc<dyn PrintBackend>, poke: mpsc::Sender<()>, failed: async_channel::Sender<String>) -> mpsc::Sender<Action> {
    let (tx, rx) = mpsc::channel::<Action>();
    let spawned = std::thread::Builder::new().name("opusdm-panel-printer".into()).spawn(move || {
        while let Ok(action) = rx.recv() {
            if let Err(text) = run_action(&*backend, &action) {
                opusdm_ipc::err_log!(LOG_TAG, "printer tray: {action:?}: {text}");
                if failed.send_blocking(text).is_err() { return; }
            }
            if poke.send(()).is_err() { opusdm_ipc::err_log!(LOG_TAG, "printer tray: the CUPS watcher is gone"); }
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(LOG_TAG, "printer tray: cannot start the action worker: {e}");
    }
    tx
}

/// Send `note` as a desktop notification, on a short thread; a failure is logged.
fn send_note(note: Note) {
    let spawned = std::thread::Builder::new().name("opusdm-panel-printer-note".into()).spawn(move || {
        if let Err(e) = notify(&note.summary, &note.body) {
            opusdm_ipc::err_log!(LOG_TAG, "printer tray: notification {:?} not sent: {e}", note.body);
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(LOG_TAG, "printer tray: cannot start a thread for a notification: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_printing::backend::{test_job, test_printer, FakeBackend};
    use opusdm_printing::model::PrintError;

    /// A fake with printer `p` and job 41.
    fn fake() -> FakeBackend {
        let f = FakeBackend::default();
        f.printers.lock().unwrap().push(test_printer("p"));
        f.jobs.lock().unwrap().push(test_job(41, "p"));
        f
    }

    #[test]
    fn actions_reach_the_backend() {
        let f = fake();
        assert_eq!(run_action(&f, &Action::Hold(41)), Ok(()));
        assert_eq!(run_action(&f, &Action::Release(41)), Ok(()));
        assert_eq!(run_action(&f, &Action::Retry("p".into(), Some(41))), Ok(()));
        assert_eq!(run_action(&f, &Action::Cancel(41)), Ok(()));
        assert_eq!(f.calls(), vec!["hold 41", "release 41", "retry p Some(41)", "cancel 41"]);
    }

    #[test]
    fn a_failure_is_reported_as_text() {
        let f = fake();
        *f.fail.lock().unwrap() = Some(PrintError::new("cancel: client-error-not-found"));
        assert_eq!(run_action(&f, &Action::Cancel(41)), Err("cancel: client-error-not-found".to_string()));
    }

    #[test]
    fn the_worker_reports_failures_and_pokes_the_watcher() {
        let f = Arc::new(fake());
        *f.fail_once.lock().unwrap() = Some(PrintError::new("hold: no"));
        let (poke_tx, poke_rx) = mpsc::channel();
        let (failed_tx, failed_rx) = async_channel::unbounded();
        let actions = spawn_worker(f.clone(), poke_tx, failed_tx);
        actions.send(Action::Hold(41)).unwrap();
        actions.send(Action::Cancel(41)).unwrap();
        assert_eq!(failed_rx.recv_blocking().unwrap(), "hold: no");
        poke_rx.recv().unwrap();
        poke_rx.recv().unwrap();
        assert_eq!(f.calls(), vec!["hold 41", "cancel 41"]);
    }
}
