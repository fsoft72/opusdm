//! The pure health model: what is wrong with the keyring and when to raise or
//! clear the alarm.

/// Why the keyring is not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The session bus cannot be reached.
    NoBus,
    /// Nobody owns `org.freedesktop.secrets`.
    NoService,
    /// There is no default collection (and no `login` one).
    NoCollection,
    /// The collection is locked.
    Locked,
    /// The service did not answer in time.
    Timeout,
}

impl Reason {
    /// The reason in plain words, shown to the user.
    pub fn text(&self) -> &'static str {
        match self {
            Reason::NoBus => "The session bus is not reachable: saved passwords are not available",
            Reason::NoService => "No keyring is running: saved passwords are not available",
            Reason::NoCollection => "The keyring has no default collection: saved passwords are not available",
            Reason::Locked => "The keyring is locked: saved passwords are not available",
            Reason::Timeout => "The keyring does not answer: saved passwords may not be available",
        }
    }
}

/// The result of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// All is fine.
    Healthy,
    /// Something is wrong, for this reason.
    Unhealthy(Reason),
}

/// What the tracker asks the caller to do after one observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Nothing changed.
    None,
    /// Show the alarm (or update it) for this reason.
    Raise(Reason),
    /// The problem is gone: remove the alarm.
    Clear,
}

/// Number of consecutive unhealthy checks needed to raise the alarm.
const FAILURES_TO_RAISE: u32 = 2;

/// Turns a stream of [`Health`] observations into [`Event`]s, so a single
/// bad check (or the start of the session) never raises a false alarm.
#[derive(Debug)]
pub struct Tracker {
    /// How many observations were made so far.
    seen: u32,
    /// The first observations that are ignored.
    grace: u32,
    /// Consecutive unhealthy observations.
    streak: u32,
    /// The reason of the last unhealthy observation.
    last: Option<Reason>,
    /// The reason of the alarm now showing, if any.
    raised: Option<Reason>,
}

impl Tracker {
    /// A tracker that ignores the first `grace_checks` observations.
    pub fn new(grace_checks: u32) -> Self {
        Tracker { seen: 0, grace: grace_checks, streak: 0, last: None, raised: None }
    }

    /// Feed one check result and get what to do about it.
    pub fn observe(&mut self, h: Health) -> Event {
        self.seen = self.seen.saturating_add(1);
        if self.seen <= self.grace { return Event::None; }
        match h {
            Health::Healthy => self.observe_healthy(),
            Health::Unhealthy(reason) => self.observe_unhealthy(reason),
        }
    }

    /// A good check: forget the streak and clear the alarm if there is one.
    fn observe_healthy(&mut self) -> Event {
        self.streak = 0;
        self.last = None;
        if self.raised.take().is_some() { Event::Clear } else { Event::None }
    }

    /// A bad check: raise after two in a row, or at once when the alarm is
    /// showing and the reason changed.
    fn observe_unhealthy(&mut self, reason: Reason) -> Event {
        self.streak = self.streak.saturating_add(1);
        self.last = Some(reason);
        if self.raised == Some(reason) { return Event::None; }
        if self.raised.is_none() && self.streak < FAILURES_TO_RAISE { return Event::None; }
        self.raised = Some(reason);
        Event::Raise(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCKED: Health = Health::Unhealthy(Reason::Locked);

    #[test]
    fn first_failure_does_not_raise() {
        let mut t = Tracker::new(0);
        assert_eq!(t.observe(LOCKED), Event::None);
    }

    #[test]
    fn two_failures_raise_once() {
        let mut t = Tracker::new(0);
        t.observe(LOCKED);
        assert_eq!(t.observe(LOCKED), Event::Raise(Reason::Locked));
        assert_eq!(t.observe(LOCKED), Event::None);
    }

    #[test]
    fn same_reason_does_not_repeat() {
        let mut t = Tracker::new(0);
        t.observe(LOCKED);
        t.observe(LOCKED);
        for _ in 0..5 { assert_eq!(t.observe(LOCKED), Event::None); }
    }

    #[test]
    fn reason_change_raises_again() {
        let mut t = Tracker::new(0);
        t.observe(LOCKED);
        t.observe(LOCKED);
        assert_eq!(t.observe(Health::Unhealthy(Reason::NoService)), Event::Raise(Reason::NoService));
        assert_eq!(t.observe(Health::Unhealthy(Reason::NoService)), Event::None);
    }

    #[test]
    fn one_good_clears() {
        let mut t = Tracker::new(0);
        t.observe(LOCKED);
        t.observe(LOCKED);
        assert_eq!(t.observe(Health::Healthy), Event::Clear);
        assert_eq!(t.observe(Health::Healthy), Event::None);
    }

    #[test]
    fn a_good_check_between_failures_resets_the_streak() {
        let mut t = Tracker::new(0);
        t.observe(LOCKED);
        t.observe(Health::Healthy);
        assert_eq!(t.observe(LOCKED), Event::None);
    }

    #[test]
    fn grace_checks_are_ignored() {
        let mut t = Tracker::new(2);
        assert_eq!(t.observe(LOCKED), Event::None);
        assert_eq!(t.observe(LOCKED), Event::None);
        assert_eq!(t.observe(LOCKED), Event::None);
        assert_eq!(t.observe(LOCKED), Event::Raise(Reason::Locked));
    }
}
