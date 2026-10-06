//! Reading and writing `input.json`.

use std::path::{Path, PathBuf};

use crate::{InputError, InputSettings};

/// Tag used in the log lines of the shared JSON helpers.
const APP_TAG: &str = "opusdm-input-config";

/// Path of `input.json` under the OpusDM config directory.
pub fn input_path() -> PathBuf {
    opusdm_ipc::settings::config_dir().join("input.json")
}

/// Load the settings saved at `path`, normalized. `None` when the file is
/// missing or unusable (a malformed file is copied to `input.json.bak` by the
/// shared helper), so callers leave the system alone in that case.
pub fn load_from(path: &Path) -> Option<InputSettings> {
    let settings: InputSettings = opusdm_ipc::settings::read_json(path, APP_TAG, "input settings")?;
    Some(settings.normalized())
}

/// Write `settings` to `path` (atomically, creating the folder).
pub fn save_to(path: &Path, settings: &InputSettings) -> Result<(), InputError> {
    opusdm_ipc::settings::save_json(path, settings)
        .map_err(|e| InputError::Write { path: path.display().to_string(), reason: e.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("input.json");
        let mut s = InputSettings::default();
        s.mouse.speed = 0.4;
        save_to(&path, &s).unwrap();
        assert_eq!(load_from(&path), Some(s));
    }

    #[test]
    fn a_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_from(&dir.path().join("input.json")), None);
    }

    #[test]
    fn a_hand_edited_file_is_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        std::fs::write(&path, r#"{"mouse":{"speed":9},"keyboard":{"layouts":[]}}"#).unwrap();
        let s = load_from(&path).unwrap();
        assert_eq!(s.mouse.speed, 1.0);
        assert_eq!(s.keyboard.layouts.len(), 1);
    }

    #[test]
    fn a_malformed_file_is_none_and_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), None);
        assert!(dir.path().join("input.json.bak").exists());
    }
}
