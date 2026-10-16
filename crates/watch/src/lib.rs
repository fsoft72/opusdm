//! The skeleton shared by the device, audio and network workers: a retrying
//! connection loop that reports an outage once, and a burst drain that turns
//! many bus signals into one wakeup, plus the [`DbusError`] their calls
//! return. No GTK.

mod dbus_error;

pub use dbus_error::DbusError;

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::MatchRule;

/// Wait before trying again when the bus or the service is missing.
pub const RETRY_SECS: u64 = 30;
/// Quiet time after a burst of signals before the state is read again.
pub const COALESCE_MS: u64 = 300;
/// Signals the reader may hold before the bus drops the oldest.
const SIGNAL_QUEUE: usize = 256;

/// Tells an outage that was already reported from a new one.
#[derive(Default)]
pub struct Outage {
    reported: bool,
}

impl Outage {
    /// Call once a session is up: the next failure is a new outage and is
    /// reported again.
    pub fn recovered(&mut self) {
        self.reported = false;
    }
}

/// Run `session` until it returns `Ok` (the receiver is gone). On `Err` the
/// text goes out once per outage through `unavailable`, then the session is
/// tried again every [`RETRY_SECS`]. Returns early when the receiver is gone.
pub fn run_with_retry<M>(
    tx: &async_channel::Sender<M>,
    unavailable: impl Fn(String) -> M,
    session: impl FnMut(&mut Outage) -> Result<(), String>,
) {
    retry_every(Duration::from_secs(RETRY_SECS), tx, unavailable, session);
}

/// [`run_with_retry`] with the wait between attempts given, for tests.
fn retry_every<M>(
    wait: Duration,
    tx: &async_channel::Sender<M>,
    unavailable: impl Fn(String) -> M,
    mut session: impl FnMut(&mut Outage) -> Result<(), String>,
) {
    let mut outage = Outage::default();
    loop {
        match session(&mut outage) {
            Ok(()) => return,
            Err(msg) => {
                if !outage.reported && tx.send_blocking(unavailable(msg)).is_err() {
                    return;
                }
                outage.reported = true;
            }
        }
        std::thread::sleep(wait);
    }
}

/// Wait for one tick, then keep reading ticks until none arrives for `quiet`.
/// Returns false when the sender is gone.
pub fn drain_burst(rx: &Receiver<()>, quiet: Duration) -> bool {
    if rx.recv().is_err() {
        return false;
    }
    loop {
        match rx.recv_timeout(quiet) {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => return true,
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
        }
    }
}

/// Forward every signal sent by the bus name `sender` as a tick, from a reader
/// thread that ends when the bus drops or the receiver does.
pub fn signal_ticks(conn: &Connection, sender: &'static str) -> Result<Receiver<()>, String> {
    signal_ticks_any(conn, &[sender])
}

/// [`signal_ticks`] for several bus names at once: a signal from any of them
/// is a tick on the one returned channel.
pub fn signal_ticks_any(conn: &Connection, senders: &[&'static str]) -> Result<Receiver<()>, String> {
    let (tx, rx) = mpsc::channel();
    for &sender in senders {
        let rule = MatchRule::builder().msg_type(Type::Signal).sender(sender).map_err(|e| e.to_string())?.build();
        let iter = MessageIterator::for_match_rule(rule, conn, Some(SIGNAL_QUEUE)).map_err(|e| e.to_string())?;
        let tx = tx.clone();
        std::thread::spawn(move || {
            for msg in iter {
                if msg.is_err() || tx.send(()).is_err() {
                    return;
                }
            }
        });
    }
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

    #[test]
    fn a_finished_session_stops_the_loop_without_a_report() {
        let (tx, rx) = async_channel::unbounded::<String>();
        run_with_retry(&tx, |m| m, |_| Ok(()));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn an_outage_is_reported_once_until_the_session_recovers() {
        let (tx, rx) = async_channel::unbounded::<String>();
        let mut calls = 0;
        retry_every(
            Duration::from_millis(1),
            &tx,
            |m| m,
            |outage| {
                calls += 1;
                match calls {
                    // Two failures in a row: one report.
                    1 | 2 => Err(format!("down {calls}")),
                    // Comes up, then fails again: a new outage, reported again.
                    3 => {
                        outage.recovered();
                        Err("down again".to_string())
                    }
                    _ => Ok(()),
                }
            },
        );
        assert_eq!(calls, 4);
        assert_eq!(rx.try_recv().unwrap(), "down 1");
        assert_eq!(rx.try_recv().unwrap(), "down again");
        assert!(rx.try_recv().is_err());
    }
}
