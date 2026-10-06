//! Applying saved settings to the whole system: at start-up and when a
//! pointer is plugged in. Used by the hub.

use crate::backend::Backend;
use crate::change::all_changes;
use crate::{InputError, InputSettings};

/// Apply every setting of `settings`; returns the errors of the commands that failed.
pub fn apply_all(backend: &impl Backend, settings: &InputSettings) -> Vec<InputError> {
    all_changes(settings).iter().filter_map(|c| backend.apply(c).err()).collect()
}

/// When the pointer signature differs from `last`: look at the devices again,
/// load the saved settings with `saved` and apply them. Does nothing, and
/// reads no file, while the signature is the same.
pub fn reapply_if_changed(backend: &impl Backend, last: &mut Option<String>, saved: impl FnOnce() -> Option<InputSettings>) -> Vec<InputError> {
    let now = backend.signature();
    if last.as_deref() == Some(now.as_str()) { return Vec::new(); }
    *last = Some(now);
    backend.refresh();
    let Some(settings) = saved() else { return Vec::new() };
    apply_all(backend, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeBackend;

    fn backend() -> FakeBackend {
        FakeBackend::new(InputSettings::default())
    }

    #[test]
    fn apply_all_runs_every_change() {
        let b = backend();
        assert!(apply_all(&b, &InputSettings::default()).is_empty());
        assert_eq!(b.applied().len(), 10);
    }

    #[test]
    fn the_first_look_applies_the_saved_settings() {
        let b = backend();
        let mut last = None;
        reapply_if_changed(&b, &mut last, || Some(InputSettings::default()));
        assert_eq!(b.applied().len(), 10);
        assert_eq!(b.refresh_count(), 1);
    }

    #[test]
    fn an_unchanged_signature_does_nothing_and_loads_nothing() {
        let b = backend();
        let mut last = None;
        reapply_if_changed(&b, &mut last, || Some(InputSettings::default()));
        let before = b.applied().len();
        reapply_if_changed(&b, &mut last, || panic!("must not load the file"));
        assert_eq!(b.applied().len(), before);
    }

    #[test]
    fn a_plugged_in_pointer_gets_the_saved_values_again() {
        let b = backend();
        let mut last = None;
        reapply_if_changed(&b, &mut last, || Some(InputSettings::default()));
        b.set_signature("9:new mouse");
        reapply_if_changed(&b, &mut last, || Some(InputSettings::default()));
        assert_eq!(b.applied().len(), 20);
        assert_eq!(b.refresh_count(), 2);
    }

    #[test]
    fn no_saved_file_means_nothing_is_applied() {
        let b = backend();
        let mut last = None;
        assert!(reapply_if_changed(&b, &mut last, || None).is_empty());
        assert!(b.applied().is_empty());
    }
}
