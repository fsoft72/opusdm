//! A worker that follows NetworkManager and reports the network state.

use crate::backend::Backend;
use crate::model::NetState;
use crate::nm::NmBackend;
use opusdm_watch::{drain_burst, run_with_retry, signal_ticks, wait_for_change, Outage, Wake, COALESCE_MS};
use std::sync::Arc;
use std::time::Duration;

/// The bus name of NetworkManager, the sender of the signals that wake the worker.
const NM_BUS_NAME: &str = "org.freedesktop.NetworkManager";
/// The state is read again after this long without signals, so a missed
/// signal can never leave the tray stale.
const REFRESH: Duration = Duration::from_secs(15);
/// A burst of signals that never goes quiet (a scan in a crowded place)
/// still reads the state at least this often.
const MAX_BURST: Duration = Duration::from_secs(2);
/// Reads of the state tried in a row before the session is given up.
const READ_TRIES: u32 = 3;
/// Wait between two reads of the state that failed.
const READ_RETRY: Duration = Duration::from_millis(700);

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
    let mut known = read_state(&*backend)?;
    // Not fatal: the tray works, only a profile that keeps its key out of
    // NetworkManager cannot join.
    if let Err(e) = backend.register_agent(crate::agent::PANEL_IDENTIFIER) {
        eprintln!("[opusdm-network:{}] secret agent: {}", std::process::id(), e.text());
    }
    let send = |m: WatchMsg| tx.send_blocking(m).is_ok();
    if !send(WatchMsg::Backend(backend.clone())) || !send(WatchMsg::State(known.clone())) { return Ok(()); }
    outage.recovered();
    loop {
        if wait_for_change(&ticks, Duration::from_millis(COALESCE_MS), MAX_BURST, REFRESH) == Wake::Closed { break; }
        let now = read_state(&*backend)?;
        // Signal strength changes alone fire many signals: only a real change is sent.
        if now == known { continue; }
        if !send(WatchMsg::State(now.clone())) { return Ok(()); }
        known = now;
    }
    Err("lost the connection to the system bus".to_string())
}

/// Read the state, trying again a few times: an object that vanishes in the
/// middle of a read (a device or a connection going away) is not an outage.
/// Every failed read is logged; the last error ends the session.
fn read_state(backend: &dyn Backend) -> Result<NetState, String> {
    read_state_with(backend, READ_TRIES, READ_RETRY)
}

/// [`read_state`] with the tries and the wait given, for tests.
fn read_state_with(backend: &dyn Backend, tries: u32, wait: Duration) -> Result<NetState, String> {
    let mut attempt = 1;
    loop {
        match backend.state() {
            Ok(state) => return Ok(state),
            Err(e) => {
                eprintln!("[opusdm-network:{}] network state, read {attempt} of {tries}: {}", std::process::id(), e.text());
                if attempt >= tries { return Err(e.text()); }
            }
        }
        attempt += 1;
        std::thread::sleep(wait);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::FakeBackend;
    use crate::error::NetError;

    #[test]
    fn a_failed_read_is_tried_again_and_the_last_error_ends_it() {
        let fake = FakeBackend::default();
        *fake.fail_state.lock().unwrap() = 1;
        assert!(read_state_with(&fake, 3, Duration::ZERO).is_ok(), "one failure is not an outage");
        *fake.fail_state.lock().unwrap() = 5;
        *fake.state_error.lock().unwrap() = Some(NetError::other("gone"));
        assert_eq!(read_state_with(&fake, 3, Duration::ZERO).unwrap_err(), "gone");
    }
}
