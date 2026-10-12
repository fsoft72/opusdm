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
    // Not fatal: the tray works, only a profile that keeps its key out of
    // NetworkManager cannot join.
    if let Err(e) = backend.register_agent(crate::agent::PANEL_IDENTIFIER) {
        eprintln!("[opusdm-network:{}] secret agent: {}", std::process::id(), e.text());
    }
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

/// What the profiles worker sends to the network manager program.
pub enum ProfilesMsg {
    /// The backend to use; sent once per session, before the first `Changed`.
    Backend(Arc<NmBackend>),
    /// The secret agent could not be registered: connections that need a
    /// secret from the keyring will fail.
    AgentFailed(String),
    /// Profiles or connections may have changed: read them again.
    Changed,
    /// The bus or NetworkManager is not available (sent once per outage).
    Unavailable(String),
}

/// Start the worker of the network manager program: it registers a secret
/// agent under `identifier` and reports every burst of NetworkManager
/// signals. Messages arrive on the returned channel until it is dropped.
pub fn spawn_profiles(identifier: &'static str) -> async_channel::Receiver<ProfilesMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || run_with_retry(&tx, ProfilesMsg::Unavailable, |outage| follow_profiles(&tx, outage, identifier)));
    rx
}

/// One session: `Ok` when the receiver is gone, `Err` when NetworkManager failed.
fn follow_profiles(tx: &async_channel::Sender<ProfilesMsg>, outage: &mut Outage, identifier: &str) -> Result<(), String> {
    let backend = Arc::new(NmBackend::connect().map_err(|e| e.text())?);
    let ticks = signal_ticks(backend.connection(), NM_BUS_NAME)?;
    let send = |m: ProfilesMsg| tx.send_blocking(m).is_ok();
    if let Err(e) = backend.register_agent(identifier) {
        if !send(ProfilesMsg::AgentFailed(e.text())) { return Ok(()); }
    }
    if !send(ProfilesMsg::Backend(backend.clone())) || !send(ProfilesMsg::Changed) { return Ok(()); }
    outage.recovered();
    while drain_burst(&ticks, Duration::from_millis(COALESCE_MS)) {
        if !send(ProfilesMsg::Changed) { return Ok(()); }
    }
    Err("lost the connection to the system bus".to_string())
}
