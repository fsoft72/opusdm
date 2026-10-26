//! Starting and supervising `opusdm-applets`, the process that shows the
//! desktop applets (X11 only in this version).
//!
//! Ownership for the whole run: [`start`] spawns the detached thread
//! `opusdm-applets-supervisor`, which owns the `Child` of `opusdm-applets`.
//! The hub never joins or waits for that thread and no hub object holds a
//! handle to it or to the child. When the child exits, the supervisor logs
//! the status and starts it again, up to [`MAX_RESTARTS`] times in
//! [`RESTART_WINDOW`]; past that it logs once that it gives up and the thread
//! ends. An exit with [`EXIT_ALREADY_RUNNING`] (another `opusdm-applets` owns
//! the socket, the hub was restarted while the old one lived) is not a
//! failure: the supervisor waits [`ALREADY_RUNNING_RETRY`] and tries again,
//! without limit, so the applets come back when the old process dies.
//! When the hub process exits the thread dies with it and the child keeps
//! running; a restarted hub then sees `EXIT_ALREADY_RUNNING` and waits.

use opusdm_ipc::instance::EXIT_ALREADY_RUNNING;
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// How many restarts are allowed inside [`RESTART_WINDOW`].
pub const MAX_RESTARTS: usize = 3;

/// The time span [`MAX_RESTARTS`] is counted over.
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// How long to wait before trying again when another `opusdm-applets` is
/// already running.
pub const ALREADY_RUNNING_RETRY: Duration = Duration::from_secs(30);

/// How long a child started after an "already running" streak must live to
/// count as having taken over the socket (the second instance exits at once).
const TAKEOVER_GRACE: Duration = Duration::from_secs(2);

/// How often the take-over check polls the child.
const TAKEOVER_POLL: Duration = Duration::from_millis(10);

/// The binary started next to the hub.
const TOOL: &str = "opusdm-applets";

/// Name of the supervisor thread.
const THREAD_NAME: &str = "opusdm-applets-supervisor";

/// Tag used in log lines.
const TAG: &str = "opusdm-hub";

/// What the supervisor does after `opusdm-applets` exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Start it again now.
    Restart,
    /// Another instance is running: wait this long, then try again.
    WaitAndRetry(Duration),
    /// Too many restarts in the window: stop supervising.
    GiveUp,
}

/// The durations the supervisor uses; short ones in tests.
#[derive(Debug, Clone, Copy)]
struct Timing {
    /// The span restarts are counted over.
    window: Duration,
    /// The wait after an "already running" exit.
    retry: Duration,
    /// How long a child must live to count as having taken over.
    takeover_grace: Duration,
}

/// The durations used by the running hub.
const PRODUCTION: Timing = Timing { window: RESTART_WINDOW, retry: ALREADY_RUNNING_RETRY, takeover_grace: TAKEOVER_GRACE };

/// Why the supervisor loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The binary does not exist.
    MissingBinary,
    /// Too many restarts in the window.
    GaveUp,
}

/// What a supervisor run did, for the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Report {
    /// Why the loop ended.
    ended: Ended,
    /// How many children took over after an "already running" streak.
    takeovers: usize,
}

/// Whether one more restart is allowed at `now`. Drops the entries older
/// than [`RESTART_WINDOW`], then allows the restart while fewer than
/// [`MAX_RESTARTS`] are left, and records it.
pub fn may_restart(history: &mut VecDeque<Instant>, now: Instant) -> bool {
    may_restart_within(history, now, RESTART_WINDOW)
}

/// [`may_restart`] with the window given.
fn may_restart_within(history: &mut VecDeque<Instant>, now: Instant, window: Duration) -> bool {
    while history.front().is_some_and(|t| now.saturating_duration_since(*t) > window) {
        history.pop_front();
    }
    if history.len() >= MAX_RESTARTS { return false; }
    history.push_back(now);
    true
}

/// The decision after `opusdm-applets` exited with `status` (the exit code,
/// `None` when killed by a signal or when it could not be started).
/// [`EXIT_ALREADY_RUNNING`] is not counted as a restart.
pub fn next_action(status: Option<i32>, history: &mut VecDeque<Instant>, now: Instant) -> Action {
    next_action_with(status, history, now, PRODUCTION)
}

/// [`next_action`] with the durations given.
fn next_action_with(status: Option<i32>, history: &mut VecDeque<Instant>, now: Instant, timing: Timing) -> Action {
    if status == Some(EXIT_ALREADY_RUNNING) { return Action::WaitAndRetry(timing.retry); }
    if may_restart_within(history, now, timing.window) { Action::Restart } else { Action::GiveUp }
}

/// Start the supervisor thread of `opusdm-applets` (X11 only: on Wayland it
/// only logs at debug level). Returns at once; the hub never waits for it.
pub fn start() {
    if opusdm_ipc::session::is_wayland() {
        opusdm_ipc::debug_log!(TAG, "{TOOL} not started: the applets are X11 only");
        return;
    }
    let dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    // Detached on purpose: the handle is dropped, the hub never joins it.
    // The thread owns the child; when the hub exits the thread dies with it
    // and the child keeps running (a restarted hub then waits for it).
    let spawned = std::thread::Builder::new().name(THREAD_NAME.into()).spawn(move || {
        let report = supervise(|| spawn_from(dir.as_deref()), opusdm_ipc::reaped::wait, std::thread::sleep, PRODUCTION);
        opusdm_ipc::debug_log!(TAG, "{THREAD_NAME} ended: {report:?}");
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(TAG, "cannot start the {THREAD_NAME} thread, no applets: {e}");
    }
}

/// Start `opusdm-applets` from `dir` (the hub's folder), or by `PATH` when
/// the folder is unknown. The child inherits the hub's environment.
fn spawn_from(dir: Option<&Path>) -> io::Result<Child> {
    Command::new(opusdm_ipc::instance::sibling_bin_in(dir, TOOL)).spawn()
}

/// The supervisor loop: start the child with `spawn`, wait for it with
/// `wait`, log the exit and act on [`next_action_with`]; `sleep` waits
/// before an "already running" retry.
fn supervise(
    mut spawn: impl FnMut() -> io::Result<Child>,
    mut wait: impl FnMut(&mut Child) -> io::Result<ExitStatus>,
    mut sleep: impl FnMut(Duration),
    timing: Timing,
) -> Report {
    let mut history = VecDeque::new();
    // Inside a streak of "already running" exits.
    let mut waiting = false;
    let mut takeovers = 0;
    loop {
        let status = match spawn() {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                opusdm_ipc::err_log!(TAG, "cannot start {TOOL}, not found: {e}; no applets until the hub is restarted");
                return Report { ended: Ended::MissingBinary, takeovers };
            }
            Err(e) => {
                opusdm_ipc::err_log!(TAG, "cannot start {TOOL}: {e}");
                None
            }
            Ok(mut child) => {
                let pid = child.id();
                let early = if waiting { _exit_within(&mut child, timing.takeover_grace) } else { None };
                let result = match early {
                    Some(result) => result,
                    None => {
                        if waiting {
                            opusdm_ipc::err_log!(TAG, "{TOOL} (pid {pid}) took over after the old one exited");
                            waiting = false;
                            takeovers += 1;
                        }
                        wait(&mut child)
                    }
                };
                _log_exit(pid, result)
            }
        };
        match next_action_with(status, &mut history, Instant::now(), timing) {
            Action::Restart => {
                waiting = false;
                opusdm_ipc::err_log!(TAG, "restarting {TOOL}");
            }
            Action::WaitAndRetry(delay) => {
                if !waiting {
                    opusdm_ipc::err_log!(TAG, "{TOOL} already running, waiting for it (retry every {}s)", delay.as_secs());
                }
                waiting = true;
                sleep(delay);
            }
            Action::GiveUp => {
                opusdm_ipc::err_log!(TAG, "{TOOL} exited more than {MAX_RESTARTS} times in {}s, giving up: no applets until the hub is restarted", timing.window.as_secs());
                return Report { ended: Ended::GaveUp, takeovers };
            }
        }
    }
}

/// Log how the child `pid` ended. Returns the exit code, `None` when it was
/// killed by a signal or the wait failed. An "already running" exit is
/// logged at debug level only: the caller logs the streak once.
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

/// The exit of `child` if it ends within `grace`, `None` while it is still
/// running then (it took over the socket).
fn _exit_within(child: &mut Child, grace: Duration) -> Option<io::Result<ExitStatus>> {
    let started = Instant::now();
    let mut lost_since = None;
    while started.elapsed() < grace {
        match opusdm_ipc::reaped::poll_exit(child, &mut lost_since) {
            Ok(None) => std::thread::sleep(TAKEOVER_POLL),
            Ok(Some(status)) => return Some(Ok(status)),
            Err(e) => return Some(Err(e)),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// Short durations so the process tests take milliseconds.
    const FAST: Timing = Timing {
        window: Duration::from_secs(30),
        retry: Duration::from_millis(1),
        takeover_grace: Duration::from_millis(50),
    };

    /// A shell exiting with `code`.
    fn sh_exit(code: i32) -> io::Result<Child> {
        Command::new("sh").arg("-c").arg(format!("exit {code}")).spawn()
    }

    #[test]
    fn may_restart_allows_three_in_a_minute_then_refuses() {
        let mut history = VecDeque::new();
        let t0 = Instant::now();
        for i in 0..MAX_RESTARTS {
            assert!(may_restart(&mut history, t0 + Duration::from_secs(i as u64)), "restart {i}");
        }
        assert!(!may_restart(&mut history, t0 + Duration::from_secs(10)));
        assert_eq!(history.len(), MAX_RESTARTS, "a refused restart is not recorded");
    }

    #[test]
    fn may_restart_forgets_restarts_older_than_the_window() {
        let mut history = VecDeque::new();
        let t0 = Instant::now();
        for _ in 0..MAX_RESTARTS {
            assert!(may_restart(&mut history, t0));
        }
        assert!(may_restart(&mut history, t0 + RESTART_WINDOW + Duration::from_secs(1)));
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn already_running_is_not_counted_as_a_restart() {
        let mut history = VecDeque::new();
        let t0 = Instant::now();
        for i in 0..10 {
            let action = next_action(Some(EXIT_ALREADY_RUNNING), &mut history, t0 + Duration::from_millis(i));
            assert_eq!(action, Action::WaitAndRetry(ALREADY_RUNNING_RETRY));
        }
        assert!(history.is_empty());
    }

    #[test]
    fn a_crash_after_an_already_running_streak_is_counted_normally() {
        let mut history = VecDeque::new();
        let now = Instant::now();
        for _ in 0..5 {
            assert_eq!(next_action(Some(EXIT_ALREADY_RUNNING), &mut history, now), Action::WaitAndRetry(ALREADY_RUNNING_RETRY));
        }
        assert_eq!(next_action(Some(1), &mut history, now), Action::Restart);
        assert_eq!(history.len(), 1);
        assert_eq!(next_action(None, &mut history, now), Action::Restart);
        assert_eq!(next_action(Some(0), &mut history, now), Action::Restart);
        assert_eq!(next_action(Some(1), &mut history, now), Action::GiveUp);
    }

    #[test]
    fn the_supervisor_restarts_a_child_that_exits_and_gives_up() {
        let spawns = Cell::new(0);
        let sleeps = RefCell::new(Vec::new());
        let report = supervise(
            || {
                spawns.set(spawns.get() + 1);
                Command::new("/bin/false").spawn()
            },
            opusdm_ipc::reaped::wait,
            |d| sleeps.borrow_mut().push(d),
            FAST,
        );
        assert_eq!(report, Report { ended: Ended::GaveUp, takeovers: 0 });
        assert_eq!(spawns.get(), MAX_RESTARTS + 1, "the first start plus the restarts");
        assert!(sleeps.borrow().is_empty(), "a crash is restarted at once");
    }

    #[test]
    fn the_supervisor_keeps_retrying_while_already_running() {
        let spawns = Cell::new(0);
        let sleeps = RefCell::new(Vec::new());
        let report = supervise(
            || {
                spawns.set(spawns.get() + 1);
                match spawns.get() {
                    1..=5 => sh_exit(EXIT_ALREADY_RUNNING),
                    6 => Command::new("sleep").arg("0.2").spawn(),
                    _ => Err(io::Error::from(io::ErrorKind::NotFound)),
                }
            },
            opusdm_ipc::reaped::wait,
            |d| sleeps.borrow_mut().push(d),
            FAST,
        );
        assert_eq!(report, Report { ended: Ended::MissingBinary, takeovers: 1 }, "no give-up");
        assert_eq!(spawns.get(), 7);
        assert_eq!(*sleeps.borrow(), vec![FAST.retry; 5]);
    }

    #[test]
    fn a_spawn_error_counts_like_a_crash() {
        let spawns = Cell::new(0);
        let report = supervise(
            || {
                spawns.set(spawns.get() + 1);
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            },
            opusdm_ipc::reaped::wait,
            |_| {},
            FAST,
        );
        assert_eq!(report.ended, Ended::GaveUp);
        assert_eq!(spawns.get(), MAX_RESTARTS + 1);
    }

    #[test]
    fn a_missing_binary_is_logged_and_does_not_panic() {
        let td = tempfile::tempdir().unwrap();
        let bin = opusdm_ipc::instance::sibling_bin_in(Some(td.path()), TOOL);
        let spawns = Cell::new(0);
        let report = supervise(
            || {
                spawns.set(spawns.get() + 1);
                Command::new(&bin).spawn()
            },
            opusdm_ipc::reaped::wait,
            |_| {},
            FAST,
        );
        assert_eq!(report, Report { ended: Ended::MissingBinary, takeovers: 0 });
        assert_eq!(spawns.get(), 1);
    }

    #[test]
    fn spawn_from_dir_targets_the_sibling_binary() {
        let td = tempfile::tempdir().unwrap();
        let err = spawn_from(Some(td.path())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
