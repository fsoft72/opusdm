//! Applies the saved input settings (`input.json`) when the hub starts and
//! again whenever a pointer is plugged in or removed.

use std::path::Path;
use std::time::Duration;

use opusdm_input_config::apply::reapply_if_changed;
use opusdm_input_config::backend::{for_session, session_problem, Backend};
use opusdm_input_config::{store, InputError};

/// Seconds between two looks at the pointer list.
const POLL_SECS: u64 = 3;

/// One look: reapply the file at `path` when the pointers changed.
pub(crate) fn tick(backend: &impl Backend, last: &mut Option<String>, path: &Path) -> Vec<InputError> {
    reapply_if_changed(backend, last, || store::load_from(path))
}

/// Start the thread that keeps the saved input settings applied. A failure
/// is logged and never stops the hub.
pub(crate) fn start() {
    // Wayland without labwc or Wayfire: the compositor owns the input settings.
    if session_problem().is_some() { return; }
    let spawned = std::thread::Builder::new().name("input-apply".to_string()).spawn(|| {
        let backend = for_session();
        let path = store::input_path();
        let mut last = None;
        loop {
            for e in tick(&backend, &mut last, &path) {
                opusdm_ipc::err_log!("opusdm-hub", "input settings: {e}");
            }
            std::thread::sleep(Duration::from_secs(POLL_SECS));
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start the input settings thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_input_config::fake::FakeBackend;
    use opusdm_input_config::InputSettings;

    #[test]
    fn tick_applies_the_saved_file_and_ignores_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        let b = FakeBackend::new(InputSettings::default());

        let mut last = None;
        assert!(tick(&b, &mut last, &path).is_empty());
        assert!(b.applied().is_empty());

        store::save_to(&path, &InputSettings::default()).unwrap();
        b.set_signature("plugged");
        tick(&b, &mut last, &path);
        assert_eq!(b.applied().len(), 10);
    }
}
