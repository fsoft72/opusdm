//! A worker that follows NetworkManager and reports the network state.

use crate::backend::Backend;
use crate::model::NetState;
use crate::nm::NmBackend;
use opusdm_watch::{drain_burst, run_with_retry, signal_ticks, Outage, COALESCE_MS};
use std::sync::Arc;
use std::time::Duration;

/// The bus name of NetworkManager, the sender of the signals that wake the worker.
const NM_BUS_NAME: &str = "org.freedesktop.NetworkManager";

/// What the worker sends.
pub enum WatchMsg {
    /// The backend to use for operations; sent once per session, before the first state.
    Backend(Arc<dyn Backend>),
    /// The state, at start and after every change.
    State(NetState),
    /// The bus or NetworkManager is not available (sent once per outage).
    Unavailable(String),
}

/// Start the worker. Messages arrive on the returned channel until it is dropped.
pub fn spawn() -> async_channel::Receiver<WatchMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || run(tx));
    rx
}

/// Connect (retrying), then follow NetworkManager until the receiver is dropped.
fn run(tx: async_channel::Sender<WatchMsg>) {
    run_with_retry(&tx, WatchMsg::Unavailable, |outage| connect_and_follow(&tx, outage));
}

/// One session: `Ok` when the receiver is gone, `Err` when NetworkManager failed.
fn connect_and_follow(tx: &async_channel::Sender<WatchMsg>, outage: &mut Outage) -> Result<(), String> {
    let backend = Arc::new(NmBackend::connect().map_err(|e| e.text())?);
    let ticks = signal_ticks(backend.connection(), NM_BUS_NAME)?;
    let mut known = backend.state().map_err(|e| e.text())?;
    let send = |m: WatchMsg| tx.send_blocking(m).is_ok();
    if !send(WatchMsg::Backend(backend.clone())) || !send(WatchMsg::State(known.clone())) { return Ok(()); }
    outage.recovered();
    while drain_burst(&ticks, Duration::from_millis(COALESCE_MS)) {
        let now = backend.state().map_err(|e| e.text())?;
        // Signal strength changes alone fire many signals: only a real change is sent.
        if now == known { continue; }
        if !send(WatchMsg::State(now.clone())) { return Ok(()); }
        known = now;
    }
    Err("lost the connection to the system bus".to_string())
}
