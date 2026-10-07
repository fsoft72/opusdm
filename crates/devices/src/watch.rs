//! A worker that follows udisks2 and reports the device list and its changes.

use crate::backend::Backend;
use crate::diff::{diff, DeviceEvent};
use crate::model::Device;
use crate::udisks::UdisksBackend;
use opusdm_watch::{drain_burst, run_with_retry, signal_ticks, Outage, COALESCE_MS};
use std::sync::Arc;
use std::time::Duration;

/// The bus name of udisks2, the sender of the signals that wake the worker.
const UDISKS_BUS_NAME: &str = "org.freedesktop.UDisks2";

/// What the worker sends.
pub enum WatchMsg {
    /// The backend to use for operations; sent once, before the first list.
    Backend(Arc<dyn Backend>),
    /// The full list, at start and after udisks2 comes back.
    Snapshot(Vec<Device>),
    Event(DeviceEvent),
    /// The bus or udisks2 is not available (sent once per outage).
    Unavailable(String),
}

/// Start the worker. Messages arrive on the returned channel until it is dropped.
pub fn spawn() -> async_channel::Receiver<WatchMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || run(tx));
    rx
}

/// Connect (retrying), then follow udisks2 until the receiver is dropped.
fn run(tx: async_channel::Sender<WatchMsg>) {
    run_with_retry(&tx, WatchMsg::Unavailable, |outage| connect_and_follow(&tx, outage));
}

/// One session: `Ok` when the receiver is gone, `Err` when udisks2 failed.
fn connect_and_follow(tx: &async_channel::Sender<WatchMsg>, outage: &mut Outage) -> Result<(), String> {
    let backend = Arc::new(UdisksBackend::connect().map_err(|e| e.text())?);
    let ticks = signal_ticks(backend.connection(), UDISKS_BUS_NAME)?;
    let mut known = backend.list().map_err(|e| e.text())?;
    let send = |m: WatchMsg| tx.send_blocking(m).is_ok();
    if !send(WatchMsg::Backend(backend.clone())) || !send(WatchMsg::Snapshot(known.clone())) { return Ok(()); }
    outage.recovered();
    while drain_burst(&ticks, Duration::from_millis(COALESCE_MS)) {
        let now = backend.list().map_err(|e| e.text())?;
        for ev in diff(&known, &now) {
            if !send(WatchMsg::Event(ev)) { return Ok(()); }
        }
        known = now;
    }
    Err("lost the connection to the system bus".to_string())
}
