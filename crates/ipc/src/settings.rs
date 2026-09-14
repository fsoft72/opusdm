//! Centralized configuration path resolution and settings persistence.
//!
//! Stored under `~/.config/opusdm/` (`$XDG_CONFIG_HOME/opusdm`).
//! Shared across `opusdm-hub`, `opusdm-desktop-config`, and `opusdm-theme-config`
//! to ensure identical path resolution, directory creation, diagnostics,
//! and default-on-error behaviors without divergence.

use crate::desktop::DesktopSettings;
use crate::proto::Settings;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Base directory for OpusDM configuration files (`~/.config/opusdm`).
///
/// Falls back to `$HOME/.config/opusdm` then `<temp_dir>/opusdm` if
/// neither `XDG_CONFIG_HOME` nor `HOME` is set.
pub fn config_dir() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path
}

/// Path to `settings.json` under [`config_dir`].
pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

/// Path to `desktop.json` under [`config_dir`].
pub fn desktop_settings_path() -> PathBuf {
    config_dir().join("desktop.json")
}

/// Load a JSON configuration entity from `path`.
///
/// - If `path` does not exist: returns `T::default()` silently (clean first run).
/// - If reading `path` fails: logs an error with `app_tag` to stderr and returns `T::default()`.
/// - If parsing fails: logs an error with `app_tag` to stderr and returns `T::default()`.
pub fn load_json<T: DeserializeOwned + Default>(path: &Path, app_tag: &str, entity_name: &str) -> T {
    if !path.exists() {
        return T::default();
    }
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{app_tag}: cannot read {entity_name} from {}: {e}", path.display());
            return T::default();
        }
    };
    match serde_json::from_str(&content) {
        Ok(val) => val,
        Err(e) => {
            eprintln!("{app_tag}: malformed {entity_name} in {}: {e}", path.display());
            T::default()
        }
    }
}

/// Serialize and save a JSON configuration entity to `path`, creating parent
/// directories if necessary.
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(value)?;
    fs::write(path, json)?;
    Ok(())
}

/// Load `Settings` from the standard path (`~/.config/opusdm/settings.json`).
pub fn load_settings(app_tag: &str) -> Settings {
    load_settings_from(&settings_path(), app_tag)
}

/// Load `Settings` from an explicit path.
pub fn load_settings_from(path: &Path, app_tag: &str) -> Settings {
    load_json(path, app_tag, "settings")
}

/// Save `Settings` to the standard path (`~/.config/opusdm/settings.json`).
pub fn save_settings(settings: &Settings) -> io::Result<()> {
    save_settings_to(&settings_path(), settings)
}

/// Save `Settings` to an explicit path.
pub fn save_settings_to(path: &Path, settings: &Settings) -> io::Result<()> {
    save_json(path, settings)
}

/// Load `DesktopSettings` from the standard path (`~/.config/opusdm/desktop.json`),
/// always returning sanitized values.
pub fn load_desktop_settings(app_tag: &str) -> DesktopSettings {
    load_desktop_settings_from(&desktop_settings_path(), app_tag)
}

/// Load `DesktopSettings` from an explicit path, always returning sanitized values.
pub fn load_desktop_settings_from(path: &Path, app_tag: &str) -> DesktopSettings {
    load_json::<DesktopSettings>(path, app_tag, "desktop settings").sanitized()
}

/// Save `DesktopSettings` to the standard path (`~/.config/opusdm/desktop.json`).
pub fn save_desktop_settings(settings: &DesktopSettings) -> io::Result<()> {
    save_desktop_settings_to(&desktop_settings_path(), settings)
}

/// Save `DesktopSettings` to an explicit path.
pub fn save_desktop_settings_to(path: &Path, settings: &DesktopSettings) -> io::Result<()> {
    save_json(path, settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_under_config_dir() {
        let dir = config_dir();
        assert_eq!(settings_path(), dir.join("settings.json"));
        assert_eq!(desktop_settings_path(), dir.join("desktop.json"));
    }

    #[test]
    fn load_settings_missing_file_is_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        assert_eq!(load_settings_from(&path, "test"), Settings::default());
    }

    #[test]
    fn load_settings_malformed_file_reports_and_returns_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load_settings_from(&path, "test"), Settings::default());
    }

    #[test]
    fn save_and_load_settings_roundtrips() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("sub").join("settings.json");
        let mut s = Settings::default();
        s.appearance.monospace_list = true;
        save_settings_to(&path, &s).unwrap();
        let loaded = load_settings_from(&path, "test");
        assert!(loaded.appearance.monospace_list);
    }

    #[test]
    fn load_desktop_settings_missing_is_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("desktop.json");
        assert_eq!(load_desktop_settings_from(&path, "test"), DesktopSettings::default());
    }

    #[test]
    fn load_desktop_settings_sanitizes_clamped_values() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("desktop.json");
        let mut d = DesktopSettings::default();
        d.icon_size = 100_000;
        save_desktop_settings_to(&path, &d).unwrap();
        let loaded = load_desktop_settings_from(&path, "test");
        assert_eq!(loaded.icon_size, crate::desktop::ICON_SIZE_RANGE.1);
    }
}
