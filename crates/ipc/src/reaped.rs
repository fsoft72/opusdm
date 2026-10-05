//! Exit statuses of children the hub's SIGCHLD reaper collected.
//!
//! The reaper calls `waitpid(-1)`, so it can take the status of a child some
//! other thread is waiting for (a compressor started by a job). That thread's
//! own wait then fails with `ECHILD`; it recovers the status here instead.

use std::collections::VecDeque;
use std::io;
use std::process::{Child, ExitStatus};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long to wait for the status of a child someone else reaped.
const REAP_GRACE: Duration = Duration::from_secs(2);

/// How often [`wait`] polls the child.
const WAIT_POLL: Duration = Duration::from_millis(10);

/// How many reaped children are remembered; older ones are dropped.
const KEEP: usize = 256;

/// `(pid, raw wait status)`, oldest first.
static REAPED: Mutex<VecDeque<(u32, i32)>> = Mutex::new(VecDeque::new());

/// Remember that `pid` was reaped with the raw wait `status`.
pub fn record(pid: u32, status: i32) {
    let Ok(mut list) = REAPED.lock() else { return };
    if list.len() >= KEEP {
        list.pop_front();
    }
    list.push_back((pid, status));
}

/// The raw wait status of `pid` if the reaper collected it (forgetting it).
pub fn take(pid: u32) -> Option<i32> {
    let mut list = REAPED.lock().ok()?;
    let at = list.iter().position(|(p, _)| *p == pid)?;
    list.remove(at).map(|(_, status)| status)
}

/// Non-blocking exit check of `child`. When the reaper collected the child
/// first, the wait fails with `ECHILD` and the status it recorded is used.
/// `lost_since` is when the child was first found gone; the status must show
/// up within a couple of seconds or the `ECHILD` error is returned.
pub fn poll_exit(child: &mut Child, lost_since: &mut Option<Instant>) -> io::Result<Option<ExitStatus>> {
    use std::os::unix::process::ExitStatusExt;
    match child.try_wait() {
        Err(e) if e.raw_os_error() == Some(libc::ECHILD) => {
            if let Some(raw) = take(child.id()) {
                crate::err_log!("reaped", "pid {} was reaped elsewhere, recorded status {raw}", child.id());
                return Ok(Some(ExitStatus::from_raw(raw)));
            }
            if lost_since.get_or_insert_with(Instant::now).elapsed() > REAP_GRACE {
                return Err(e);
            }
            Ok(None)
        }
        other => other,
    }
}

/// Block until `child` exits, like `Child::wait`, but safe against the
/// SIGCHLD reaper taking the child first.
pub fn wait(child: &mut Child) -> io::Result<ExitStatus> {
    let mut lost_since = None;
    loop {
        if let Some(status) = poll_exit(child, &mut lost_since)? {
            return Ok(status);
        }
        std::thread::sleep(WAIT_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn a_child_reaped_by_someone_else_still_reports_its_status() {
        let mut child = Command::new("false").spawn().unwrap();
        let pid = child.id();
        let mut raw: libc::c_int = 0;
        // SAFETY: waits for our own child; `raw` is a valid out-pointer.
        assert_eq!(unsafe { libc::waitpid(pid as libc::pid_t, &mut raw, 0) }, pid as libc::pid_t);
        record(pid, raw);
        assert!(!wait(&mut child).unwrap().success());
    }

    #[test]
    fn wait_returns_the_status_of_a_normal_child() {
        assert!(wait(&mut Command::new("true").spawn().unwrap()).unwrap().success());
    }

    #[test]
    fn a_recorded_status_is_taken_once() {
        record(4_000_001, 256);
        assert_eq!(take(4_000_001), Some(256));
        assert_eq!(take(4_000_001), None);
    }

    #[test]
    fn only_the_latest_children_are_kept() {
        for pid in 0..(KEEP as u32 + 10) {
            record(5_000_000 + pid, 0);
        }
        assert_eq!(take(5_000_000), None);
        assert_eq!(take(5_000_000 + KEEP as u32 + 9), Some(0));
    }
}
