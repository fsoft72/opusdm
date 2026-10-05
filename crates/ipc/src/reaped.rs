//! Exit statuses of children the hub's SIGCHLD reaper collected.
//!
//! The reaper calls `waitpid(-1)`, so it can take the status of a child some
//! other thread is waiting for (a compressor started by a job). That thread's
//! own wait then fails with `ECHILD`; it recovers the status here instead.

use std::collections::VecDeque;
use std::sync::Mutex;

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

#[cfg(test)]
mod tests {
    use super::*;

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
