//! Dock launcher list: `~/.config/opusdm/dock.json`.
//!
//! Owned by `opusdm-dock` (drag and drop edits) and `opusdm-settings` (the Dock
//! tab); the dock watches the file, so either side can write it.

use crate::settings::{config_dir, save_json};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// On-disk shape of `dock.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DockConfig {
    /// Pinned apps as desktop file ids (`firefox.desktop`), in dock order.
    pub apps: Vec<String>,
}

/// Path to `dock.json` under [`config_dir`].
pub fn dock_config_path() -> PathBuf {
    config_dir().join("dock.json")
}

/// Load the dock config from the standard path. `None` when the file is
/// missing or malformed, so the caller can fall back to its defaults.
pub fn load_dock_config() -> Option<DockConfig> {
    load_dock_config_from(&dock_config_path())
}

/// Testable core of [`load_dock_config`].
pub fn load_dock_config_from(path: &Path) -> Option<DockConfig> {
    crate::settings::read_json(path, "ipc", "dock config")
}

/// Save the dock config to the standard path, creating the directory.
pub fn save_dock_config(cfg: &DockConfig) -> io::Result<()> {
    save_dock_config_to(&dock_config_path(), cfg)
}

/// Testable core of [`save_dock_config`].
pub fn save_dock_config_to(path: &Path, cfg: &DockConfig) -> io::Result<()> {
    save_json(path, cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_none() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(load_dock_config_from(&td.path().join("dock.json")), None);
    }

    #[test]
    fn malformed_file_is_none() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("dock.json");
        std::fs::write(&path, b"nope").unwrap();
        assert_eq!(load_dock_config_from(&path), None);
        assert_eq!(std::fs::read_to_string(td.path().join("dock.json.bak")).unwrap(), "nope", "the broken file is kept");
    }

    #[test]
    fn save_then_load_roundtrips_in_order() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("nested").join("dock.json");
        let cfg = DockConfig { apps: vec!["b.desktop".into(), "a.desktop".into()] };
        save_dock_config_to(&path, &cfg).unwrap();
        assert_eq!(load_dock_config_from(&path), Some(cfg));
    }

    #[test]
    fn empty_object_is_an_empty_list_not_defaults() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("dock.json");
        std::fs::write(&path, b"{}").unwrap();
        assert_eq!(load_dock_config_from(&path), Some(DockConfig::default()));
    }
}
