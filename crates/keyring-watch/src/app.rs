//! The watcher process: checks the Secret Service now and then and keeps the
//! tray icon and the notification in step with the result.
//!
//! Ownership for the whole run: [`run`] owns the session bus connection that
//! holds the single instance name, the checker thread (joined by `run`: if it
//! dies the process exits with an error and the hub restarts it) and the
//! command thread. The tray handle lives in the checker thread and exists
//! only while the keyring has a problem.

use crate::ask::AskMode;
use crate::gnome;
use crate::health::{Event, Reason, Tracker};
use crate::notify;
use crate::probe::{probe_with_timeout, PROBE_TIMEOUT};
use crate::selftest::{self, SecretServiceStore};
use crate::tray::{Cmd, KeyringTray};
use ksni::blocking::{Handle, TrayMethods};
use opusdm_ipc::instance::EXIT_ALREADY_RUNNING;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

/// Tag used in log lines.
const TAG: &str = "opusdm-keyring-watch";
/// The bus name that makes the watcher a single instance.
const INSTANCE_NAME: &str = "org.opusdm.KeyringWatch";
/// The wait before the first check, so the session can come up.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
/// The time between two checks.
const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// How long a call to the tray or the notification service may take before
/// the checker goes on without it.
const UI_CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Exit code for a failure that is not "already running".
const EXIT_FAILURE: i32 = 2;

/// What the process was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Watch the keyring (no arguments).
    Watch,
    /// Show the password window.
    Ask(AskMode),
    /// Arguments that mean nothing.
    Invalid,
}

/// Read the command line (without the program name).
pub fn parse_args(args: &[&str]) -> Mode {
    match args {
        [] => Mode::Watch,
        ["--unlock"] => Mode::Ask(AskMode::Unlock),
        ["--ask-password", "unlock"] => Mode::Ask(AskMode::Unlock),
        ["--ask-password", "restart"] => Mode::Ask(AskMode::Restart),
        _ => Mode::Invalid,
    }
}

/// Run the watcher. Returns the exit code of the process.
pub fn run() -> i32 {
    let _instance = match _acquire_instance() {
        Ok(conn) => conn,
        Err(code) => return code,
    };
    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    let commands = {
        let wake_tx = wake_tx.clone();
        std::thread::Builder::new().name("opusdm-keyring-commands".into()).spawn(move || _command_loop(cmd_rx, wake_tx))
    };
    if let Err(e) = commands {
        opusdm_ipc::err_log!(TAG, "cannot start the command thread: {e}");
        return EXIT_FAILURE;
    }
    let checker = std::thread::Builder::new().name("opusdm-keyring-checker".into()).spawn(move || _check_loop(cmd_tx, wake_rx));
    match checker {
        Ok(handle) => match handle.join() {
            Ok(()) => EXIT_FAILURE,
            Err(_) => {
                opusdm_ipc::err_log!(TAG, "the checker thread panicked, exiting so the hub restarts the watcher");
                EXIT_FAILURE
            }
        },
        Err(e) => {
            opusdm_ipc::err_log!(TAG, "cannot start the checker thread: {e}");
            EXIT_FAILURE
        }
    }
}

/// Take the single instance name on the session bus. The connection must
/// stay alive for the name to stay owned.
fn _acquire_instance() -> Result<zbus::blocking::Connection, i32> {
    let conn = zbus::blocking::Connection::session().map_err(|e| {
        opusdm_ipc::err_log!(TAG, "cannot connect to the session bus: {e}");
        EXIT_FAILURE
    })?;
    match conn.request_name_with_flags(INSTANCE_NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into()) {
        Ok(_) => Ok(conn),
        Err(zbus::Error::NameTaken) => {
            opusdm_ipc::err_log!(TAG, "another watcher is running, exiting");
            Err(EXIT_ALREADY_RUNNING)
        }
        Err(e) => {
            opusdm_ipc::err_log!(TAG, "cannot take {INSTANCE_NAME}: {e}");
            Err(EXIT_FAILURE)
        }
    }
}

/// The check loop: wait, check, and show or remove the alarm. Never returns
/// unless the wake channel is gone (then the process ends).
fn _check_loop(cmd_tx: Sender<Cmd>, wake_rx: Receiver<()>) {
    let mut tracker = Tracker::new(0);
    let mut alarm = Alarm::default();
    let mut wait = FIRST_CHECK_DELAY;
    loop {
        match wake_rx.recv_timeout(wait) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                opusdm_ipc::err_log!(TAG, "the wake channel is closed, the watcher stops");
                return;
            }
        }
        wait = CHECK_INTERVAL;
        let health = probe_with_timeout(PROBE_TIMEOUT);
        opusdm_ipc::debug_log!(TAG, "check: {health:?}");
        match tracker.observe(health) {
            Event::None => {}
            Event::Raise(reason) => alarm.raise(reason, &cmd_tx),
            Event::Clear => alarm.clear(),
        }
    }
}

/// The alarm now showing: the tray icon and the last notification.
#[derive(Default)]
struct Alarm {
    /// The tray item, while the keyring has a problem.
    tray: Option<Handle<KeyringTray>>,
    /// The id of the last notification, to replace it.
    notification: u32,
}

impl Alarm {
    /// Show the alarm for `reason`: the tray icon (created or updated) and
    /// a notification that replaces the previous one.
    fn raise(&mut self, reason: Reason, cmd_tx: &Sender<Cmd>) {
        let gnome = gnome::owner_is_gnome();
        match self.tray.clone() {
            Some(handle) if !handle.is_closed() => {
                let updated = _bounded("update the tray item", move || handle.update(|t| { t.reason = reason; t.gnome = gnome; }));
                if updated.flatten().is_none() { opusdm_ipc::err_log!(TAG, "the tray item is gone or does not answer, cannot update it"); }
            }
            _ => {
                let tray = KeyringTray { reason, gnome, tx: cmd_tx.clone() };
                self.tray = match _bounded("show the tray icon", move || tray.spawn()) {
                    Some(Ok(handle)) => Some(handle),
                    Some(Err(e)) => {
                        opusdm_ipc::err_log!(TAG, "cannot show the tray icon (is there a StatusNotifierWatcher?): {e}; the notification is the only signal");
                        None
                    }
                    None => None,
                };
            }
        }
        let (summary, body, replaces) = ("Keyring problem", format!("{}. Click the icon in the tray.", reason.text()), self.notification);
        match _bounded("show the notification", move || notify::send(summary, &body, replaces)) {
            Some(Ok(id)) => self.notification = id,
            Some(Err(e)) => opusdm_ipc::err_log!(TAG, "cannot show the notification: {e}"),
            None => {}
        }
    }

    /// Remove the alarm: the tray icon goes away, the next problem gets a
    /// new notification.
    fn clear(&mut self) {
        if let Some(handle) = self.tray.take() { _bounded("remove the tray icon", move || handle.shutdown().wait()); }
        self.notification = 0;
    }
}

/// Run `f` on its own thread and wait [`UI_CALL_TIMEOUT`] for its result.
/// `None` (logged) when it does not answer in time or the thread cannot be
/// started: a stuck panel or notification daemon must not freeze the checker.
/// The stuck thread is left to finish by itself.
fn _bounded<R: Send + 'static>(what: &str, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    _bounded_within(what, f, UI_CALL_TIMEOUT)
}

/// [`_bounded`] with the time limit given.
fn _bounded_within<R: Send + 'static>(what: &str, f: impl FnOnce() -> R + Send + 'static, limit: Duration) -> Option<R> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("opusdm-keyring-ui".into()).spawn(move || {
        // The receiver is gone when the caller timed out: nothing to report.
        let _ = tx.send(f());
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(TAG, "cannot start a thread to {what}: {e}");
        return None;
    }
    match rx.recv_timeout(limit) {
        Ok(result) => Some(result),
        Err(_) => {
            opusdm_ipc::err_log!(TAG, "timed out (no answer in {limit:?}) trying to {what}");
            None
        }
    }
}

/// Take the "an action is running" flag; `false` when it was already taken.
fn _claim(busy: &AtomicBool) -> bool {
    !busy.swap(true, Ordering::SeqCst)
}

/// Run the commands of the tray menu. A slow action (a prompt, the password
/// window, the test) runs on its own thread, one at a time: a click while it
/// runs is logged and answered with a notification, and "Check now" always
/// works.
fn _command_loop(cmd_rx: Receiver<Cmd>, wake_tx: Sender<()>) {
    let busy = Arc::new(AtomicBool::new(false));
    for cmd in cmd_rx {
        opusdm_ipc::debug_log!(TAG, "command: {cmd:?}");
        if cmd != Cmd::CheckNow {
            if !_claim(&busy) {
                opusdm_ipc::err_log!(TAG, "{cmd:?} ignored: another keyring action is still running");
                _tell("Keyring", "Another keyring action is still running.");
                continue;
            }
            let (running, wake_tx) = (busy.clone(), wake_tx.clone());
            let spawned = std::thread::Builder::new().name("opusdm-keyring-action".into()).spawn(move || {
                _run_action(cmd);
                running.store(false, Ordering::SeqCst);
                // Whatever was done, look again now.
                if wake_tx.send(()).is_err() { opusdm_ipc::err_log!(TAG, "the checker is gone, cannot ask for a new check"); }
            });
            if let Err(e) = spawned {
                opusdm_ipc::err_log!(TAG, "cannot start a thread for {cmd:?}: {e}");
                busy.store(false, Ordering::SeqCst);
            }
            continue;
        }
        if wake_tx.send(()).is_err() {
            opusdm_ipc::err_log!(TAG, "the checker is gone, the command loop stops");
            return;
        }
    }
    opusdm_ipc::err_log!(TAG, "the command channel is closed, the command loop stops");
}

/// Do one slow action of the menu.
fn _run_action(cmd: Cmd) {
    match cmd {
        Cmd::Unlock => _unlock_through_service(),
        Cmd::UnlockWithPassword => _ask_password("unlock"),
        Cmd::Restart => _ask_password("restart"),
        Cmd::Test => _self_test(),
        Cmd::CheckNow => {}
    }
}

/// Ask the Secret Service to unlock the default collection; the provider
/// shows its own prompt.
fn _unlock_through_service() {
    use secret_service::blocking::SecretService;
    use secret_service::EncryptionType;
    let result = SecretService::connect(EncryptionType::Dh).and_then(|ss| ss.get_collection_by_alias("default").and_then(|c| c.unlock()));
    if let Err(e) = result {
        opusdm_ipc::err_log!(TAG, "unlock through the Secret Service failed: {e}");
        _tell("Keyring unlock failed", &e.to_string());
    }
}

/// Run the password window as a child process and log how it ended.
fn _ask_password(mode: &str) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            opusdm_ipc::err_log!(TAG, "cannot find my own program to open the password window: {e}");
            return;
        }
    };
    match std::process::Command::new(exe).args(["--ask-password", mode]).status() {
        Ok(status) => opusdm_ipc::debug_log!(TAG, "the password window ended: {status}"),
        Err(e) => opusdm_ipc::err_log!(TAG, "cannot open the password window: {e}"),
    }
}

/// Write, read and delete a test item and tell the result.
fn _self_test() {
    match selftest::run(&SecretServiceStore, &selftest::random_value()) {
        Ok(()) => _tell("Keyring test", "The keyring works: an item was written, read and deleted."),
        Err(e) => {
            opusdm_ipc::err_log!(TAG, "keyring test failed: {e}");
            _tell("Keyring test failed", &e);
        }
    }
}

/// Show a notification; a failure is logged.
fn _tell(summary: &str, body: &str) {
    if let Err(e) = notify::send(summary, body, 0) { opusdm_ipc::err_log!(TAG, "cannot show the notification \"{summary}\": {e}"); }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`_bounded_within`] under a name that reads well in the tests.
    fn _bounded_for_test<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static, limit: Duration) -> Option<R> {
        _bounded_within("test", f, limit)
    }

    #[test]
    fn only_one_action_runs_at_a_time() {
        let busy = AtomicBool::new(false);
        assert!(_claim(&busy));
        assert!(!_claim(&busy), "a second action is refused while the first runs");
        busy.store(false, Ordering::SeqCst);
        assert!(_claim(&busy));
    }

    #[test]
    fn a_stuck_call_does_not_block_the_caller() {
        let started = std::time::Instant::now();
        let result = _bounded_for_test(|| { std::thread::sleep(Duration::from_secs(2)); 1 }, Duration::from_millis(50));
        assert_eq!(result, None);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(_bounded_for_test(|| 7, Duration::from_secs(1)), Some(7));
    }

    #[test]
    fn no_arguments_watch() {
        assert_eq!(parse_args(&[]), Mode::Watch);
    }

    #[test]
    fn unlock_flags() {
        assert_eq!(parse_args(&["--unlock"]), Mode::Ask(AskMode::Unlock));
        assert_eq!(parse_args(&["--ask-password", "unlock"]), Mode::Ask(AskMode::Unlock));
        assert_eq!(parse_args(&["--ask-password", "restart"]), Mode::Ask(AskMode::Restart));
    }

    #[test]
    fn unknown_arguments_are_invalid() {
        assert_eq!(parse_args(&["--nope"]), Mode::Invalid);
        assert_eq!(parse_args(&["--ask-password"]), Mode::Invalid);
        assert_eq!(parse_args(&["--ask-password", "other"]), Mode::Invalid);
    }
}
