//! A worker that follows BlueZ and reports the Bluetooth state. It also
//! serves the pairing agent, registered again every time BlueZ comes back.

use crate::agent::{self, AgentSetup};
use crate::backend::Backend;
use crate::bluez::{BluezBackend, SERVICE};
use crate::model::BtState;
use opusdm_watch::{drain_burst, run_with_retry, Outage, COALESCE_MS};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use zbus::blocking::{Connection, MessageIterator};
use zbus::MatchRule;

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-bluetooth";
/// Signals the reader may hold before the bus drops the oldest.
const SIGNAL_QUEUE: usize = 256;

/// What the worker sends.
pub enum WatchMsg {
    /// The backend to use for requests; sent once per session, before the
    /// first state. Its connection serves the agent.
    Backend(Arc<dyn Backend>),
    /// The state, at start and after every change.
    State(BtState),
    /// The bus or BlueZ is not available (sent once per outage).
    Unavailable(String),
}

/// Start the worker; with `agent`, it serves and registers that agent. The
/// messages arrive on the returned channel until it is dropped.
pub fn spawn(agent: Option<AgentSetup>) -> async_channel::Receiver<WatchMsg> {
    let (tx, rx) = async_channel::unbounded();
    let spawned = std::thread::Builder::new().name("opusdm-bluetooth-watch".into()).spawn(move || {
        run_with_retry(&tx, WatchMsg::Unavailable, |outage| connect_and_follow(&tx, outage, agent.as_ref()));
    });
    if let Err(e) = spawned { opusdm_ipc::err_log!(LOG_TAG, "cannot start the Bluetooth watcher: {e}"); }
    rx
}

/// One session: `Ok` when the receiver is gone, `Err` when BlueZ failed,
/// went away or was started again (a new BlueZ forgot the agent).
fn connect_and_follow(tx: &async_channel::Sender<WatchMsg>, outage: &mut Outage, agent: Option<&AgentSetup>) -> Result<(), String> {
    let backend = Arc::new(BluezBackend::connect().map_err(|e| e.text())?);
    let (ticks, owner_changed) = follow_signals(backend.connection())?;
    let mut known = backend.state().map_err(|e| e.text())?;
    if let Some(setup) = agent {
        // Not fatal: the tray still works, only the pairing dialogs do not.
        if let Err(e) = agent::register(backend.connection(), setup) {
            opusdm_ipc::err_log!(LOG_TAG, "pairing agent {} not registered: {}", setup.path, e.text());
        }
    }
    let send = |m: WatchMsg| tx.send_blocking(m).is_ok();
    if !send(WatchMsg::Backend(backend.clone())) || !send(WatchMsg::State(known.clone())) { return Ok(()); }
    outage.recovered();
    while drain_burst(&ticks, Duration::from_millis(COALESCE_MS)) {
        if owner_changed.load(Ordering::Relaxed) { return Err("BlueZ stopped or was started again".to_string()); }
        let now = backend.state().map_err(|e| e.text())?;
        if now == known { continue; }
        if !send(WatchMsg::State(now.clone())) { return Ok(()); }
        known = now;
    }
    Err("lost the connection to the system bus".to_string())
}

/// A tick for every BlueZ signal, and for every change of the owner of
/// `org.bluez`, which also raises the returned flag.
fn follow_signals(conn: &Connection) -> Result<(mpsc::Receiver<()>, Arc<AtomicBool>), String> {
    let (tx, rx) = mpsc::channel();
    let rule = MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(SERVICE).map_err(|e| e.to_string())?.build();
    spawn_ticker(MessageIterator::for_match_rule(rule, conn, Some(SIGNAL_QUEUE)).map_err(|e| e.to_string())?, tx.clone(), None);
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .and_then(|b| b.interface("org.freedesktop.DBus"))
        .and_then(|b| b.member("NameOwnerChanged"))
        .and_then(|b| b.add_arg(SERVICE))
        .map_err(|e| e.to_string())?
        .build();
    let flag = Arc::new(AtomicBool::new(false));
    spawn_ticker(MessageIterator::for_match_rule(rule, conn, None).map_err(|e| e.to_string())?, tx, Some(flag.clone()));
    Ok((rx, flag))
}

/// Forward every message of `iter` as a tick, raising `flag` first, from a
/// reader thread that ends when the bus drops or the receiver does.
fn spawn_ticker(iter: MessageIterator, tick: mpsc::Sender<()>, flag: Option<Arc<AtomicBool>>) {
    std::thread::spawn(move || {
        for msg in iter {
            if msg.is_err() { return; }
            if let Some(f) = &flag { f.store(true, Ordering::Relaxed); }
            if tick.send(()).is_err() { return; }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::MANAGER_AGENT_PATH;

    /// Follows the real BlueZ with a (not default) agent: run by hand with
    /// `--ignored`.
    #[test]
    #[ignore]
    fn live_watcher_sends_backend_then_state() {
        let (events, _events_rx) = async_channel::unbounded();
        let rx = spawn(Some(AgentSetup { path: MANAGER_AGENT_PATH, default: false, events }));
        assert!(matches!(rx.recv_blocking().unwrap(), WatchMsg::Backend(_)));
        let WatchMsg::State(s) = rx.recv_blocking().unwrap() else { panic!("a state") };
        assert!(s.adapter.is_some());
    }
}
