//! The live / revert state machine behind the Input window. No GTK here.

use std::path::Path;

use crate::backend::{Backend, Status};
use crate::change::diff;
use crate::{store, InputError, InputSettings};

/// Settings being edited live: what was there when the window opened (the
/// snapshot), what is applied now, and whether Save was pressed.
pub struct Session<B: Backend> {
    backend: B,
    snapshot: InputSettings,
    current: InputSettings,
    saved: bool,
}

impl<B: Backend> Session<B> {
    /// Start a session: the snapshot is what the system has now.
    pub fn open(backend: B) -> Self {
        let snapshot = backend.read_current().normalized();
        Self { backend, current: snapshot.clone(), snapshot, saved: false }
    }

    /// Which groups can be edited.
    pub fn status(&self) -> Status {
        self.backend.status()
    }

    /// What is applied right now.
    pub fn current(&self) -> &InputSettings {
        &self.current
    }

    /// What the system had when the session opened.
    pub fn snapshot(&self) -> &InputSettings {
        &self.snapshot
    }

    /// Apply the differences between the current settings and `new`. Returns
    /// the errors of the commands that failed; `new` is kept either way, so a
    /// later revert still compares against it.
    pub fn change(&mut self, new: InputSettings) -> Vec<InputError> {
        let new = new.normalized();
        let errors = self.apply_diff(&new);
        self.current = new;
        self.saved = false;
        errors
    }

    /// Write the current settings to `path`; they stay applied.
    pub fn commit(&mut self, path: &Path) -> Result<(), InputError> {
        store::save_to(path, &self.current)?;
        self.saved = true;
        Ok(())
    }

    /// Put the snapshot back, unless the settings were saved.
    pub fn revert(&mut self) -> Vec<InputError> {
        if self.saved { return Vec::new(); }
        let snapshot = self.snapshot.clone();
        let errors = self.apply_diff(&snapshot);
        self.current = snapshot;
        errors
    }

    /// Apply every difference between the current settings and `target`.
    fn apply_diff(&self, target: &InputSettings) -> Vec<InputError> {
        diff(&self.current, target).iter().filter_map(|c| self.backend.apply(c).err()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Change;
    use crate::fake::FakeBackend;

    fn session() -> Session<FakeBackend> {
        Session::open(FakeBackend::new(InputSettings::default()))
    }

    fn with_speed(speed: f64) -> InputSettings {
        let mut s = InputSettings::default();
        s.mouse.speed = speed;
        s
    }

    #[test]
    fn a_change_applies_only_the_difference() {
        let mut s = session();
        assert!(s.change(with_speed(0.5)).is_empty());
        assert_eq!(s.backend.applied(), vec![Change::MouseSpeed(0.5)]);
        assert_eq!(s.current().mouse.speed, 0.5);
    }

    #[test]
    fn an_identical_change_runs_nothing() {
        let mut s = session();
        s.change(InputSettings::default());
        assert!(s.backend.applied().is_empty());
    }

    #[test]
    fn revert_restores_the_snapshot() {
        let mut s = session();
        s.change(with_speed(0.5));
        assert!(s.revert().is_empty());
        assert_eq!(s.backend.applied(), vec![Change::MouseSpeed(0.5), Change::MouseSpeed(0.0)]);
        assert_eq!(s.current(), s.snapshot());
    }

    #[test]
    fn revert_with_no_changes_runs_no_command() {
        let mut s = session();
        assert!(s.revert().is_empty());
        assert!(s.backend.applied().is_empty());
    }

    #[test]
    fn revert_after_a_save_keeps_the_saved_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        let mut s = session();
        s.change(with_speed(0.5));
        s.commit(&path).unwrap();
        assert!(s.revert().is_empty());
        assert_eq!(s.backend.applied(), vec![Change::MouseSpeed(0.5)]);
        assert_eq!(store::load_from(&path).unwrap().mouse.speed, 0.5);
    }

    #[test]
    fn a_change_after_a_save_can_be_reverted() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = session();
        s.change(with_speed(0.5));
        s.commit(&dir.path().join("input.json")).unwrap();
        s.change(with_speed(0.9));
        s.revert();
        assert_eq!(s.current().mouse.speed, 0.0);
    }

    #[test]
    fn revert_still_restores_after_a_failed_apply() {
        let mut s = session();
        s.backend.set_failing(true);
        assert_eq!(s.change(with_speed(0.5)).len(), 1);
        s.backend.set_failing(false);
        assert!(s.revert().is_empty());
        assert_eq!(s.backend.applied().last(), Some(&Change::MouseSpeed(0.0)));
    }

    #[test]
    fn out_of_range_values_are_clamped_before_applying() {
        let mut s = session();
        s.change(with_speed(7.0));
        assert_eq!(s.backend.applied(), vec![Change::MouseSpeed(1.0)]);
    }

    #[test]
    fn a_failed_save_is_an_error_and_does_not_count_as_saved() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        let mut s = session();
        s.change(with_speed(0.5));
        assert!(s.commit(&blocker.join("input.json")).is_err());
        s.revert();
        assert_eq!(s.current().mouse.speed, 0.0);
    }
}
