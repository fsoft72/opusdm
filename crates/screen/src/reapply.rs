//! Puts a saved profile back when the set of connected monitors changes.
//! Used by the hub; no GTK here.

use std::path::Path;

use crate::backend::ScreenBackend;
use crate::store::{self, profile_key};
use crate::ScreenError;

/// Look at the monitors and apply the saved profile when the set of
/// connected monitors is not the one seen last (`last_key`, `None` the first
/// time). A set with no profile is left alone, and so is a layout that
/// already matches the profile. Returns whether a layout was applied. When
/// the monitors cannot be read, `last_key` stays as it was.
pub fn reapply_if_changed(backend: &impl ScreenBackend, last_key: &mut Option<Vec<String>>, path: &Path) -> Result<bool, ScreenError> {
    let parsed = backend.query()?;
    let key = profile_key(&parsed.monitors);
    if last_key.as_ref() == Some(&key) { return Ok(false); }
    *last_key = Some(key);
    let Some(profiles) = store::load_from(path) else { return Ok(false) };
    let Some(layout) = profiles.find(&parsed.monitors) else { return Ok(false) };
    if layout == parsed.layout { return Ok(false); }
    backend.apply(&layout)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{two_monitor_system, FakeBackend};
    use crate::modes;

    /// A profile file for the two monitors holding the mirrored layout.
    fn profile_file(dir: &Path) -> std::path::PathBuf {
        let sys = two_monitor_system();
        let path = dir.join("screens.json");
        store::remember(&path, &sys.monitors, modes::mirror(&sys.monitors, &sys.layout).unwrap()).unwrap();
        path
    }

    #[test]
    fn a_known_set_of_monitors_gets_its_profile_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = profile_file(dir.path());
        let b = FakeBackend::new(two_monitor_system());
        let mut last = None;
        assert_eq!(reapply_if_changed(&b, &mut last, &path), Ok(true));
        assert_eq!(b.applied().len(), 1);
        assert!(b.applied()[0].is_mirrored());
        assert_eq!(reapply_if_changed(&b, &mut last, &path), Ok(false));
        assert_eq!(b.applied().len(), 1, "same monitors: nothing more");
    }

    #[test]
    fn a_change_the_user_makes_is_not_undone_while_the_monitors_stay() {
        let dir = tempfile::tempdir().unwrap();
        let path = profile_file(dir.path());
        let b = FakeBackend::new(two_monitor_system());
        let mut last = None;
        reapply_if_changed(&b, &mut last, &path).unwrap();
        b.set_state(two_monitor_system());
        assert_eq!(reapply_if_changed(&b, &mut last, &path), Ok(false));
    }

    #[test]
    fn plugging_a_monitor_in_again_applies_the_profile_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = profile_file(dir.path());
        let b = FakeBackend::new(two_monitor_system());
        let mut last = None;
        reapply_if_changed(&b, &mut last, &path).unwrap();
        let mut one = two_monitor_system();
        one.monitors.truncate(1);
        one.layout.monitors.truncate(1);
        b.set_state(one);
        assert_eq!(reapply_if_changed(&b, &mut last, &path), Ok(false), "no profile for one monitor");
        b.set_state(two_monitor_system());
        assert_eq!(reapply_if_changed(&b, &mut last, &path), Ok(true));
        assert_eq!(b.applied().len(), 2);
    }

    #[test]
    fn no_file_or_an_unknown_set_leaves_the_screens_alone() {
        let dir = tempfile::tempdir().unwrap();
        let b = FakeBackend::new(two_monitor_system());
        let mut last = None;
        assert_eq!(reapply_if_changed(&b, &mut last, &dir.path().join("none.json")), Ok(false));
        assert!(b.applied().is_empty());
    }

    #[test]
    fn a_layout_that_already_matches_is_not_applied() {
        let dir = tempfile::tempdir().unwrap();
        let sys = two_monitor_system();
        let path = dir.path().join("screens.json");
        store::remember(&path, &sys.monitors, sys.layout.clone()).unwrap();
        let b = FakeBackend::new(sys);
        assert_eq!(reapply_if_changed(&b, &mut None, &path), Ok(false));
        assert!(b.applied().is_empty());
    }

    #[test]
    fn a_failed_apply_is_an_error_and_a_failed_read_keeps_the_last_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = profile_file(dir.path());
        let b = FakeBackend::new(two_monitor_system());
        b.set_failing(true);
        let mut last = None;
        assert!(reapply_if_changed(&b, &mut last, &path).is_err());
        assert!(last.is_some(), "this set was seen, so a retry is not made on every event");
    }
}
