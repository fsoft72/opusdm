//! The options of `opusdm-shot` that outlive one run, kept in
//! `~/.config/opusdm/shot.json`.

use opusdm_ipc::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Longest delay offered in the editor, in seconds.
pub const MAX_DELAY_SECS: u32 = 60;

/// Persistent options of the screenshot tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShotConfig {
    /// Seconds to wait before the screen is captured (0 = at once).
    pub delay_secs: u32,
}

impl ShotConfig {
    /// The config with every value inside its allowed range.
    fn sanitized(mut self) -> Self {
        self.delay_secs = self.delay_secs.min(MAX_DELAY_SECS);
        self
    }
}

/// Path to `shot.json` under the OpusDM config directory.
fn config_path() -> PathBuf {
    config_dir().join("shot.json")
}

/// Load the config; a missing or malformed file gives the defaults.
pub fn load() -> ShotConfig {
    load_from(&config_path())
}

/// Save the config, logging a failure (a lost option is not worth a crash).
pub fn save(config: &ShotConfig) {
    if let Err(e) = save_to(&config_path(), config) {
        opusdm_ipc::err_log!("opusdm-shot", "cannot save the options: {e}");
    }
}

/// Load the config from `path`.
fn load_from(path: &Path) -> ShotConfig {
    load_json::<ShotConfig>(path, "opusdm-shot", "shot options").sanitized()
}

/// Save the config to `path`.
fn save_to(path: &Path, config: &ShotConfig) -> std::io::Result<()> {
    save_json(path, &config.sanitized())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_no_delay() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_from(&dir.path().join("shot.json")).delay_secs, 0);
    }

    #[test]
    fn the_delay_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.json");
        save_to(&path, &ShotConfig { delay_secs: 5 }).unwrap();
        assert_eq!(load_from(&path).delay_secs, 5);
    }

    #[test]
    fn a_huge_delay_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.json");
        std::fs::write(&path, r#"{"delay_secs": 99999}"#).unwrap();
        assert_eq!(load_from(&path).delay_secs, MAX_DELAY_SECS);
    }
}
