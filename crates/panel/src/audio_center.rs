//! The panel side of the audio tray: one shared device list, the requests
//! sent to the sound server, and the buttons that show it. Owned by `attach`,
//! so a monitor-hotplug rebuild of the bars never loses the state.

use crate::audio_model::{coalesce, should_show, IconLevel, Model, Request};
use crate::widgets::audio::AudioButton;
use gtk4 as gtk;
use gtk::glib;
use opusdm_audio::{watch, AudioDevice, AudioError, Backend, Kind, WatchMsg};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{mpsc, Arc};

/// How long an error line stays in the popover.
const MESSAGE_SECS: u32 = 5;

/// Shared state of the audio tray.
pub struct AudioCenter {
    model: RefCell<Model>,
    /// Strong, because nothing else owns an `AudioButton`; pruned once its
    /// widget leaves the bar (a rebuild closes the old bar window).
    buttons: RefCell<Vec<Rc<AudioButton>>>,
    /// Feeds the worker thread; `None` while the server is not reachable.
    requests: RefCell<Option<mpsc::Sender<Request>>>,
    message: RefCell<Option<String>>,
    message_gen: Cell<u32>,
    /// Where the worker thread reports a failed request.
    error_tx: async_channel::Sender<AudioError>,
}

impl AudioCenter {
    /// Start the watcher and the error pump.
    pub fn new() -> Rc<Self> {
        let (error_tx, error_rx) = async_channel::unbounded();
        let center = Rc::new(Self {
            model: RefCell::new(Model::default()),
            buttons: RefCell::new(Vec::new()),
            requests: RefCell::new(None),
            message: RefCell::new(None),
            message_gen: Cell::new(0),
            error_tx,
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
                while let Ok(e) = error_rx.recv().await {
                    if should_show(&e) {
                        center.show_message(e.text());
                    }
                }
            });
        }
        center
    }

    /// The tray widget for one bar. It follows the shared list while it is alive.
    pub fn button(self: &Rc<Self>) -> gtk::Widget {
        let button = AudioButton::new(self);
        self.buttons.borrow_mut().push(button.clone());
        button.refresh(self);
        button.widget()
    }

    /// Whether the sound server is reachable.
    pub fn is_available(&self) -> bool {
        self.model.borrow().is_available()
    }

    /// The devices of one kind.
    pub fn devices_of(&self, kind: Kind) -> Vec<AudioDevice> {
        self.model.borrow().devices_of(kind)
    }

    /// The icon symbol.
    pub fn icon_level(&self) -> IconLevel {
        self.model.borrow().icon_level()
    }

    /// The icon tooltip.
    pub fn tooltip(&self) -> String {
        self.model.borrow().tooltip()
    }

    /// The error line to show, if any.
    pub fn message(&self) -> Option<String> {
        self.message.borrow().clone()
    }

    /// Hand a request to the worker thread; the GTK thread never waits for it.
    pub fn send(self: &Rc<Self>, request: Request) {
        let sent = self.requests.borrow().as_ref().is_some_and(|tx| tx.send(request).is_ok());
        if !sent {
            self.show_message("The sound server is not ready".to_string());
        }
    }

    /// One message of the watcher: the backend, a new list or a change.
    fn on_watch(self: &Rc<Self>, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(backend) => {
                *self.requests.borrow_mut() = Some(spawn_worker(backend, self.error_tx.clone()));
            }
            WatchMsg::Unavailable(why) => {
                opusdm_ipc::err_log!("opusdm-panel", "audio tray: {why}");
                *self.requests.borrow_mut() = None;
                self.model.borrow_mut().apply(WatchMsg::Unavailable(why));
            }
            other => self.model.borrow_mut().apply(other),
        }
        self.notify();
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
        let live: Vec<Rc<AudioButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
    }
}

/// Run requests on their own thread so a slow server never freezes the panel.
/// Requests that queued up meanwhile are merged: a drag sends only its last
/// volume. Errors go back to the GTK thread through `errors`.
fn spawn_worker(backend: Arc<dyn Backend>, errors: async_channel::Sender<AudioError>) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            for request in coalesce(batch) {
                let Err(e) = execute(&*backend, &request) else { continue };
                if errors.send_blocking(e).is_err() {
                    return;
                }
            }
        }
    });
    tx
}

/// Run one request on the backend.
fn execute(backend: &dyn Backend, request: &Request) -> Result<(), AudioError> {
    match request {
        Request::SetVolume { kind, index, percent } => backend.set_volume(*kind, *index, *percent),
        Request::SetMute { kind, index, muted } => backend.set_mute(*kind, *index, *muted),
        Request::SetDefault { kind, name } => backend.set_default(*kind, name),
    }
}
