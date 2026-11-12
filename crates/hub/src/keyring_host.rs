//! Starting and supervising `opusdm-keyring-watch`, the process that checks
//! the Secret Service and shows a tray icon when the keyring is locked or
//! missing.
//!
//! Ownership for the whole run: [`start`] spawns the detached thread
//! `opusdm-keyring-supervisor`, which owns the `Child` of the watcher. The
//! hub never joins or waits for that thread and no hub object holds a handle
//! to it or to the child. When the child exits, the supervisor logs the
//! status and starts it again after a growing delay (1 s, 2 s, 4 s), up to
//! [`MAX_RESTARTS`] times in [`RESTART_WINDOW`]; past that it logs once that
//! it gives up and the thread ends. An exit with [`EXIT_ALREADY_RUNNING`]
//! (another watcher owns the instance name, the hub was restarted while the
//! old one lived) is not a failure: the supervisor waits
//! [`ALREADY_RUNNING_RETRY`] and tries again, without limit. When the hub
//! process exits the thread dies with it and the watcher keeps running; a
//! restarted hub then sees `EXIT_ALREADY_RUNNING` and waits. If the watcher
//! is gone for good (the supervisor gave up, or the binary is missing) the
//! log says so and the hub, the Lister and the hotkeys are not affected:
//! only the keyring alarm is missing.

use crate::applet_host::{next_action, Action, MAX_RESTARTS, RESTART_WINDOW};
use opusdm_ipc::instance::EXIT_ALREADY_RUNNING;
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// The binary started next to the hub.
const TOOL: &str = "opusdm-keyring-watch";

/// Name of the supervisor thread.
const THREAD_NAME: &str = "opusdm-keyring-supervisor";

/// Tag used in log lines.
const TAG: &str = "opusdm-hub";

/// How often the supervisor checks whether the child exited (slow on
/// purpose: it runs for the whole session).
const SUPERVISOR_POLL: Duration = Duration::from_millis(500);

/// The delay before the first restart in the window; it doubles for each
/// further one (1 s, 2 s, 4 s).
const RESTART_BACKOFF_BASE: Duration = Duration::from_secs(1);

/// The highest doubling of [`RESTART_BACKOFF_BASE`], so the shift never
/// overflows.
const RESTART_BACKOFF_MAX_SHIFT: usize = 6;

/// Why the supervisor loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The binary does not exist.
    MissingBinary,
    /// Too many restarts in the window.
    GaveUp,
}

/// The delay before the `n`-th restart in the window (1-based): 1 s, 2 s,
/// 4 s... `0` is treated as the first.
fn restart_delay(n: usize) -> Duration {
    let shift = n.saturating_sub(1).min(RESTART_BACKOFF_MAX_SHIFT) as u32;
    RESTART_BACKOFF_BASE * (1u32 << shift)
}

/// Start the supervisor thread of `opusdm-keyring-watch`. Returns at once;
/// the hub never waits for it.
pub fn start() {
    let dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    // Detached on purpose: the handle is dropped, the hub never joins it.
    // The thread owns the child; when the hub exits the thread dies with it
    // and the child keeps running (a restarted hub then waits for it).
    let spawned = std::thread::Builder::new().name(THREAD_NAME.into()).spawn(move || {
        let ended = supervise(|| spawn_from(dir.as_deref()), |child| _wait_polling(child, SUPERVISOR_POLL), std::thread::sleep);
        opusdm_ipc::err_log!(TAG, "{THREAD_NAME} ended: {ended:?}, no keyring alarm until the hub is restarted");
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(TAG, "cannot start the {THREAD_NAME} thread, no keyring alarm: {e}");
    }
}

/// Start `opusdm-keyring-watch` from `dir` (the hub's folder), or by `PATH`
/// when the folder is unknown. The child inherits the hub's environment.
fn spawn_from(dir: Option<&Path>) -> io::Result<Child> {
    Command::new(opusdm_ipc::instance::sibling_bin_in(dir, TOOL)).spawn()
}

/// The supervisor loop: start the child with `spawn`, wait for it with
/// `wait`, log the exit and act on [`next_action`]; `sleep` waits before a
/// restart or an "already running" retry.
fn supervise(
    mut spawn: impl FnMut() -> io::Result<Child>,
    mut wait: impl FnMut(&mut Child) -> io::Result<ExitStatus>,
    mut sleep: impl FnMut(Duration),
) -> Ended {
    let mut history = VecDeque::new();
    loop {
        let status = match spawn() {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                opusdm_ipc::err_log!(TAG, "cannot start {TOOL}, not found: {e}");
                return Ended::MissingBinary;
            }
            Err(e) => {
                opusdm_ipc::err_log!(TAG, "cannot start {TOOL}: {e}");
                None
            }
            Ok(mut child) => {
                let pid = child.id();
                _log_exit(pid, wait(&mut child))
            }
        };
        match next_action(status, &mut history, Instant::now()) {
            Action::Restart => {
                let delay = restart_delay(history.len());
                opusdm_ipc::err_log!(TAG, "restarting {TOOL} in {}s", delay.as_secs());
                sleep(delay);
            }
            Action::WaitAndRetry(delay) => {
                opusdm_ipc::debug_log!(TAG, "{TOOL} already running, retrying in {}s", delay.as_secs());
                sleep(delay);
            }
            Action::GiveUp => {
                opusdm_ipc::err_log!(TAG, "{TOOL} exited more than {MAX_RESTARTS} times in {}s, giving up", RESTART_WINDOW.as_secs());
                return Ended::GaveUp;
            }
        }
    }
}

/// Log how the child `pid` ended. Returns the exit code, `None` when it was
/// killed by a signal or the wait failed. An "already running" exit is
/// logged at debug level only.
fn _log_exit(pid: u32, result: io::Result<ExitStatus>) -> Option<i32> {
    let status = match result {
        Ok(status) => status,
        Err(e) => {
            opusdm_ipc::err_log!(TAG, "cannot wait for {TOOL} (pid {pid}): {e}");
            return None;
        }
    };
    if status.code() == Some(EXIT_ALREADY_RUNNING) {
        opusdm_ipc::debug_log!(TAG, "{TOOL} (pid {pid}) exited: {status} (already running)");
    } else {
        opusdm_ipc::err_log!(TAG, "{TOOL} (pid {pid}) exited: {status}");
    }
    status.code()
}

/// Block until `child` exits, checking every `interval`, like
/// `reaped::wait` but with a slow poll (safe against the SIGCHLD reaper
/// taking the child first).
fn _wait_polling(child: &mut Child, interval: Duration) -> io::Result<ExitStatus> {
    let mut lost_since = None;
    loop {
        if let Some(status) = opusdm_ipc::reaped::poll_exit(child, &mut lost_since)? {
            return Ok(status);
        }
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applet_host::ALREADY_RUNNING_RETRY;
    use std::cell::{Cell, RefCell};

    /// A shell exiting with `code`.
    fn sh_exit(code: i32) -> io::Result<Child> {
        Command::new("sh").arg("-c").arg(format!("exit {code}")).spawn()
    }

    /// A spawn that fails with "not found", to end a supervisor run.
    fn not_found() -> io::Result<Child> {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }

    #[test]
    fn restarts_after_a_crash() {
        let spawns = Cell::new(0);
        let sleeps = RefCell::new(Vec::new());
        let ended = supervise(
            || {
                spawns.set(spawns.get() + 1);
                if spawns.get() <= 2 { sh_exit(1) } else { not_found() }
            },
            opusdm_ipc::reaped::wait,
            |d| sleeps.borrow_mut().push(d),
        );
        assert_eq!(ended, Ended::MissingBinary);
        assert_eq!(spawns.get(), 3);
        assert_eq!(*sleeps.borrow(), [Duration::from_secs(1), Duration::from_secs(2)], "a growing delay before each restart");
    }

    #[test]
    fn already_running_waits_and_retries() {
        let spawns = Cell::new(0);
        let sleeps = RefCell::new(Vec::new());
        let ended = supervise(
            || {
                spawns.set(spawns.get() + 1);
                if spawns.get() <= 6 { sh_exit(EXIT_ALREADY_RUNNING) } else { not_found() }
            },
            opusdm_ipc::reaped::wait,
            |d| sleeps.borrow_mut().push(d),
        );
        assert_eq!(ended, Ended::MissingBinary, "six waits are more than the restart limit and it never gave up");
        assert_eq!(*sleeps.borrow(), vec![ALREADY_RUNNING_RETRY; 6]);
    }

    #[test]
    fn gives_up_after_too_many_restarts() {
        let spawns = Cell::new(0);
        let ended = supervise(
            || {
                spawns.set(spawns.get() + 1);
                Command::new("/bin/false").spawn()
            },
            opusdm_ipc::reaped::wait,
            |_| {},
        );
        assert_eq!(ended, Ended::GaveUp);
        assert_eq!(spawns.get(), MAX_RESTARTS + 1, "the first start plus the restarts");
    }

    #[test]
    fn missing_binary_ends_the_supervisor() {
        let td = tempfile::tempdir().unwrap();
        let bin = opusdm_ipc::instance::sibling_bin_in(Some(td.path()), TOOL);
        let spawns = Cell::new(0);
        let ended = supervise(
            || {
                spawns.set(spawns.get() + 1);
                Command::new(&bin).spawn()
            },
            opusdm_ipc::reaped::wait,
            |_| {},
        );
        assert_eq!(ended, Ended::MissingBinary);
        assert_eq!(spawns.get(), 1);
    }

    #[test]
    fn a_spawn_error_counts_like_a_crash() {
        let spawns = Cell::new(0);
        let ended = supervise(
            || {
                spawns.set(spawns.get() + 1);
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            },
            opusdm_ipc::reaped::wait,
            |_| {},
        );
        assert_eq!(ended, Ended::GaveUp);
        assert_eq!(spawns.get(), MAX_RESTARTS + 1);
    }

    #[test]
    fn restart_delay_doubles_from_one_second() {
        assert_eq!(restart_delay(1), Duration::from_secs(1));
        assert_eq!(restart_delay(2), Duration::from_secs(2));
        assert_eq!(restart_delay(3), Duration::from_secs(4));
    }

    #[test]
    fn spawn_from_dir_targets_the_sibling_binary() {
        let td = tempfile::tempdir().unwrap();
        let err = spawn_from(Some(td.path())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
