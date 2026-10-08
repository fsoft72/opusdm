//! The edit / apply / confirm / revert state machine behind the Screens
//! window. No GTK here: the window runs the confirmation timer and calls
//! `confirm` or `revert`.

use std::path::Path;

use crate::backend::ScreenBackend;
use crate::{store, Layout, Monitor, ScreenError};

/// Where the session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Nothing is waiting for an answer.
    Idle,
    /// A layout was applied and the user has not said yet whether to keep it.
    Pending,
}

/// Monitors being edited: what is set now (committed), what the user is
/// building (edited) and, after Apply, the layout to go back to.
pub struct Session<B: ScreenBackend> {
    backend: B,
    monitors: Vec<Monitor>,
    committed: Layout,
    edited: Layout,
    previous: Option<Layout>,
    state: State,
}

impl<B: ScreenBackend> Session<B> {
    /// Start a session on what the system has now.
    pub fn open(backend: B) -> Result<Self, ScreenError> {
        let parsed = backend.query()?;
        Ok(Self { backend, monitors: parsed.monitors, committed: parsed.layout.clone(), edited: parsed.layout, previous: None, state: State::Idle })
    }

    /// The connected monitors.
    pub fn monitors(&self) -> &[Monitor] {
        &self.monitors
    }

    /// The layout the system has (the last one confirmed or found at start).
    pub fn committed(&self) -> &Layout {
        &self.committed
    }

    /// The layout being built in the window.
    pub fn edited(&self) -> &Layout {
        &self.edited
    }

    /// Whether an applied layout waits for an answer.
    pub fn state(&self) -> State {
        self.state
    }

    /// Whether the edited layout differs from the one in use.
    pub fn is_dirty(&self) -> bool {
        self.edited != self.committed
    }

    /// Replace the layout being built.
    pub fn edit(&mut self, layout: Layout) {
        self.edited = layout;
    }

    /// Throw the edits away.
    pub fn reset(&mut self) {
        self.edited = self.committed.clone();
    }

    /// Apply the edited layout. On success it waits for `confirm` or
    /// `revert`; on failure nothing changed on the session.
    pub fn apply(&mut self) -> Result<(), ScreenError> {
        if self.state == State::Pending { return Err(ScreenError::Session("a layout is waiting for an answer".to_string())); }
        self.edited.validate(&self.monitors)?;
        self.backend.apply(&self.edited)?;
        self.previous = Some(std::mem::replace(&mut self.committed, self.edited.clone()));
        self.state = State::Pending;
        Ok(())
    }

    /// Keep the applied layout. With `profiles` it is also saved for these
    /// monitors; a failed save is returned but the layout stays kept.
    pub fn confirm(&mut self, profiles: Option<&Path>) -> Result<(), ScreenError> {
        if self.state != State::Pending { return Ok(()); }
        self.previous = None;
        self.state = State::Idle;
        match profiles {
            Some(path) => store::remember(path, &self.monitors, self.committed.clone()),
            None => Ok(()),
        }
    }

    /// Put the previous layout back (the user said no, or did not answer). If
    /// that fails the applied layout stays and the error says so.
    pub fn revert(&mut self) -> Result<(), ScreenError> {
        let Some(previous) = self.previous.clone() else { return Ok(()) };
        self.backend.apply(&previous)?;
        self.committed = previous.clone();
        self.edited = previous;
        self.previous = None;
        self.state = State::Idle;
        Ok(())
    }

    /// Read the system again (a monitor was plugged in or removed). Edits
    /// are dropped. Not allowed while a layout waits for an answer.
    pub fn refresh(&mut self) -> Result<(), ScreenError> {
        if self.state == State::Pending { return Err(ScreenError::Session("a layout is waiting for an answer".to_string())); }
        let parsed = self.backend.query()?;
        self.monitors = parsed.monitors;
        self.committed = parsed.layout.clone();
        self.edited = parsed.layout;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{two_monitor_system, FakeBackend};
    use crate::modes;

    fn session() -> Session<FakeBackend> {
        Session::open(FakeBackend::new(two_monitor_system())).unwrap()
    }

    #[test]
    fn opens_on_the_layout_of_the_system() {
        let s = session();
        assert_eq!(s.monitors().len(), 2);
        assert_eq!(s.edited(), s.committed());
        assert!(!s.is_dirty());
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn apply_then_confirm_keeps_the_layout() {
        let mut s = session();
        let mirrored = modes::mirror(s.monitors(), s.committed()).unwrap();
        s.edit(mirrored.clone());
        assert!(s.is_dirty());
        s.apply().unwrap();
        assert_eq!(s.state(), State::Pending);
        s.confirm(None).unwrap();
        assert_eq!(s.state(), State::Idle);
        assert_eq!(s.committed(), &mirrored);
        assert!(!s.is_dirty());
        assert_eq!(s.revert(), Ok(()), "nothing to go back to after confirm");
    }

    #[test]
    fn apply_then_revert_puts_the_old_layout_back() {
        let mut s = session();
        let old = s.committed().clone();
        s.edit(modes::mirror(s.monitors(), &old).unwrap());
        s.apply().unwrap();
        s.revert().unwrap();
        assert_eq!(s.state(), State::Idle);
        assert_eq!(s.committed(), &old);
        assert_eq!(s.edited(), &old);
    }

    #[test]
    fn a_failed_revert_is_reported_and_keeps_the_state() {
        let mut s = session();
        s.edit(modes::mirror(s.monitors(), s.committed()).unwrap());
        s.apply().unwrap();
        s.backend.set_failing(true);
        assert!(s.revert().is_err());
        assert_eq!(s.state(), State::Pending);
    }

    #[test]
    fn a_failed_apply_changes_nothing() {
        let mut s = session();
        let old = s.committed().clone();
        s.edit(modes::mirror(s.monitors(), &old).unwrap());
        s.backend.set_failing(true);
        assert!(s.apply().is_err());
        assert_eq!(s.state(), State::Idle);
        assert_eq!(s.committed(), &old);
    }

    #[test]
    fn an_invalid_layout_is_not_sent_to_the_backend() {
        let mut s = session();
        let mut bad = s.committed().clone();
        bad.monitors.iter_mut().for_each(|m| m.enabled = false);
        s.edit(bad);
        assert!(matches!(s.apply(), Err(ScreenError::Invalid(_))));
        assert!(s.backend.applied().is_empty());
    }

    #[test]
    fn a_second_apply_waits_for_the_first_answer() {
        let mut s = session();
        s.edit(modes::mirror(s.monitors(), s.committed()).unwrap());
        s.apply().unwrap();
        assert!(s.apply().is_err());
        assert!(s.refresh().is_err());
    }

    #[test]
    fn reset_drops_the_edits() {
        let mut s = session();
        s.edit(modes::mirror(s.monitors(), s.committed()).unwrap());
        s.reset();
        assert!(!s.is_dirty());
    }

    #[test]
    fn confirm_can_save_a_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screens.json");
        let mut s = session();
        let mirrored = modes::mirror(s.monitors(), s.committed()).unwrap();
        s.edit(mirrored.clone());
        s.apply().unwrap();
        s.confirm(Some(&path)).unwrap();
        assert_eq!(store::load_from(&path).unwrap().find(s.monitors()), Some(mirrored));
    }

    #[test]
    fn refresh_reads_a_new_set_of_monitors() {
        let mut s = session();
        let mut one = two_monitor_system();
        one.monitors.truncate(1);
        one.layout.monitors.truncate(1);
        s.backend.set_state(one);
        s.refresh().unwrap();
        assert_eq!(s.monitors().len(), 1);
        assert_eq!(s.committed().monitors.len(), 1);
    }
}
