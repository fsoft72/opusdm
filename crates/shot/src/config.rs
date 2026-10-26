//! The options of `opusdm-shot` that outlive one run, kept in
//! `~/.config/opusdm/shot.json`.

use opusdm_ipc::settings::{config_dir, load_json, save_json, state_dir};
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

/// Runtime state of the tool, kept apart from the options in
/// `~/.local/state/opusdm/shot.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct ShotState {
    /// Folder of the last image saved with "Save as", offered next time.
    last_dir: Option<PathBuf>,
}

/// Path to the state file under the OpusDM state directory.
fn state_path() -> PathBuf {
    state_dir().join("shot.json")
}

/// The folder of the last image saved with "Save as", if one is stored.
pub fn last_dir() -> Option<PathBuf> {
    load_json::<ShotState>(&state_path(), "opusdm-shot", "shot state").last_dir
}

/// Remember the folder of `saved` as the one to offer in the next save dialog.
pub fn remember_dir(saved: &Path) {
    let Some(dir) = saved.parent() else { return };
    if let Err(e) = remember_dir_in(&state_path(), dir) {
        opusdm_ipc::err_log!("opusdm-shot", "cannot save the last folder: {e}");
    }
}

/// Store `dir` as the last folder in the state file `path`.
fn remember_dir_in(path: &Path, dir: &Path) -> std::io::Result<()> {
    let state = ShotState { last_dir: Some(dir.to_path_buf()) };
    if load_json::<ShotState>(path, "opusdm-shot", "shot state") == state { return Ok(()); }
    save_json(path, &state)
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
    fn the_last_dir_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("shot.json");
        remember_dir_in(&path, Path::new("/tmp/x")).unwrap();
        let state: ShotState = load_json(&path, "opusdm-shot", "shot state");
        assert_eq!(state.last_dir, Some(PathBuf::from("/tmp/x")));
    }

    #[test]
    fn a_missing_state_file_gives_no_dir() {
        let dir = tempfile::tempdir().unwrap();
        let state: ShotState = load_json(&dir.path().join("shot.json"), "opusdm-shot", "shot state");
        assert_eq!(state.last_dir, None);
    }

    #[test]
    fn a_huge_delay_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.json");
        std::fs::write(&path, r#"{"delay_secs": 99999}"#).unwrap();
        assert_eq!(load_from(&path).delay_secs, MAX_DELAY_SECS);
    }
}
