//! Settings persistence: stored in ~/.config/opusdm/settings.json.
//!
//! The on-disk shape is `opusdm_ipc::Settings` itself - the same struct the
//! hub keeps in memory (`server::owner_loop`'s `current_settings`) and sends
//! to Listers in `Welcome`/`SettingsUpdate`. One struct, three roles (file
//! format, in-memory state, wire format), so a new setting is one field
//! added in one place rather than three kept in sync by hand.

use opusdm_ipc::Settings;
use std::fs;
use std::path::{Path, PathBuf};

fn config_path() -> PathBuf {
    // dirs::config_dir() fails only when neither XDG_CONFIG_HOME nor HOME is
    // set; fall back to $HOME/.config built by hand rather than a literal
    // "~" path, which fs calls never expand.
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("settings.json");
    path
}

pub fn load_settings() -> Settings {
    load_settings_from(&config_path())
}

/// Testable core of [`load_settings`].
pub fn load_settings_from(path: &Path) -> Settings {
    if !path.exists() {
        return Settings::default();
    }
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("opusdm-hub: cannot read settings from {}: {e}", path.display());
            return Settings::default();
        }
    };
    match serde_json::from_str(&content) {
        Ok(settings) => settings,
        Err(e) => {
            eprintln!("opusdm-hub: malformed settings in {}: {e}", path.display());
            Settings::default()
        }
    }
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    save_settings_to(&config_path(), settings)
}

/// Testable core of [`save_settings`].
pub fn save_settings_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
