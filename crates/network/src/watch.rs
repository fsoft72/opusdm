//! A worker that follows NetworkManager and reports the network state.

use crate::backend::Backend;
use crate::model::NetState;
use crate::nm::NmBackend;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;
use zbus::blocking::MessageIterator;
use zbus::message::Type;
use zbus::MatchRule;

/// Quiet time after a burst of NetworkManager signals before the state is read again.
const COALESCE_MS: u64 = 300;
/// Wait before trying again when the bus or NetworkManager is missing.
const RETRY_SECS: u64 = 30;

/// What the worker sends.
pub enum WatchMsg {
    /// The backend to use for operations; sent once per session, before the first state.
    Backend(Arc<dyn Backend>),
    /// The state, at start and after every change.
    State(NetState),
    /// The bus or NetworkManager is not available (sent once per outage).
    Unavailable(String),
}

/// Wait for one tick, then keep reading ticks until none arrives for `quiet`.
/// Returns false when the sender is gone.
pub fn drain_burst(rx: &Receiver<()>, quiet: Duration) -> bool {
    if rx.recv().is_err() { return false; }
    loop {
        match rx.recv_timeout(quiet) {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => return true,
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
        }
    }
}

/// Start the worker. Messages arrive on the returned channel until it is dropped.
pub fn spawn() -> async_channel::Receiver<WatchMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || run(tx));
    rx
}

/// Connect (retrying), then follow NetworkManager until the receiver is dropped.
fn run(tx: async_channel::Sender<WatchMsg>) {
    let mut reported = false;
    loop {
        match connect_and_follow(&tx) {
            Ok(()) => return,
            Err(msg) => {
                if !reported && tx.send_blocking(WatchMsg::Unavailable(msg)).is_err() { return; }
                reported = true;
            }
        }
        std::thread::sleep(Duration::from_secs(RETRY_SECS));
    }
}

/// One session: `Ok` when the receiver is gone, `Err` when NetworkManager failed.
fn connect_and_follow(tx: &async_channel::Sender<WatchMsg>) -> Result<(), String> {
    let backend = Arc::new(NmBackend::connect().map_err(|e| e.text())?);
    let ticks = start_signal_reader(&backend)?;
    let mut known = backend.state().map_err(|e| e.text())?;
    let send = |m: WatchMsg| tx.send_blocking(m).is_ok();
    if !send(WatchMsg::Backend(backend.clone())) || !send(WatchMsg::State(known.clone())) { return Ok(()); }
    while drain_burst(&ticks, Duration::from_millis(COALESCE_MS)) {
        let now = backend.state().map_err(|e| e.text())?;
        // Signal strength changes alone fire many signals: only a real change is sent.
        if now == known { continue; }
        if !send(WatchMsg::State(now.clone())) { return Ok(()); }
        known = now;
    }
    Err("lost the connection to the system bus".to_string())
}

/// Forward every NetworkManager signal as a tick from a reader thread.
fn start_signal_reader(backend: &NmBackend) -> Result<Receiver<()>, String> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.freedesktop.NetworkManager")
        .map_err(|e| e.to_string())?
        .build();
    let iter = MessageIterator::for_match_rule(rule, backend.connection(), Some(256)).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for msg in iter {
            if msg.is_err() || tx.send(()).is_err() { return; }
        }
    });
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_ticks_is_one_wakeup() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..3 {
                tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(300));
            drop(tx);
        });
        assert!(drain_burst(&rx, Duration::from_millis(100)));
        assert!(!drain_burst(&rx, Duration::from_millis(100)));
    }
}
