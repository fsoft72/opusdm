//! Starting and supervising `opusdm-dock`, the dock process, and sending it
//! the hub's "Hide All" state.
//!
//! Ownership for the whole run: [`DockHost::start`] spawns two detached
//! threads. `opusdm-dock-supervisor` owns the `Child` of `opusdm-dock` and
//! the receiver of the wake channel; `opusdm-dock-link` owns the receiver of
//! the link channel. The returned [`DockHost`] holds the two senders and is
//! owned by the hub's `Widgets` for the whole run. The hub never joins the
//! threads. When the hub exits the threads die with it and the dock keeps
//! running; a restarted hub then sees `EXIT_ALREADY_RUNNING` and waits for
//! the old dock, as with the applets.
//!
//! The supervisor starts the dock only while `dock.json` says `visible`.
//! Exits of the dock:
//! - `EXIT_DISABLED` (the dock was turned off): park until a wake (the hub
//!   watches `dock.json`) finds `visible` again. Not counted as a restart.
//! - `EXIT_ALREADY_RUNNING`: another dock owns the socket; retry every
//!   [`ALREADY_RUNNING_RETRY`].
//! - anything else: restart after a growing delay, at most
//!   [`crate::applet_host::MAX_RESTARTS`] times in [`RESTART_WINDOW`]; past
//!   that, log that it gives up and park until the next change of
//!   `dock.json`.
//! - binary missing: log and end the supervisor.
//!
//! The hub's GTK main loop never talks to the dock socket: [`DockHost::set_hidden`]
//! stores the state and wakes the link thread, which sends it.

use crate::applet_host::{MAX_RESTARTS, RESTART_WINDOW};
use opusdm_ipc::dock::{DockReply, DockRequest};
use opusdm_ipc::instance::{EXIT_ALREADY_RUNNING, EXIT_DISABLED};
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The binary started next to the hub.
const TOOL: &str = "opusdm-dock";

/// Name of the supervisor thread.
const SUPERVISOR_THREAD: &str = "opusdm-dock-supervisor";

/// Name of the thread that sends the hidden state.
const LINK_THREAD: &str = "opusdm-dock-link";

/// Tag used in log lines.
const TAG: &str = "opusdm-hub";

/// How long to wait before trying again when another dock is running.
pub const ALREADY_RUNNING_RETRY: Duration = Duration::from_secs(30);

/// How long a new dock may take to answer `Ping` before it is logged.
const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// How often a new dock is pinged until it answers.
const READY_POLL: Duration = Duration::from_millis(100);

/// How often the supervisor checks whether the dock exited.
const SUPERVISOR_POLL: Duration = Duration::from_millis(500);

/// The delay before the first restart in the window; it doubles for each
/// further one (1 s, 2 s, 4 s).
const RESTART_BACKOFF_BASE: Duration = Duration::from_secs(1);

/// The highest doubling of [`RESTART_BACKOFF_BASE`].
const RESTART_BACKOFF_MAX_SHIFT: usize = 6;

/// The hub's handle on the dock: wakes the supervisor and sends the hidden
/// state. Owned by the hub `Widgets` for the whole run.
pub struct DockHost {
    wake_tx: Sender<()>,
    link_tx: Sender<()>,
    hidden: Arc<AtomicBool>,
    /// Pid of the dock the supervisor started, 0 when none runs.
    pid: Arc<AtomicU32>,
    /// Set by [`DockHost::stop`]: the supervisor starts no dock any more.
    stopping: Arc<AtomicBool>,
}

impl DockHost {
    /// Start the supervisor and link threads. A thread that cannot start is
    /// logged (no dock, or no Hide All for the dock); the hub goes on.
    pub fn start() -> DockHost {
        let (wake_tx, wake_rx) = mpsc::channel();
        let (link_tx, link_rx) = mpsc::channel();
        let hidden = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(false));
        let pid = Arc::new(AtomicU32::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
        let sock = opusdm_ipc::dock::dock_socket_path();

        {
            let (hidden, running) = (hidden.clone(), running.clone());
            let sock = sock.clone();
            let spawned = std::thread::Builder::new().name(LINK_THREAD.into()).spawn(move || {
                link_loop(&link_rx, &hidden, &running, &sock, opusdm_ipc::dock::dock_request_at);
            });
            if let Err(e) = spawned {
                opusdm_ipc::err_log!(TAG, "cannot start the {LINK_THREAD} thread, Hide All will not reach the dock: {e}");
            }
        }
        {
            let link_tx = link_tx.clone();
            let (pid, stopping) = (pid.clone(), stopping.clone());
            // Detached on purpose: the thread owns the child for the whole run.
            let spawned = std::thread::Builder::new().name(SUPERVISOR_THREAD.into()).spawn(move || {
                let ops = Ops {
                    visible: || opusdm_ipc::dock::load_dock_config().visible,
                    spawn: || Command::new(opusdm_ipc::instance::sibling_bin_in(dir.as_deref(), TOOL)).spawn(),
                    run: |child: &mut Child| {
                        pid.store(child.id(), Ordering::SeqCst);
                        let result = run_child(child, &sock, &running, &link_tx);
                        pid.store(0, Ordering::SeqCst);
                        result
                    },
                    sleep: std::thread::sleep,
                    stopping: || stopping.load(Ordering::SeqCst),
                };
                let report = supervise(ops, &wake_rx, PRODUCTION);
                opusdm_ipc::err_log!(TAG, "{SUPERVISOR_THREAD} ended: {report:?}");
            });
            if let Err(e) = spawned {
                opusdm_ipc::err_log!(TAG, "cannot start the {SUPERVISOR_THREAD} thread, no dock: {e}");
            }
        }
        // A dock that outlived the previous hub may still be hidden: tell
        // whoever owns the dock socket the state of this hub (not hidden).
        if link_tx.send(()).is_err() {
            opusdm_ipc::err_log!(TAG, "the {LINK_THREAD} thread is gone: Hide All does not reach the dock");
        }
        DockHost { wake_tx, link_tx, hidden, pid, stopping }
    }

    /// The user quits OpusDM: end the dock this hub started (SIGTERM) and
    /// start no other. A dock started by an earlier hub is left alone.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid == 0 {
            opusdm_ipc::debug_log!(TAG, "quit: no {TOOL} started by this hub to stop");
            return;
        }
        // SAFETY: kill only sends a signal to a pid; it touches no memory.
        if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
            opusdm_ipc::err_log!(TAG, "quit: cannot stop {TOOL} (pid {pid}): {}", io::Error::last_os_error());
        } else {
            opusdm_ipc::debug_log!(TAG, "quit: {TOOL} (pid {pid}) asked to end");
        }
    }

    /// `dock.json` changed: let the supervisor check whether the dock must
    /// be started.
    pub fn wake(&self) {
        if self.wake_tx.send(()).is_err() {
            opusdm_ipc::err_log!(TAG, "the dock supervisor is gone: a change of dock.json cannot start the dock");
        }
    }

    /// The hub hid (`true`) or revealed the desktop chrome: tell the dock.
    pub fn set_hidden(&self, hidden: bool) {
        self.hidden.store(hidden, Ordering::Relaxed);
        if self.link_tx.send(()).is_err() {
            opusdm_ipc::err_log!(TAG, "the {LINK_THREAD} thread is gone: Hide All does not reach the dock");
        }
    }
}

/// The durations the supervisor uses; short ones in tests.
#[derive(Debug, Clone, Copy)]
struct Timing {
    /// The span restarts are counted over.
    window: Duration,
    /// The wait after an "already running" exit.
    retry: Duration,
}

/// The durations used by the running hub.
const PRODUCTION: Timing = Timing { window: RESTART_WINDOW, retry: ALREADY_RUNNING_RETRY };

/// What the supervisor does after the dock exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Start it again after this delay.
    Restart(Duration),
    /// Another dock is running: wait this long, then try again.
    WaitAndRetry(Duration),
    /// The dock was turned off: wait for `dock.json` to turn it on.
    Park,
    /// Too many restarts in the window.
    GiveUp,
}

/// The step after the dock exited with `status` (`None`: killed by a
/// signal, or it could not be started or waited for).
fn next_step(status: Option<i32>, history: &mut VecDeque<Instant>, now: Instant, timing: Timing) -> Step {
    match status {
        Some(EXIT_DISABLED) => Step::Park,
        Some(EXIT_ALREADY_RUNNING) => Step::WaitAndRetry(timing.retry),
        _ => counted_restart(history, now, timing),
    }
}

/// A restart that counts toward the limit, or [`Step::GiveUp`].
fn counted_restart(history: &mut VecDeque<Instant>, now: Instant, timing: Timing) -> Step {
    while history.front().is_some_and(|t| now.saturating_duration_since(*t) > timing.window) {
        history.pop_front();
    }
    if history.len() >= MAX_RESTARTS {
        return Step::GiveUp;
    }
    history.push_back(now);
    Step::Restart(restart_delay(history.len()))
}

/// The delay before the `n`-th restart in the window (1-based): 1 s, 2 s,
/// 4 s... `0` is treated as the first.
fn restart_delay(n: usize) -> Duration {
    let shift = n.saturating_sub(1).min(RESTART_BACKOFF_MAX_SHIFT) as u32;
    RESTART_BACKOFF_BASE * (1u32 << shift)
}

/// Why the supervisor loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The binary does not exist.
    MissingBinary,
    /// The wake channel closed: the hub is shutting down.
    HubGone,
    /// The user quit OpusDM ([`DockHost::stop`]).
    Stopped,
}

/// What a supervisor run did, for the logs and the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Report {
    ended: Ended,
    /// How many times the dock was started.
    starts: usize,
}

/// What the supervisor loop calls; fakes in the tests.
struct Ops<V, S, R, Sl, St> {
    /// Whether `dock.json` asks for the dock now.
    visible: V,
    /// Start the dock process.
    spawn: S,
    /// Run the started dock until it exits.
    run: R,
    /// Wait before a retry or a restart.
    sleep: Sl,
    /// Whether the hub is quitting: no more starts.
    stopping: St,
}

/// The supervisor loop, see the module documentation.
fn supervise<V, S, R, Sl, St>(mut ops: Ops<V, S, R, Sl, St>, wake_rx: &Receiver<()>, timing: Timing) -> Report
where
    V: FnMut() -> bool,
    S: FnMut() -> io::Result<Child>,
    R: FnMut(&mut Child) -> io::Result<ExitStatus>,
    Sl: FnMut(Duration),
    St: FnMut() -> bool,
{
    let mut history = VecDeque::new();
    let mut waiting = false;
    let mut starts = 0;
    loop {
        if !park_until_visible(&mut ops.visible, wake_rx) {
            return Report { ended: Ended::HubGone, starts };
        }
        // Woken while the hub quits: start nothing.
        if (ops.stopping)() {
            return Report { ended: Ended::Stopped, starts };
        }
        starts += 1;
        let status = match (ops.spawn)() {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                opusdm_ipc::err_log!(TAG, "cannot start {TOOL}, not found: {e}; no dock until the hub is restarted");
                return Report { ended: Ended::MissingBinary, starts };
            }
            Err(e) => {
                opusdm_ipc::err_log!(TAG, "cannot start {TOOL}: {e}");
                None
            }
            Ok(mut child) => {
                let pid = child.id();
                opusdm_ipc::debug_log!(TAG, "{TOOL} started (pid {pid})");
                log_exit(pid, (ops.run)(&mut child))
            }
        };
        if (ops.stopping)() {
            return Report { ended: Ended::Stopped, starts };
        }
        let mut step = next_step(status, &mut history, Instant::now(), timing);
        // Turned off, but dock.json says on again already: a quick toggle,
        // or the dock and the hub do not agree on the file. Count it, so a
        // disagreement cannot turn into a tight loop.
        if step == Step::Park && (ops.visible)() {
            opusdm_ipc::err_log!(TAG, "{TOOL} exited as turned off, but dock.json asks for it: starting it again");
            step = counted_restart(&mut history, Instant::now(), timing);
        }
        match step {
            Step::Restart(delay) => {
                waiting = false;
                opusdm_ipc::err_log!(TAG, "restarting {TOOL} in {}s", delay.as_secs());
                (ops.sleep)(delay);
            }
            Step::WaitAndRetry(delay) => {
                if !waiting {
                    opusdm_ipc::err_log!(TAG, "{TOOL} already running, waiting for it (retry every {}s)", delay.as_secs());
                }
                waiting = true;
                (ops.sleep)(delay);
            }
            Step::Park => {
                waiting = false;
                opusdm_ipc::debug_log!(TAG, "{TOOL} is turned off in dock.json: waiting for it to be turned on");
            }
            Step::GiveUp => {
                waiting = false;
                opusdm_ipc::err_log!(
                    TAG,
                    "{TOOL} exited more than {MAX_RESTARTS} times in {}s, giving up: no dock until dock.json changes or the hub is restarted",
                    timing.window.as_secs()
                );
                history.clear();
                if !wait_for_fresh_wake(wake_rx) {
                    return Report { ended: Ended::HubGone, starts };
                }
            }
        }
    }
}

/// Return once `visible` is true, waiting for wakes while it is false.
/// `false` when the wake channel closed.
fn park_until_visible(visible: &mut impl FnMut() -> bool, wake_rx: &Receiver<()>) -> bool {
    loop {
        // Wakes queued while the dock ran are stale: the check below reads
        // the file as it is now.
        loop {
            match wake_rx.try_recv() {
                Ok(()) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        if visible() {
            return true;
        }
        if wake_rx.recv().is_err() {
            return false;
        }
        opusdm_ipc::debug_log!(TAG, "dock.json changed: checking whether to start {TOOL}");
    }
}

/// Drop the queued wakes, then wait for a new one. `false` when the wake
/// channel closed.
fn wait_for_fresh_wake(wake_rx: &Receiver<()>) -> bool {
    loop {
        match wake_rx.try_recv() {
            Ok(()) => {}
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
    wake_rx.recv().is_ok()
}

/// Log how the dock `pid` ended. Returns the exit code, `None` when it was
/// killed by a signal or the wait failed.
fn log_exit(pid: u32, result: io::Result<ExitStatus>) -> Option<i32> {
    let status = match result {
        Ok(status) => status,
        Err(e) => {
            opusdm_ipc::err_log!(TAG, "cannot wait for {TOOL} (pid {pid}): {e}");
            return None;
        }
    };
    match status.code() {
        Some(EXIT_ALREADY_RUNNING) => opusdm_ipc::debug_log!(TAG, "{TOOL} (pid {pid}) exited: {status} (already running)"),
        Some(EXIT_DISABLED) => opusdm_ipc::debug_log!(TAG, "{TOOL} (pid {pid}) exited: {status} (turned off)"),
        _ => opusdm_ipc::err_log!(TAG, "{TOOL} (pid {pid}) exited: {status}"),
    }
    status.code()
}

/// Run the started dock `child` until it exits: wait for it to answer on
/// `sock`, push the hidden state through `link_tx`, then wait for the exit.
/// `running` is true while it answers.
fn run_child(child: &mut Child, sock: &Path, running: &AtomicBool, link_tx: &Sender<()>) -> io::Result<ExitStatus> {
    let started = Instant::now();
    let mut lost_since = None;
    let mut ready = false;
    while started.elapsed() < READY_TIMEOUT {
        if let Some(status) = opusdm_ipc::reaped::poll_exit(child, &mut lost_since)? {
            return Ok(status);
        }
        if matches!(opusdm_ipc::dock::dock_request_at(sock, &DockRequest::Ping), Ok(DockReply::Hidden(_))) {
            ready = true;
            break;
        }
        std::thread::sleep(READY_POLL);
    }
    if ready {
        opusdm_ipc::debug_log!(TAG, "{} answers after {TOOL} (pid {}) started", sock.display(), child.id());
        running.store(true, Ordering::Relaxed);
        if link_tx.send(()).is_err() {
            opusdm_ipc::err_log!(TAG, "the {LINK_THREAD} thread is gone: the restarted dock does not get the Hide All state");
        }
    } else {
        opusdm_ipc::err_log!(TAG, "{TOOL} (pid {}) did not answer on {} within {}s: Hide All may not reach it", child.id(), sock.display(), READY_TIMEOUT.as_secs());
    }
    let result = wait_polling(child, &mut lost_since);
    running.store(false, Ordering::Relaxed);
    result
}

/// Block until `child` exits, checking every [`SUPERVISOR_POLL`] (safe
/// against the SIGCHLD reaper taking the child first).
fn wait_polling(child: &mut Child, lost_since: &mut Option<Instant>) -> io::Result<ExitStatus> {
    loop {
        if let Some(status) = opusdm_ipc::reaped::poll_exit(child, lost_since)? {
            return Ok(status);
        }
        std::thread::sleep(SUPERVISOR_POLL);
    }
}

/// The link thread: on every message, send the current `hidden` state to
/// the dock socket at `sock` with `request`. A failure is an error while the
/// dock is `running`, and a debug line otherwise (turned off, starting, or
/// another dock being waited for).
fn link_loop(
    rx: &Receiver<()>,
    hidden: &AtomicBool,
    running: &AtomicBool,
    sock: &Path,
    request: impl Fn(&Path, &DockRequest) -> io::Result<DockReply>,
) {
    while rx.recv().is_ok() {
        // Several changes queued: only the latest state matters.
        while rx.try_recv().is_ok() {}
        let want = hidden.load(Ordering::Relaxed);
        match request(sock, &DockRequest::SetHidden(want)) {
            Ok(DockReply::Hidden(got)) if got == want => opusdm_ipc::debug_log!(TAG, "dock {}", if want { "hidden" } else { "revealed" }),
            Ok(other) => opusdm_ipc::err_log!(TAG, "the dock answered {other:?} to SetHidden({want})"),
            Err(e) if running.load(Ordering::Relaxed) => opusdm_ipc::err_log!(TAG, "cannot tell the dock SetHidden({want}): {e}"),
            Err(e) => opusdm_ipc::debug_log!(TAG, "dock not running, SetHidden({want}) not sent: {e}"),
        }
    }
    opusdm_ipc::debug_log!(TAG, "{LINK_THREAD} ended: the hub dropped its dock handle");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::sync::Mutex;

    /// Short durations so the process tests take milliseconds.
    const FAST: Timing = Timing { window: Duration::from_secs(30), retry: Duration::from_millis(1) };

    /// A shell exiting with `code`.
    fn sh_exit(code: i32) -> io::Result<Child> {
        Command::new("sh").arg("-c").arg(format!("exit {code}")).spawn()
    }

    #[test]
    fn disabled_parks_already_running_retries_and_the_rest_restarts() {
        let mut history = VecDeque::new();
        let now = Instant::now();
        assert_eq!(next_step(Some(EXIT_DISABLED), &mut history, now, FAST), Step::Park);
        assert_eq!(next_step(Some(EXIT_ALREADY_RUNNING), &mut history, now, FAST), Step::WaitAndRetry(FAST.retry));
        assert!(history.is_empty(), "neither is counted");
        assert_eq!(next_step(Some(1), &mut history, now, FAST), Step::Restart(Duration::from_secs(1)));
        assert_eq!(next_step(None, &mut history, now, FAST), Step::Restart(Duration::from_secs(2)));
        assert_eq!(next_step(Some(0), &mut history, now, FAST), Step::Restart(Duration::from_secs(4)));
        assert_eq!(next_step(Some(1), &mut history, now, FAST), Step::GiveUp);
    }

    #[test]
    fn a_dock_turned_off_at_start_is_not_started_until_a_wake_finds_it_on() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let visible = Arc::new(AtomicBool::new(false));
        let spawns = Arc::new(Mutex::new(0));
        let handle = {
            let (visible, spawns) = (visible.clone(), spawns.clone());
            std::thread::spawn(move || {
                let ops = Ops {
                    visible: || visible.load(Ordering::SeqCst),
                    spawn: || {
                        *spawns.lock().unwrap() += 1;
                        // Turned off again by the "user" right away.
                        visible.store(false, Ordering::SeqCst);
                        sh_exit(EXIT_DISABLED)
                    },
                    run: opusdm_ipc::reaped::wait,
                    sleep: |_| {},
                    stopping: || false,
                };
                supervise(ops, &wake_rx, FAST)
            })
        };
        // A wake while it is still off starts nothing.
        wake_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(*spawns.lock().unwrap(), 0);
        // Turned on: the next wake starts it.
        visible.store(true, Ordering::SeqCst);
        wake_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while *spawns.lock().unwrap() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*spawns.lock().unwrap(), 1);
        // The hub goes away: the supervisor ends.
        drop(wake_tx);
        let report = handle.join().unwrap();
        assert_eq!(report, Report { ended: Ended::HubGone, starts: 1 });
    }

    /// Drop `wake_tx` after a while, from another thread: ends a supervisor
    /// that waits for a wake after giving up.
    fn close_later(wake_tx: Sender<()>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(wake_tx);
        })
    }

    #[test]
    fn a_crashing_dock_is_restarted_then_given_up_until_a_wake() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let spawns = Cell::new(0);
        let sleeps = RefCell::new(Vec::new());
        let ops = Ops {
            visible: || true,
            spawn: || {
                spawns.set(spawns.get() + 1);
                Command::new("/bin/false").spawn()
            },
            run: opusdm_ipc::reaped::wait,
            sleep: |d| sleeps.borrow_mut().push(d),
            stopping: || false,
        };
        let closer = close_later(wake_tx);
        let report = supervise(ops, &wake_rx, FAST);
        closer.join().unwrap();
        assert_eq!(report, Report { ended: Ended::HubGone, starts: MAX_RESTARTS + 1 }, "no start after giving up");
        assert_eq!(*sleeps.borrow(), [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4)]);
    }

    #[test]
    fn after_giving_up_a_wake_starts_the_dock_again() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let spawns = Arc::new(Mutex::new(0));
        let handle = {
            let spawns = spawns.clone();
            std::thread::spawn(move || {
                let ops = Ops {
                    visible: || true,
                    spawn: || {
                        *spawns.lock().unwrap() += 1;
                        Command::new("/bin/false").spawn()
                    },
                    run: opusdm_ipc::reaped::wait,
                    sleep: |_| {},
                    stopping: || false,
                };
                supervise(ops, &wake_rx, FAST)
            })
        };
        let wait_for = |n: usize| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while *spawns.lock().unwrap() < n && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            *spawns.lock().unwrap()
        };
        assert_eq!(wait_for(MAX_RESTARTS + 1), MAX_RESTARTS + 1);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(*spawns.lock().unwrap(), MAX_RESTARTS + 1, "given up: no more starts");
        wake_tx.send(()).unwrap();
        assert_eq!(wait_for(MAX_RESTARTS + 2), MAX_RESTARTS + 2, "a change of dock.json starts it again");
        drop(wake_tx);
        let report = handle.join().unwrap();
        assert_eq!(report.ended, Ended::HubGone);
    }

    #[test]
    fn a_turned_off_exit_while_dock_json_says_on_is_counted() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let spawns = Cell::new(0);
        let ops = Ops {
            visible: || true,
            spawn: || {
                spawns.set(spawns.get() + 1);
                sh_exit(EXIT_DISABLED)
            },
            run: opusdm_ipc::reaped::wait,
            sleep: |_| {},
            stopping: || false,
        };
        let closer = close_later(wake_tx);
        // Without the count this would spin forever.
        let report = supervise(ops, &wake_rx, FAST);
        closer.join().unwrap();
        assert_eq!(report, Report { ended: Ended::HubGone, starts: MAX_RESTARTS + 1 });
    }

    #[test]
    fn already_running_retries_without_limit() {
        let (_wake_tx, wake_rx) = mpsc::channel();
        let spawns = Cell::new(0);
        let sleeps = RefCell::new(Vec::new());
        let ops = Ops {
            visible: || true,
            spawn: || {
                spawns.set(spawns.get() + 1);
                if spawns.get() <= 10 { sh_exit(EXIT_ALREADY_RUNNING) } else { Err(io::Error::from(io::ErrorKind::NotFound)) }
            },
            run: opusdm_ipc::reaped::wait,
            sleep: |d| sleeps.borrow_mut().push(d),
            stopping: || false,
        };
        let report = supervise(ops, &wake_rx, FAST);
        assert_eq!(report, Report { ended: Ended::MissingBinary, starts: 11 });
        assert_eq!(*sleeps.borrow(), vec![FAST.retry; 10]);
    }

    #[test]
    fn a_quit_ends_the_supervisor_instead_of_restarting_the_dock() {
        let (_wake_tx, wake_rx) = mpsc::channel();
        let spawns = Cell::new(0);
        let stopping = Cell::new(false);
        let ops = Ops {
            visible: || true,
            spawn: || {
                spawns.set(spawns.get() + 1);
                // The hub quits while this dock runs; it ends by the signal.
                stopping.set(true);
                Command::new("/bin/false").spawn()
            },
            run: opusdm_ipc::reaped::wait,
            sleep: |_| panic!("no restart after a quit"),
            stopping: || stopping.get(),
        };
        assert_eq!(supervise(ops, &wake_rx, FAST), Report { ended: Ended::Stopped, starts: 1 });
        assert_eq!(spawns.get(), 1);
    }

    #[test]
    fn the_link_sends_the_latest_state_and_logs_failures() {
        let (tx, rx) = mpsc::channel();
        let hidden = AtomicBool::new(false);
        let running = AtomicBool::new(true);
        let sent = RefCell::new(Vec::new());
        hidden.store(true, Ordering::Relaxed);
        tx.send(()).unwrap();
        tx.send(()).unwrap();
        drop(tx);
        link_loop(&rx, &hidden, &running, Path::new("/nowhere"), |_, req| {
            sent.borrow_mut().push(*req);
            match req {
                DockRequest::SetHidden(h) => Ok(DockReply::Hidden(*h)),
                DockRequest::Ping => Ok(DockReply::Hidden(false)),
            }
        });
        assert_eq!(*sent.borrow(), vec![DockRequest::SetHidden(true)], "two queued changes, one send of the latest state");
    }

    #[test]
    fn the_link_survives_a_dock_that_is_not_there() {
        let (tx, rx) = mpsc::channel();
        let calls = Arc::new(Mutex::new(0));
        let link = {
            let calls = calls.clone();
            std::thread::spawn(move || {
                let (hidden, running) = (AtomicBool::new(true), AtomicBool::new(true));
                link_loop(&rx, &hidden, &running, Path::new("/nowhere"), |_, _| {
                    let mut n = calls.lock().unwrap();
                    *n += 1;
                    if *n == 1 { Err(io::Error::from(io::ErrorKind::NotFound)) } else { Ok(DockReply::Hidden(true)) }
                });
            })
        };
        let wait_for = |n: usize| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while *calls.lock().unwrap() < n && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        tx.send(()).unwrap();
        wait_for(1);
        // The first send failed; the thread still serves the next one.
        tx.send(()).unwrap();
        wait_for(2);
        drop(tx);
        link.join().unwrap();
        assert_eq!(*calls.lock().unwrap(), 2);
    }

    #[test]
    fn the_real_supervisor_polls_slowly_and_waits_long_for_another_dock() {
        assert_eq!(SUPERVISOR_POLL, Duration::from_millis(500));
        assert_eq!(PRODUCTION.retry, Duration::from_secs(30));
    }
}
