//! The panel side of the battery tray: one shared power state, the profile
//! requests sent to power-profiles-daemon, the screen brightness, and the
//! buttons that show them. Owned by `attach`, so a monitor-hotplug rebuild of
//! the bars never loses the state.

use crate::battery_model::{coalesce, IconInfo, Model, Request};
use crate::transient_message::TransientMessage;
use crate::widgets::battery::BatteryButton;
use gtk4 as gtk;
use gtk::glib;
use opusdm_power::{watch, Backend, PowerError, PowerState, WatchMsg};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{mpsc, Arc};


/// Shared state of the battery tray.
pub struct BatteryCenter {
    model: RefCell<Model>,
    /// Strong, because nothing else owns a `BatteryButton`; pruned once its
    /// widget leaves the bar (a rebuild closes the old bar window).
    buttons: RefCell<Vec<Rc<BatteryButton>>>,
    /// Feeds the worker thread; `None` while UPower is not reachable.
    requests: RefCell<Option<mpsc::Sender<Request>>>,
    /// The error line shown for a few seconds.
    message: TransientMessage,
    /// Where the worker thread reports a failed request.
    error_tx: async_channel::Sender<PowerError>,
    /// The brightness worker; started with the first battery, so a desktop
    /// with no battery never probes `ddcutil` for this tray.
    brightness: RefCell<Option<opusdm_brightness::Controller>>,
    /// The screen brightness, once a way to change it was found.
    brightness_percent: Cell<Option<u8>>,
}

impl BatteryCenter {
    /// Start the watcher and the error pump.
    pub fn new() -> Rc<Self> {
        let (error_tx, error_rx) = async_channel::unbounded();
        let center = Rc::new(Self {
            model: RefCell::new(Model::default()),
            buttons: RefCell::new(Vec::new()),
            requests: RefCell::new(None),
            message: TransientMessage::default(),
            error_tx,
            brightness: RefCell::new(None),
            brightness_percent: Cell::new(None),
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
                    center.show_message(e.text());
                }
            });
        }
        center
    }

    /// The tray widget for one bar. It follows the shared state while it is alive.
    pub fn button(self: &Rc<Self>) -> gtk::Widget {
        let button = BatteryButton::new(self);
        self.buttons.borrow_mut().push(button.clone());
        button.refresh(self);
        button.widget()
    }

    /// Whether the tray is shown.
    pub fn is_visible(&self) -> bool {
        self.model.borrow().is_visible()
    }

    /// The current state.
    pub fn state(&self) -> PowerState {
        self.model.borrow().state().clone()
    }

    /// What the icon draws.
    pub fn icon_info(&self) -> IconInfo {
        self.model.borrow().icon_info()
    }

    /// The icon tooltip.
    pub fn tooltip(&self) -> String {
        self.model.borrow().tooltip()
    }

    /// The error line to show, if any.
    pub fn message(&self) -> Option<String> {
        self.message.get()
    }

    /// The screen brightness, or `None` while there is no way to change it.
    pub fn brightness(&self) -> Option<u8> {
        self.brightness_percent.get()
    }

    /// Ask the brightness worker for `percent`; the GTK thread never waits for it.
    pub fn set_brightness(self: &Rc<Self>, percent: u8) {
        let result = match self.brightness.borrow().as_ref() {
            Some(controller) => controller.set(percent).map_err(|e| e.to_string()),
            None => Err("The brightness cannot be changed".to_string()),
        };
        match result {
            Ok(()) => self.brightness_percent.set(Some(opusdm_brightness::clamp_percent(percent))),
            Err(e) => self.show_message(e),
        }
    }

    /// Ask the brightness worker to read the brightness again, since a key or
    /// another program may have changed it. Quiet when there is no worker.
    pub fn reread_brightness(&self) {
        let Some(controller) = self.brightness.borrow().as_ref().map(|c| c.read()) else { return };
        if let Err(e) = controller {
            opusdm_ipc::err_log!("opusdm-panel", "battery tray: brightness: {e}");
        }
    }

    /// Hand a request to the worker thread; the GTK thread never waits for it.
    pub fn send(self: &Rc<Self>, request: Request) {
        let sent = self.requests.borrow().as_ref().is_some_and(|tx| tx.send(request).is_ok());
        if !sent {
            self.show_message("The power service is not ready".to_string());
        }
    }

    /// One message of the watcher: the backend or a new state.
    fn on_watch(self: &Rc<Self>, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(backend) => {
                *self.requests.borrow_mut() = Some(spawn_worker(backend, self.error_tx.clone()));
            }
            WatchMsg::Unavailable(why) => {
                opusdm_ipc::err_log!("opusdm-panel", "battery tray: {why}");
                *self.requests.borrow_mut() = None;
                self.model.borrow_mut().apply(WatchMsg::Unavailable(why));
            }
            other => self.model.borrow_mut().apply(other),
        }
        if self.is_visible() {
            self.start_brightness();
        }
        self.notify();
    }

    /// Start the brightness worker once; its events fill `brightness_percent`.
    fn start_brightness(self: &Rc<Self>) {
        if self.brightness.borrow().is_some() { return; }
        let (controller, events) = opusdm_brightness::spawn(opusdm_brightness::detect);
        *self.brightness.borrow_mut() = Some(controller);
        let center = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                match event {
                    opusdm_brightness::Event::Ready { percent, .. } | opusdm_brightness::Event::Current(percent) => {
                        center.brightness_percent.set(Some(opusdm_brightness::clamp_percent(percent)));
                        center.notify();
                    }
                    opusdm_brightness::Event::Unavailable(why) => {
                        opusdm_ipc::err_log!("opusdm-panel", "battery tray: brightness: {why}");
                    }
                    opusdm_brightness::Event::Applied(_) => {}
                    opusdm_brightness::Event::Failed(why) => center.show_message(why),
                }
            }
        });
    }

    /// Show an error line for a few seconds.
    fn show_message(self: &Rc<Self>, text: String) {
        TransientMessage::show(self, |c| &c.message, Self::notify, text);
    }

    /// Redraw every live button.
    fn notify(self: &Rc<Self>) {
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<BatteryButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
    }
}

/// Run requests on their own thread so a slow daemon never freezes the panel.
/// Requests that queued up meanwhile are merged: a flurry of profile clicks
/// sends only the last one. Errors go back to the GTK thread through `errors`.
fn spawn_worker(backend: Arc<dyn Backend>, errors: async_channel::Sender<PowerError>) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            for request in coalesce(batch) {
                let Err(e) = execute(&*backend, request) else { continue };
                if errors.send_blocking(e).is_err() {
                    return;
                }
            }
        }
    });
    tx
}

/// Run one request on the backend.
fn execute(backend: &dyn Backend, request: Request) -> Result<(), PowerError> {
    match request {
        Request::SetProfile(profile) => backend.set_profile(profile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_power::{FakeBackend, Profile};

    #[test]
    fn a_profile_request_reaches_the_backend() {
        let backend = FakeBackend::default();
        execute(&backend, Request::SetProfile(Profile::PowerSaver)).unwrap();
        assert_eq!(backend.calls(), vec!["profile power-saver"]);
    }
}
