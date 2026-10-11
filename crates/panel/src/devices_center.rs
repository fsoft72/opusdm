//! The panel side of the device notifier: one shared list, the operations on
//! it, and the widgets that show it. Owned by `attach`, so a monitor-hotplug
//! rebuild of the bars never loses the state.

use crate::devices_model::{Model, Row};
use crate::transient_message::TransientMessage;
use crate::device_popup;
use crate::widgets::devices::DeviceButton;
use gtk4 as gtk;
use gtk::glib;
use opusdm_devices::{ops, watch, Backend, Device, DeviceError, WatchMsg};
use opusdm_ipc::PanelPosition;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;


/// What the user asked for; the payload is the object path of the volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Mount(String),
    MountAndOpen(String),
    Unmount(String),
    Eject(String),
}

impl Op {
    /// The volume the operation is about.
    fn path(&self) -> &str {
        match self {
            Op::Mount(p) | Op::MountAndOpen(p) | Op::Unmount(p) | Op::Eject(p) => p,
        }
    }
}

/// The outcome of an operation, sent back from the worker thread.
struct OpResult {
    op: Op,
    result: Result<Option<PathBuf>, DeviceError>,
}

/// Shared state of the device notifier.
pub struct DeviceCenter {
    model: RefCell<Model>,
    backend: RefCell<Option<Arc<dyn Backend>>>,
    /// Strong, because nothing else owns a `DeviceButton`; pruned once its
    /// widget leaves the bar (a rebuild closes the old bar window).
    buttons: RefCell<Vec<Rc<DeviceButton>>>,
    on_open: Rc<dyn Fn(&Path)>,
    position: Rc<Cell<PanelPosition>>,
    result_tx: async_channel::Sender<OpResult>,
    /// The error line shown for a few seconds.
    message: TransientMessage,
}

impl DeviceCenter {
    /// Start the watcher and the result pump. `on_open` opens a folder (the
    /// hub starts a Lister); `position` is the live edge of the panel.
    pub fn new(on_open: Rc<dyn Fn(&Path)>, position: Rc<Cell<PanelPosition>>) -> Rc<Self> {
        let (result_tx, result_rx) = async_channel::unbounded();
        let center = Rc::new(Self {
            model: RefCell::new(Model::default()),
            backend: RefCell::new(None),
            buttons: RefCell::new(Vec::new()),
            on_open,
            position,
            result_tx,
            message: TransientMessage::default(),
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
                while let Ok(res) = result_rx.recv().await {
                    center.on_result(res);
                }
            });
        }
        center
    }

    /// The drop-down widget for one bar. It follows the shared list for as
    /// long as it is alive.
    pub fn button(self: &Rc<Self>) -> gtk::Widget {
        let button = DeviceButton::new(self);
        self.buttons.borrow_mut().push(button.clone());
        button.refresh(self);
        button.widget()
    }

    /// The rows of the drop-down.
    pub fn rows(&self) -> Vec<Row> {
        self.model.borrow().rows()
    }

    /// Whether there is no device to show.
    pub fn is_empty(&self) -> bool {
        self.model.borrow().is_empty()
    }

    /// The error line to show, if any.
    pub fn message(&self) -> Option<String> {
        self.message.get()
    }

    /// The folder opener the hub gave us.
    pub fn on_open(&self) -> Rc<dyn Fn(&Path)> {
        self.on_open.clone()
    }

    /// The live panel edge, for placing the popup.
    pub fn position(&self) -> PanelPosition {
        self.position.get()
    }

    /// Whether the volume `path` is listed and not mounted.
    pub fn is_pending(&self, path: &str) -> bool {
        self.model.borrow().find(path).is_some_and(|d| d.mount_point.is_none())
    }

    /// Run `op` on a worker thread; the result comes back to `on_result`.
    pub fn run(self: &Rc<Self>, op: Op) {
        let Some(backend) = self.backend.borrow().clone() else {
            self.show_message("The disk service is not ready".to_string());
            return;
        };
        let (device, all) = {
            let m = self.model.borrow();
            (m.find(op.path()).cloned(), m.devices().to_vec())
        };
        let Some(device) = device else { return };
        self.set_busy(&device, &all, true);
        self.notify();
        let tx = self.result_tx.clone();
        std::thread::spawn(move || {
            let result = match &op {
                Op::Mount(_) | Op::MountAndOpen(_) => backend.mount(&device.path).map(Some),
                Op::Unmount(_) => backend.unmount(&device.path).map(|_| None),
                Op::Eject(_) => ops::eject(&*backend, &device, &all).map(|_| None),
            };
            let _ = tx.send_blocking(OpResult { op, result });
        });
    }

    /// Mark the volumes an operation touches: all of the drive for an eject.
    fn set_busy(&self, device: &Device, all: &[Device], busy: bool) {
        let mut m = self.model.borrow_mut();
        for d in all.iter().filter(|d| d.drive_path == device.drive_path) {
            m.set_busy(&d.path, busy);
        }
    }

    /// One message of the watcher: the backend, a new list or a change.
    fn on_watch(self: &Rc<Self>, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(b) => *self.backend.borrow_mut() = Some(b),
            WatchMsg::Unavailable(why) => opusdm_ipc::err_log!("opusdm-panel", "device notifier: {why}"),
            other => self.model.borrow_mut().apply(other),
        }
        let new = self.model.borrow_mut().take_new();
        self.notify();
        for d in new {
            device_popup::show(self, &d);
        }
    }

    /// The end of an operation.
    fn on_result(self: &Rc<Self>, res: OpResult) {
        let device = self.model.borrow().find(res.op.path()).cloned();
        if let Some(device) = &device {
            let all = self.model.borrow().devices().to_vec();
            self.set_busy(device, &all, false);
        }
        match res.result {
            Ok(Some(point)) if matches!(res.op, Op::MountAndOpen(_)) => (self.on_open)(&point),
            Ok(_) => {}
            Err(e) if e.is_already_done() => {}
            // The device was unplugged in the meantime: the failure is expected.
            Err(_) if device.is_none() => {}
            Err(e) => self.show_message(e.text()),
        }
        self.notify();
    }

    /// Show an error line for a few seconds.
    fn show_message(self: &Rc<Self>, text: String) {
        TransientMessage::show(self, |c| &c.message, Self::notify, text);
    }

    /// Redraw every live button and drop popups of devices that are gone or mounted.
    fn notify(self: &Rc<Self>) {
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<DeviceButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
        device_popup::sync(self);
    }
}
