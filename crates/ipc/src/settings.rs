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

/// Base directory for OpusDM state files that are not configuration, such as
/// the last folder used by a tool (`~/.local/state/opusdm`).
///
/// Falls back to `$HOME/.local/state/opusdm` then `<temp_dir>/opusdm` if
/// neither `XDG_STATE_HOME` nor `HOME` is set.
pub fn state_dir() -> PathBuf {
    let mut path = dirs::state_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".local/state"))
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
/// - If parsing fails: copies the file to `<name>.bak` (an existing backup is kept),
///   logs an error with `app_tag` to stderr and returns `T::default()`. The copy
///   matters because the next save would otherwise overwrite what the user wrote.
pub fn load_json<T: DeserializeOwned + Default>(path: &Path, app_tag: &str, entity_name: &str) -> T {
    read_json(path, app_tag, entity_name).unwrap_or_default()
}

/// Like [`load_json`] but tells a file that could not be used (missing,
/// unreadable or malformed, the last one backed up to `<name>.bak`) from a valid
/// one, for callers that skip such files instead of falling back to a default.
pub fn read_json<T: DeserializeOwned>(path: &Path, app_tag: &str, entity_name: &str) -> Option<T> {
    if !path.exists() {
        return None;
    }
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            crate::err_log!(app_tag, "cannot read {entity_name} from {}: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_str(&content) {
        Ok(val) => Some(val),
        Err(e) => {
            crate::err_log!(app_tag, "malformed {entity_name} in {}: {e}", path.display());
            backup_malformed(path, app_tag);
            None
        }
    }
}

/// Copy the malformed file at `path` to `<name>.bak` unless a backup already
/// exists (the first broken copy is the one worth keeping). Failures are logged.
fn backup_malformed(path: &Path, app_tag: &str) {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".bak");
    let backup = path.with_file_name(name);
    if backup.exists() {
        return;
    }
    match fs::copy(path, &backup) {
        Ok(_) => crate::err_log!(app_tag, "a copy of the malformed file was kept in {}", backup.display()),
        Err(e) => crate::err_log!(app_tag, "could not back up {}: {e}", path.display()),
    }
}

/// Serialize and save a JSON configuration entity to `path`, creating parent
/// directories if necessary.
///
/// The write is atomic: the JSON goes to a sibling temporary file, is flushed
/// to disk and then renamed over `path`, so a crash or a full disk never leaves
/// a truncated file behind (the previous content stays intact).
pub fn save_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(value)?;
    let mut tmp_name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let written = write_synced(&tmp, json.as_bytes()).and_then(|_| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Write `data` to `path` and flush it to disk before returning.
fn write_synced(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = fs::File::create(path)?;
    file.write_all(data)?;
    file.sync_all()
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
    fn malformed_file_is_copied_to_bak_and_an_existing_bak_is_kept() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        let bak = td.path().join("settings.json.bak");
        fs::write(&path, b"{ first").unwrap();
        load_settings_from(&path, "test");
        assert_eq!(fs::read_to_string(&bak).unwrap(), "{ first");
        fs::write(&path, b"{ second").unwrap();
        load_settings_from(&path, "test");
        assert_eq!(fs::read_to_string(&bak).unwrap(), "{ first");
    }

    #[test]
    fn valid_file_leaves_no_bak() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        save_settings_to(&path, &Settings::default()).unwrap();
        load_settings_from(&path, "test");
        assert!(!td.path().join("settings.json.bak").exists());
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
    fn save_json_replaces_content_and_leaves_no_temp_file() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        save_json(&path, &vec![1, 2, 3]).unwrap();
        save_json(&path, &vec![4]).unwrap();
        let loaded: Vec<i32> = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded, vec![4]);
        let names: Vec<_> = fs::read_dir(td.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("settings.json")]);
    }

    #[test]
    fn save_json_failure_keeps_the_old_file() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        save_json(&path, &vec![1]).unwrap();
        // A map with non-string keys cannot be serialized: nothing may be touched.
        let mut bad = std::collections::HashMap::new();
        bad.insert(vec![1u8], 1);
        assert!(save_json(&path, &bad).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "[\n  1\n]");
        assert_eq!(fs::read_dir(td.path()).unwrap().count(), 1);
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
        let d = DesktopSettings {
            icon_size: 100_000,
            ..DesktopSettings::default()
        };
        save_desktop_settings_to(&path, &d).unwrap();
        let loaded = load_desktop_settings_from(&path, "test");
        assert_eq!(loaded.icon_size, crate::desktop::ICON_SIZE_RANGE.1);
    }
}
