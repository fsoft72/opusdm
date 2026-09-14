//! Settings persistence: stored in ~/.config/opusdm/settings.json.
//!
//! Delegated to `opusdm_ipc::settings` so configuration path resolution,
//! directory creation, and error diagnostics are shared across all crates.

use opusdm_ipc::Settings;
use std::path::{Path, PathBuf};

pub fn config_path() -> PathBuf {
    opusdm_ipc::settings::settings_path()
}

pub fn load_settings() -> Settings {
    opusdm_ipc::settings::load_settings("opusdm-hub")
}

/// Testable core of [`load_settings`].
pub fn load_settings_from(path: &Path) -> Settings {
    opusdm_ipc::settings::load_settings_from(path, "opusdm-hub")
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    opusdm_ipc::settings::save_settings(settings)
}

/// Testable core of [`save_settings`].
pub fn save_settings_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    opusdm_ipc::settings::save_settings_to(path, settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_from_missing_file_is_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        assert_eq!(load_settings_from(&path), Settings::default());
    }

    #[test]
    fn load_from_malformed_file_reports_and_returns_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        fs::write(&path, b"{ not json at all").unwrap();
        assert_eq!(load_settings_from(&path), Settings::default());
    }

    #[test]
    fn save_and_load_roundtrips() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("nested").join("settings.json");
        let mut s = Settings::default();
        s.appearance.monospace_list = true;
        save_settings_to(&path, &s).unwrap();
        let loaded = load_settings_from(&path);
        assert!(loaded.appearance.monospace_list);
    }
}
