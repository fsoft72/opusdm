//! Desktop settings persistence: `~/.config/opusdm/desktop.json`.
//!
//! Delegated to `opusdm_ipc::settings` so configuration path resolution,
//! directory creation, and error diagnostics are shared across all crates.

use opusdm_ipc::DesktopSettings;
use std::path::{Path, PathBuf};

/// Path to `~/.config/opusdm/desktop.json`.
pub fn config_path() -> PathBuf {
    opusdm_ipc::settings::desktop_settings_path()
}

/// Load desktop settings from the standard path. Missing or unparseable ->
/// [`DesktopSettings::default`]. Always clamped via
/// [`DesktopSettings::sanitized`].
pub fn load() -> DesktopSettings {
    opusdm_ipc::settings::load_desktop_settings("opusdm-hub")
}

/// Persist `settings` to the standard path, creating the directory.
pub fn save(settings: &DesktopSettings) -> std::io::Result<()> {
    opusdm_ipc::settings::save_desktop_settings(settings)
}

/// Testable core of [`load`].
pub fn load_from(path: &Path) -> DesktopSettings {
    opusdm_ipc::settings::load_desktop_settings_from(path, "opusdm-hub")
}

/// Testable core of [`save`].
pub fn save_to(path: &Path, settings: &DesktopSettings) -> std::io::Result<()> {
    opusdm_ipc::settings::save_desktop_settings_to(path, settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_from_missing_file_is_the_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("desktop.json");
        assert_eq!(load_from(&path), DesktopSettings::default());
    }

    #[test]
    fn load_from_malformed_file_is_the_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("desktop.json");
        fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load_from(&path), DesktopSettings::default());
    }

    #[test]
    fn save_then_load_roundtrips_and_sanitizes() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("nested").join("desktop.json");
        let mut d = DesktopSettings::default();
        d.icon_size = 100_000; // out of range on purpose
        save_to(&path, &d).unwrap();
        let got = load_from(&path);
        assert_eq!(got.icon_size, opusdm_ipc::ICON_SIZE_RANGE.1); // clamped
    }
}
