//! Desktop settings persistence: `~/.config/opusdm/desktop.json`.
//!
//! Same three-roles-one-struct design as [`crate::settings`]: the on-disk
//! shape is `opusdm_ipc::DesktopSettings`, which is also the hub's in-memory
//! copy and the payload of `ToHub::SetDesktopSettings`.

use opusdm_ipc::DesktopSettings;
use std::fs;
use std::path::{Path, PathBuf};

/// Path to `~/.config/opusdm/desktop.json`. Falls back to `$HOME/.config`
/// then the temp dir when `dirs::config_dir()` yields nothing, exactly like
/// [`crate::settings`].
pub fn config_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("desktop.json");
    path
}

/// Load desktop settings from the standard path. Missing or unparseable ->
/// [`DesktopSettings::default`]. Always clamped via
/// [`DesktopSettings::sanitized`].
pub fn load() -> DesktopSettings {
    load_from(&config_path())
}

/// Persist `settings` to the standard path, creating the directory.
pub fn save(settings: &DesktopSettings) -> std::io::Result<()> {
    save_to(&config_path(), settings)
}

/// Testable core of [`load`].
fn load_from(path: &Path) -> DesktopSettings {
    if !path.exists() {
        return DesktopSettings::default();
    }
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("opusdm-hub: cannot read desktop settings from {}: {e}", path.display());
            return DesktopSettings::default();
        }
    };
    match serde_json::from_str::<DesktopSettings>(&content) {
        Ok(settings) => settings.sanitized(),
        Err(e) => {
            eprintln!("opusdm-hub: malformed desktop settings in {}: {e}", path.display());
            DesktopSettings::default().sanitized()
        }
    }
}

/// Testable core of [`save`].
fn save_to(path: &Path, settings: &DesktopSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

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
