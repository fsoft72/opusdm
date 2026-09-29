//! Loop protection for script events (F-05). A hook on `ListerOpened` that
//! itself opens a lister would fire `ListerOpened` again, forever. Two
//! guards break the loop: an event is dropped while runs of its previous
//! emission are still going, and the same event is never emitted twice
//! within [`EVENT_COOLDOWN`].

pub use opusdm_ipc::bindings::ScriptEvent;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Minimum time between two emissions of the same event.
pub const EVENT_COOLDOWN: Duration = Duration::from_secs(2);

/// A run still "in flight" after this long is treated as finished, so a run
/// that leaked (its Lister vanished) cannot block an event forever.
pub const INFLIGHT_MAX_AGE: Duration = Duration::from_secs(60);

/// How long the hub waits for `Shutdown` hooks before quitting anyway.
pub const SHUTDOWN_HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// Tracks the last emission and the in-flight runs of every event.
#[derive(Default)]
pub struct EventGuard {
    last: HashMap<ScriptEvent, Instant>,
    inflight: HashMap<u64, (ScriptEvent, Instant)>,
}

impl EventGuard {
    /// An empty guard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `event` may be emitted at `now`. On `true` the emission is
    /// recorded (it starts the cooldown). `false` when the previous emission
    /// is younger than [`EVENT_COOLDOWN`] or one of its runs is still in
    /// flight and younger than [`INFLIGHT_MAX_AGE`].
    pub fn try_begin(&mut self, event: ScriptEvent, now: Instant) -> bool {
        if let Some(&last) = self.last.get(&event) {
            if now.saturating_duration_since(last) < EVENT_COOLDOWN {
                return false;
            }
        }
        let busy = self
            .inflight
            .values()
            .any(|&(e, started)| e == event && now.saturating_duration_since(started) < INFLIGHT_MAX_AGE);
        if busy {
            return false;
        }
        self.last.insert(event, now);
        true
    }

    /// Record that run `run_id` was started for `event` at `now`.
    pub fn add_run(&mut self, event: ScriptEvent, run_id: u64, now: Instant) {
        self.inflight.insert(run_id, (event, now));
    }

    /// Record that run `run_id` ended (or can never complete).
    pub fn finish(&mut self, run_id: u64) {
        self.inflight.remove(&run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_first_emission_of_an_event_is_allowed() {
        let mut guard = EventGuard::new();
        assert!(guard.try_begin(ScriptEvent::Startup, Instant::now()));
    }

    #[test]
    fn a_second_emission_inside_the_cooldown_is_dropped() {
        let mut guard = EventGuard::new();
        let t0 = Instant::now();
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0));
        assert!(!guard.try_begin(ScriptEvent::ListerOpened, t0 + EVENT_COOLDOWN - Duration::from_millis(1)));
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0 + EVENT_COOLDOWN));
    }

    #[test]
    fn other_events_are_not_throttled_by_one_event() {
        let mut guard = EventGuard::new();
        let t0 = Instant::now();
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0));
        assert!(guard.try_begin(ScriptEvent::ListerClosed, t0));
    }

    #[test]
    fn a_run_still_in_flight_blocks_the_next_emission_after_the_cooldown() {
        let mut guard = EventGuard::new();
        let t0 = Instant::now();
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0));
        guard.add_run(ScriptEvent::ListerOpened, 7, t0);
        let later = t0 + EVENT_COOLDOWN + Duration::from_secs(1);
        assert!(!guard.try_begin(ScriptEvent::ListerOpened, later), "run 7 has not finished");
        guard.finish(7);
        assert!(guard.try_begin(ScriptEvent::ListerOpened, later));
    }

    #[test]
    fn a_leaked_run_stops_blocking_after_the_max_age() {
        let mut guard = EventGuard::new();
        let t0 = Instant::now();
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0));
        guard.add_run(ScriptEvent::ListerOpened, 7, t0);
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0 + INFLIGHT_MAX_AGE + Duration::from_secs(1)));
    }

    #[test]
    fn finishing_an_unknown_run_is_harmless() {
        let mut guard = EventGuard::new();
        guard.finish(99);
    }
}
