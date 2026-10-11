//! The worker that owns the backend. `ddcutil` takes up to a second per call,
//! so detection and every `set` run on their own thread, and a burst of
//! slider values is collapsed to the last one.

use std::sync::mpsc;
use std::thread;

use async_channel::Receiver;

use crate::backend::Detected;
use crate::{clamp_percent, BrightnessError};

/// What the worker reports to the window.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A backend was found: its name and the current brightness.
    Ready { method: &'static str, percent: u8 },
    /// No backend works; the text says why.
    Unavailable(String),
    /// The last value was applied.
    Applied(u8),
    /// The brightness read back on request (see [`Controller::read`]).
    Current(u8),
    /// The last value could not be applied; the text says why.
    Failed(String),
}

/// What the controller asks the worker for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Command {
    /// Apply this brightness.
    Set(u8),
    /// Read the brightness again: another program or a key may have changed it.
    Read,
}

/// The handle used by the window to ask for a brightness. Dropping it stops
/// the worker.
pub struct Controller {
    tx: mpsc::Sender<Command>,
}

impl Controller {
    /// Ask the worker to set `percent` (limited to the allowed range).
    pub fn set(&self, percent: u8) -> Result<(), BrightnessError> {
        self.send(Command::Set(clamp_percent(percent)))
    }

    /// Ask the worker to read the brightness again; the answer comes back as
    /// [`Event::Current`].
    pub fn read(&self) -> Result<(), BrightnessError> {
        self.send(Command::Read)
    }

    /// Hand one command to the worker.
    fn send(&self, command: Command) -> Result<(), BrightnessError> {
        self.tx.send(command).map_err(|_| BrightnessError::Unavailable("the brightness worker has stopped".to_string()))
    }
}

/// Start the worker. `detect` runs on the new thread; the events come back on
/// the returned channel (`recv().await` in the UI, `recv_blocking()` in tests).
pub fn spawn(detect: impl FnOnce() -> Result<Detected, BrightnessError> + Send + 'static) -> (Controller, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let (events_tx, events_rx) = async_channel::unbounded();
    thread::spawn(move || run(detect, rx, events_tx));
    (Controller { tx }, events_rx)
}

/// The worker loop: detect, then apply the last pending value and answer
/// reads until the controller is dropped.
fn run(detect: impl FnOnce() -> Result<Detected, BrightnessError>, rx: mpsc::Receiver<Command>, events: async_channel::Sender<Event>) {
    // A closed events channel means the window is gone: nobody to tell.
    let send = |event: Event| {
        let _ = events.send_blocking(event);
    };
    let Detected { backend, percent } = match detect() {
        Ok(found) => found,
        Err(e) => return send(Event::Unavailable(e.to_string())),
    };
    send(Event::Ready { method: backend.name(), percent });
    while let Ok(first) = rx.recv() {
        let batch: Vec<Command> = std::iter::once(first).chain(rx.try_iter()).collect();
        let (latest, read) = squash(&batch);
        if let Some(percent) = latest {
            match backend.set(percent) {
                Ok(()) => send(Event::Applied(percent)),
                Err(e) => send(Event::Failed(e.to_string())),
            }
        }
        if read {
            match backend.get() {
                Ok(percent) => send(Event::Current(clamp_percent(percent))),
                Err(e) => send(Event::Failed(e.to_string())),
            }
        }
    }
}

/// Collapse a burst of commands: the last value to set, and whether a read
/// was asked for. The read runs after the set, so it sees the new value.
fn squash(batch: &[Command]) -> (Option<u8>, bool) {
    let latest = batch.iter().rev().find_map(|c| match c {
        Command::Set(percent) => Some(*percent),
        Command::Read => None,
    });
    (latest, batch.contains(&Command::Read))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::fake::FakeBackend;

    fn detected(fake: FakeBackend) -> Result<Detected, BrightnessError> {
        let percent = fake.get()?;
        Ok(Detected { backend: Box::new(fake), percent })
    }

    #[test]
    fn reports_ready_with_the_current_value() {
        let (_controller, events) = spawn(|| detected(FakeBackend::new(64)));
        assert_eq!(events.recv_blocking().unwrap(), Event::Ready { method: "fake", percent: 64 });
    }

    #[test]
    fn reports_why_nothing_is_available() {
        let (_controller, events) = spawn(|| Err(BrightnessError::Unavailable("no way".to_string())));
        assert_eq!(events.recv_blocking().unwrap(), Event::Unavailable("no way".to_string()));
    }

    #[test]
    fn the_last_value_is_always_applied() {
        let fake = FakeBackend::new(50);
        let applied = fake.applied_handle();
        let (controller, events) = spawn(|| detected(fake));
        for v in [20, 30, 40] {
            controller.set(v).unwrap();
        }
        loop {
            if events.recv_blocking().unwrap() == Event::Applied(40) {
                break;
            }
        }
        assert_eq!(applied.lock().unwrap().last(), Some(&40));
    }

    #[test]
    fn values_are_clamped_before_they_reach_the_backend() {
        let fake = FakeBackend::new(50);
        let applied = fake.applied_handle();
        let (controller, events) = spawn(|| detected(fake));
        controller.set(0).unwrap();
        loop {
            if let Event::Applied(v) = events.recv_blocking().unwrap() {
                assert_eq!(v, crate::MIN_PERCENT);
                break;
            }
        }
        assert_eq!(applied.lock().unwrap().as_slice(), &[crate::MIN_PERCENT]);
    }

    #[test]
    fn a_read_reports_the_current_value() {
        let (controller, events) = spawn(|| detected(FakeBackend::new(64)));
        controller.read().unwrap();
        loop {
            if let Event::Current(v) = events.recv_blocking().unwrap() {
                assert_eq!(v, 64);
                break;
            }
        }
    }

    #[test]
    fn a_burst_keeps_the_last_set_and_any_read() {
        assert_eq!(squash(&[Command::Set(20), Command::Read, Command::Set(30)]), (Some(30), true));
        assert_eq!(squash(&[Command::Read, Command::Read]), (None, true));
        assert_eq!(squash(&[Command::Set(20)]), (Some(20), false));
    }

    #[test]
    fn a_failing_set_is_reported() {
        let (controller, events) = spawn(|| {
            // Probe works, the set fails: a backend that reads but cannot write.
            struct ReadOnly;
            impl Backend for ReadOnly {
                fn name(&self) -> &'static str {
                    "read-only"
                }
                fn get(&self) -> Result<u8, BrightnessError> {
                    Ok(50)
                }
                fn set(&self, _: u8) -> Result<(), BrightnessError> {
                    Err(BrightnessError::Unavailable("read only".to_string()))
                }
            }
            Ok(Detected { backend: Box::new(ReadOnly), percent: 50 })
        });
        controller.set(30).unwrap();
        loop {
            if let Event::Failed(text) = events.recv_blocking().unwrap() {
                assert_eq!(text, "read only");
                break;
            }
        }
    }
}
