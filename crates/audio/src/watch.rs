//! A worker that follows the sound server and reports the device list and its changes.

use crate::backend::Backend;
use crate::diff::AudioEvent;
use crate::model::AudioDevice;
use crate::pulse::Session;
use opusdm_watch::{run_with_retry, Outage};
use std::sync::Arc;

/// What the worker sends.
pub enum WatchMsg {
    /// The backend to use for operations; sent before every snapshot.
    Backend(Arc<dyn Backend>),
    /// The full list, at start and after the server comes back.
    Snapshot(Vec<AudioDevice>),
    Event(AudioEvent),
    /// The server is not available (sent once per outage).
    Unavailable(String),
}

/// Start the worker. Messages arrive on the returned channel until it is dropped.
pub fn spawn() -> async_channel::Receiver<WatchMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || run(tx));
    rx
}

/// Connect (retrying), then follow the server until the receiver is dropped.
fn run(tx: async_channel::Sender<WatchMsg>) {
    run_with_retry(&tx, WatchMsg::Unavailable, |outage| connect_and_follow(&tx, outage));
}

/// One session: `Ok` when the receiver is gone, `Err` when the server failed.
/// `outage` is told once a session is up, so the next outage is reported again.
fn connect_and_follow(tx: &async_channel::Sender<WatchMsg>, outage: &mut Outage) -> Result<(), String> {
    let mut session = Session::connect().map_err(|e| e.text())?;
    let known = session.list().map_err(|e| e.text())?;
    let send = |m: WatchMsg| tx.send_blocking(m).is_ok();
    if !send(WatchMsg::Backend(session.backend())) || !send(WatchMsg::Snapshot(known.clone())) {
        return Ok(());
    }
    outage.recovered();
    session.follow(known, |event| send(WatchMsg::Event(event))).map_err(|e| e.text())
}
