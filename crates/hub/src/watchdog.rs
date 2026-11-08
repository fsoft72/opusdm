//! Main-loop watchdog: notices when the hub's GTK main thread stops turning
//! and leaves evidence behind.
//!
//! The panel indicators, the menu, the dock and the desktop all live on the
//! GTK main thread. When it blocks (a deadlock, a call that never returns)
//! everything freezes and nothing is logged. A timeout on the main loop
//! stamps a [`Heartbeat`] every second; a separate thread checks it and, when
//! it has been silent for [`STALL_AFTER`], logs it with `err_log!` and writes
//! a report under `~/.local/state/opusdm/hub-stalls/` with the state of every
//! thread and their stacks (gdb, or eu-stack). When the loop runs again the
//! recovery is logged too.
//!
//! Ownership: the heartbeat is shared (`Arc`) by the main-loop timeout, that
//! never stops, and by the watchdog thread, that loops for the whole run. If
//! the timeout is gone (the GTK loop ended) the heartbeat ages and a stall is
//! reported, which is the visible outcome we want.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk4::glib;

/// Tag of the watchdog's log lines.
const TAG: &str = "opusdm-hub";

/// How often the main loop stamps the heartbeat.
const BEAT_INTERVAL: Duration = Duration::from_secs(1);

/// How often the watchdog thread looks at the heartbeat.
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// A main loop silent for this long is stalled.
pub const STALL_AFTER: Duration = Duration::from_secs(10);

/// Reports written per run at most, so a hub that stalls often does not fill
/// the disk. Further stalls are still logged.
const MAX_REPORTS: u32 = 5;

/// How long a stack dumper may take before it is asked to stop.
const DUMP_TIMEOUT: Duration = Duration::from_secs(60);

/// How long to wait for a dumper to exit after asking it to stop.
const DUMP_STOP_GRACE: Duration = Duration::from_secs(5);

/// How often a running dumper is polled.
const DUMP_POLL: Duration = Duration::from_millis(100);

/// Folder, under the OpusDM state dir, of the stall reports.
const REPORT_DIR: &str = "hub-stalls";

/// Set by the first [`start`]; later calls do nothing.
static STARTED: AtomicBool = AtomicBool::new(false);

/// The last moment the main loop proved it was running.
pub struct Heartbeat {
    origin: Instant,
    last_ms: AtomicU64,
}

impl Heartbeat {
    /// A heartbeat that has just beaten.
    pub fn new() -> Self {
        Self { origin: Instant::now(), last_ms: AtomicU64::new(0) }
    }

    /// Record that the main loop is running now.
    pub fn beat(&self) {
        self.last_ms.store(self.origin.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// Time since the last [`beat`](Self::beat).
    pub fn age(&self) -> Duration {
        let last = Duration::from_millis(self.last_ms.load(Ordering::Relaxed));
        self.origin.elapsed().saturating_sub(last)
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

/// A change seen by [`StallTracker::observe`].
#[derive(Debug, PartialEq, Eq)]
pub enum StallEvent {
    /// The main loop has been silent for this long: a new stall.
    Stalled(Duration),
    /// The main loop runs again after a stall of about this long.
    Recovered(Duration),
}

/// Turns heartbeat ages into stall and recovery events, one of each per stall.
#[derive(Debug, Default)]
pub struct StallTracker {
    stalled: bool,
    longest: Duration,
}

impl StallTracker {
    /// Look at the heartbeat `age`; returns an event when the state changes.
    pub fn observe(&mut self, age: Duration) -> Option<StallEvent> {
        if !self.stalled && age < STALL_AFTER { return None; }
        if !self.stalled {
            self.stalled = true;
            self.longest = age;
            return Some(StallEvent::Stalled(age));
        }
        if age >= STALL_AFTER {
            self.longest = self.longest.max(age);
            return None;
        }
        self.stalled = false;
        Some(StallEvent::Recovered(self.longest))
    }
}

/// Start the watchdog: a heartbeat timeout on the current (GTK main) thread's
/// main loop and the thread that checks it. Must be called on the main
/// thread; only the first call does anything.
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) { return; }
    let heartbeat = Arc::new(Heartbeat::new());
    let beat = Arc::clone(&heartbeat);
    glib::timeout_add_local(BEAT_INTERVAL, move || {
        beat.beat();
        glib::ControlFlow::Continue
    });
    let spawned = std::thread::Builder::new().name("opusdm-watchdog".into()).spawn(move || watch(&heartbeat));
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(TAG, "cannot start the main loop watchdog, a frozen hub will not be reported: {e}");
    }
}

/// The watchdog thread: check the heartbeat forever, report stalls.
fn watch(heartbeat: &Heartbeat) {
    let mut tracker = StallTracker::default();
    let mut reports = 0;
    loop {
        std::thread::sleep(CHECK_INTERVAL);
        match tracker.observe(heartbeat.age()) {
            None => {}
            Some(StallEvent::Stalled(age)) => on_stall(age, &mut reports),
            Some(StallEvent::Recovered(longest)) => {
                opusdm_ipc::err_log!(TAG, "main loop is running again after a stall of about {}s", longest.as_secs());
            }
        }
    }
}

/// Log a new stall and, within [`MAX_REPORTS`], write its report.
fn on_stall(age: Duration, reports: &mut u32) {
    if *reports >= MAX_REPORTS {
        opusdm_ipc::err_log!(TAG, "main loop stalled for {}s (no report, {MAX_REPORTS} already written)", age.as_secs());
        return;
    }
    *reports += 1;
    let path = report_path(&opusdm_ipc::settings::state_dir().join(REPORT_DIR));
    opusdm_ipc::err_log!(TAG, "main loop stalled for {}s, writing the thread stacks to {}", age.as_secs(), path.display());
    match write_report(&path, age) {
        Ok(()) => opusdm_ipc::err_log!(TAG, "stall report written to {}", path.display()),
        Err(e) => opusdm_ipc::err_log!(TAG, "cannot write the stall report {}: {e}", path.display()),
    }
}

/// A new report file name in `dir`, from the local time and the pid.
fn report_path(dir: &Path) -> PathBuf {
    let stamp = glib::DateTime::now_local()
        .and_then(|now| now.format("%Y%m%d-%H%M%S"))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()));
    dir.join(format!("stall-{stamp}-{}.txt", std::process::id()))
}

/// Write the stall report: header, a line per thread, then the stacks.
fn write_report(path: &Path, age: Duration) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = File::create(path)?;
    writeln!(file, "opusdm-hub {} pid {}: main loop silent for {}s", env!("CARGO_PKG_VERSION"), std::process::id(), age.as_secs())?;
    writeln!(file, "\n== threads (tid name wchan syscall)")?;
    file.write_all(thread_summary(Path::new("/proc/self/task")).as_bytes())?;
    writeln!(file, "\n== stacks")?;
    file.flush()?;
    let outcome = dump_stacks(&file, std::process::id());
    writeln!(file, "\n== {outcome}")?;
    file.flush()
}

/// One line per thread of the process whose `/proc/<pid>/task` is
/// `task_dir`: tid, name, kernel wait channel and current syscall line. A
/// field that cannot be read shows the error instead.
fn thread_summary(task_dir: &Path) -> String {
    let entries = match std::fs::read_dir(task_dir) {
        Ok(entries) => entries,
        Err(e) => return format!("cannot list {}: {e}\n", task_dir.display()),
    };
    let mut tids: Vec<String> = entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    tids.sort_by_key(|tid| tid.parse::<u64>().unwrap_or(u64::MAX));
    let mut out = String::new();
    for tid in tids {
        let dir = task_dir.join(&tid);
        let field = |name: &str| match std::fs::read_to_string(dir.join(name)) {
            Ok(text) => text.trim().to_string(),
            Err(e) => format!("<{name}: {e}>"),
        };
        out.push_str(&format!("{tid} {} wchan={} syscall={}\n", field("comm"), field("wchan"), field("syscall")));
    }
    out
}

/// The stack dumpers to try, best first, for process `pid`.
fn dumpers(pid: u32) -> Vec<(&'static str, Vec<String>)> {
    let pid = pid.to_string();
    vec![
        ("gdb", vec!["-p".into(), pid.clone(), "-batch".into(), "-nx".into(), "-ex".into(), "set pagination off".into(), "-ex".into(), "thread apply all bt".into()]),
        ("eu-stack", vec!["-p".into(), pid]),
    ]
}

/// Append the stacks of every thread of `pid` to `out`, with the first
/// dumper that is installed. Returns a line telling how it went.
fn dump_stacks(out: &File, pid: u32) -> String {
    for (program, args) in dumpers(pid) {
        let (stdout, stderr) = match (out.try_clone(), out.try_clone()) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => return format!("cannot share the report file with {program}: {e}"),
        };
        let spawned = Command::new(program).args(&args).stdin(Stdio::null()).stdout(stdout).stderr(stderr).spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return format!("cannot run {program}: {e}"),
        };
        return wait_dumper(program, &mut child);
    }
    "no stack dumper found (install gdb or elfutils for the stacks)".to_string()
}

/// Wait for a dumper, asking it to stop (SIGTERM, so gdb detaches) when it
/// takes longer than [`DUMP_TIMEOUT`]. Returns a line telling how it went.
fn wait_dumper(program: &str, child: &mut std::process::Child) -> String {
    let started = Instant::now();
    let mut lost_since = None;
    let mut stop_sent: Option<Instant> = None;
    loop {
        match opusdm_ipc::reaped::poll_exit(child, &mut lost_since) {
            Ok(Some(status)) => return format!("{program} finished: {status}"),
            Ok(None) => {}
            Err(e) => return format!("cannot wait for {program}: {e}"),
        }
        if let Some(sent) = stop_sent {
            if sent.elapsed() > DUMP_STOP_GRACE {
                return format!("{program} did not stop after {}s, left running", DUMP_STOP_GRACE.as_secs());
            }
        } else if started.elapsed() > DUMP_TIMEOUT {
            // SAFETY: plain kill(2) on the pid of our own child, still unreaped.
            unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
            stop_sent = Some(Instant::now());
        }
        std::thread::sleep(DUMP_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn a_running_loop_is_not_a_stall() {
        let mut tracker = StallTracker::default();
        for _ in 0..100 {
            assert_eq!(tracker.observe(SECOND), None);
        }
    }

    #[test]
    fn a_stall_is_reported_once_then_its_recovery_once() {
        let mut tracker = StallTracker::default();
        assert_eq!(tracker.observe(STALL_AFTER), Some(StallEvent::Stalled(STALL_AFTER)));
        assert_eq!(tracker.observe(STALL_AFTER + SECOND), None);
        assert_eq!(tracker.observe(STALL_AFTER + 5 * SECOND), None);
        assert_eq!(tracker.observe(SECOND), Some(StallEvent::Recovered(STALL_AFTER + 5 * SECOND)));
        assert_eq!(tracker.observe(SECOND), None);
    }

    #[test]
    fn a_second_stall_is_reported_again() {
        let mut tracker = StallTracker::default();
        assert!(tracker.observe(STALL_AFTER).is_some());
        assert!(tracker.observe(SECOND).is_some());
        assert_eq!(tracker.observe(STALL_AFTER * 2), Some(StallEvent::Stalled(STALL_AFTER * 2)));
    }

    #[test]
    fn the_heartbeat_ages_until_it_beats() {
        let heartbeat = Heartbeat::new();
        std::thread::sleep(Duration::from_millis(30));
        assert!(heartbeat.age() >= Duration::from_millis(30));
        heartbeat.beat();
        assert!(heartbeat.age() < Duration::from_millis(30));
    }

    #[test]
    fn the_thread_summary_names_this_thread() {
        let name = "watchdog-probe";
        let summary = std::thread::Builder::new()
            .name(name.into())
            .spawn(|| thread_summary(Path::new("/proc/self/task")))
            .unwrap()
            .join()
            .unwrap();
        assert!(summary.lines().any(|line| line.contains(name)), "{summary}");
        assert!(summary.lines().all(|line| line.contains("wchan=") && line.contains("syscall=")), "{summary}");
    }

    #[test]
    fn a_missing_task_dir_is_reported_not_hidden() {
        let summary = thread_summary(Path::new("/nonexistent/task"));
        assert!(summary.starts_with("cannot list /nonexistent/task"), "{summary}");
    }

    #[test]
    fn the_report_goes_in_the_given_folder_with_the_pid() {
        let path = report_path(Path::new("/x/hub-stalls"));
        assert_eq!(path.parent(), Some(Path::new("/x/hub-stalls")));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("stall-") && name.ends_with(&format!("-{}.txt", std::process::id())), "{name}");
    }

    /// Attaches a real debugger to the test process: needs gdb or eu-stack
    /// and ptrace permission (`kernel.yama.ptrace_scope` 0). Run by hand
    /// with `cargo test -p opusdm-hub watchdog -- --ignored`.
    #[test]
    #[ignore]
    fn the_report_holds_the_stacks_of_this_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stall.txt");
        write_report(&path, STALL_AFTER).unwrap();
        let report = std::fs::read_to_string(&path).unwrap();
        assert!(report.contains("main loop silent for 10s"), "{report}");
        assert!(report.contains("the_report_holds_the_stacks_of_this_process"), "{report}");
        assert!(report.contains("finished: exit status: 0"), "{report}");
    }
}
