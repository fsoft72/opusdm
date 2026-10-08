//! When the panel's tray exists: both tray threads (`systray` for XEmbed,
//! `sni` for StatusNotifierItem) report here once apps can dock with them,
//! and the hub waits for that before it counts the Startup Apps' wait times
//! (an app started earlier finds no tray: it may show its window, or quit
//! when its window is closed instead of hiding in the tray).
//!
//! A tray thread that cannot start (no X server, no session bus, the
//! selection already owned) reports too: there is nothing to wait for.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// One of the two tray protocols the panel hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayKind {
    XEmbed,
    Sni,
}

/// Which trays have reported, and the condition the waiters sleep on.
struct Readiness {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl Readiness {
    const fn new() -> Self {
        Self { state: Mutex::new((false, false)), changed: Condvar::new() }
    }

    fn mark(&self, kind: TrayKind) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match kind {
            TrayKind::XEmbed => state.0 = true,
            TrayKind::Sni => state.1 = true,
        }
        self.changed.notify_all();
    }

    fn wait(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while !(state.0 && state.1) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self.changed.wait_timeout(state, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        true
    }
}

static READINESS: Readiness = Readiness::new();

/// Record that the `kind` tray is ready (or will never be).
pub(crate) fn mark_ready(kind: TrayKind) {
    READINESS.mark(kind);
}

/// Block until both trays are ready, or `limit` passes. Returns whether
/// they became ready in time.
pub fn wait_until_ready(limit: Duration) -> bool {
    READINESS.wait(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn waits_for_both_trays() {
        let r = Arc::new(Readiness::new());
        r.mark(TrayKind::XEmbed);
        assert!(!r.wait(Duration::from_millis(30)), "SNI is not ready yet");
        let marker = r.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            marker.mark(TrayKind::Sni);
        });
        assert!(r.wait(Duration::from_secs(5)));
        t.join().unwrap();
    }

    #[test]
    fn a_ready_tray_returns_at_once() {
        let r = Readiness::new();
        r.mark(TrayKind::Sni);
        r.mark(TrayKind::XEmbed);
        let begun = Instant::now();
        assert!(r.wait(Duration::from_secs(5)));
        assert!(begun.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn gives_up_after_the_limit() {
        let r = Readiness::new();
        let begun = Instant::now();
        assert!(!r.wait(Duration::from_millis(100)));
        assert!(begun.elapsed() >= Duration::from_millis(100));
    }
}
