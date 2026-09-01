//! Settings persistence: stored in ~/.config/opusdm/settings.json.
//!
//! The on-disk shape is `opusdm_ipc::Settings` itself - the same struct the
//! hub keeps in memory (`server::owner_loop`'s `current_settings`) and sends
//! to Listers in `Welcome`/`SettingsUpdate`. One struct, three roles (file
//! format, in-memory state, wire format), so a new setting is one field
//! added in one place rather than three kept in sync by hand.

use opusdm_ipc::Settings;
use std::fs;
use std::path::PathBuf;

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
    let path = config_path();
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(settings) = serde_json::from_str(&content) {
                return settings;
            }
        }
    }
    Settings::default()
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    fs::write(path, json)?;
    Ok(())
}
