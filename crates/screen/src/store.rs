//! The saved layouts, one per set of connected monitors, in `screens.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Layout, Monitor, ScreenError};

/// Tag used in the log lines of the shared JSON helpers.
const APP_TAG: &str = "opusdm-screen";

/// A layout and the monitors it is for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// The sorted EDID ids of the monitors (see `profile_key`).
    pub key: Vec<String>,
    pub layout: Layout,
}

/// All the saved profiles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Profiles {
    pub profiles: Vec<Profile>,
}

/// The key of a set of monitors: their EDID ids, sorted. The same monitors on
/// other ports give the same key.
pub fn profile_key(monitors: &[Monitor]) -> Vec<String> {
    let mut key: Vec<String> = monitors.iter().map(|m| m.edid_id.clone()).collect();
    key.sort();
    key
}

impl Profiles {
    /// Save `layout` for `monitors`, replacing the profile of the same
    /// monitors.
    pub fn set(&mut self, monitors: &[Monitor], layout: Layout) {
        let key = profile_key(monitors);
        match self.profiles.iter_mut().find(|p| p.key == key) {
            Some(profile) => profile.layout = layout,
            None => self.profiles.push(Profile { key, layout }),
        }
    }

    /// The saved layout for `monitors`, adapted to their output names. `None`
    /// when there is none, or when it no longer fits the monitors.
    pub fn find(&self, monitors: &[Monitor]) -> Option<Layout> {
        let key = profile_key(monitors);
        self.profiles.iter().find(|p| p.key == key)?.layout.remapped_to(monitors)
    }
}

/// Path of `screens.json` under the OpusDM config directory.
pub fn profiles_path() -> PathBuf {
    opusdm_ipc::settings::config_dir().join("screens.json")
}

/// Load the profiles saved at `path`. `None` when the file is missing or
/// unusable (a malformed file is copied to `screens.json.bak` by the shared
/// helper), so callers leave the screens alone in that case.
pub fn load_from(path: &Path) -> Option<Profiles> {
    opusdm_ipc::settings::read_json(path, APP_TAG, "screen profiles")
}

/// Write `profiles` to `path` (atomically, creating the folder).
pub fn save_to(path: &Path, profiles: &Profiles) -> Result<(), ScreenError> {
    opusdm_ipc::settings::save_json(path, profiles)
        .map_err(|e| ScreenError::Write { path: path.display().to_string(), reason: e.to_string() })
}

/// Save `layout` for `monitors` in the file at `path`, keeping the other
/// profiles.
pub fn remember(path: &Path, monitors: &[Monitor], layout: Layout) -> Result<(), ScreenError> {
    let mut profiles = load_from(path).unwrap_or_default();
    profiles.set(monitors, layout);
    save_to(path, &profiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::two_monitor_system;

    #[test]
    fn the_key_is_sorted_and_ignores_the_port() {
        let mut ms = two_monitor_system().monitors;
        let key = profile_key(&ms);
        assert_eq!(key, ["aaaa", "bbbb"]);
        ms.reverse();
        ms[0].name = "DP-3".to_string();
        assert_eq!(profile_key(&ms), key);
    }

    #[test]
    fn set_replaces_the_profile_of_the_same_monitors() {
        let sys = two_monitor_system();
        let mut profiles = Profiles::default();
        profiles.set(&sys.monitors, sys.layout.clone());
        let mut other = sys.layout.clone();
        other.monitors[1].pos = (0, 1600);
        profiles.set(&sys.monitors, other.clone());
        assert_eq!(profiles.profiles.len(), 1);
        assert_eq!(profiles.find(&sys.monitors), Some(other));
    }

    #[test]
    fn find_is_none_for_other_monitors_and_stale_layouts() {
        let sys = two_monitor_system();
        let mut profiles = Profiles::default();
        profiles.set(&sys.monitors, sys.layout.clone());
        assert_eq!(profiles.find(&sys.monitors[..1]), None);

        let mut changed = sys.monitors.clone();
        changed[1].modes.clear();
        assert_eq!(profiles.find(&changed), None, "the saved resolution is gone");
    }

    #[test]
    fn find_follows_a_monitor_to_another_port() {
        let sys = two_monitor_system();
        let mut profiles = Profiles::default();
        profiles.set(&sys.monitors, sys.layout.clone());
        let mut moved = sys.monitors.clone();
        moved[1].name = "DP-2".to_string();
        assert_eq!(profiles.find(&moved).unwrap().monitors[1].name, "DP-2");
    }

    #[test]
    fn remember_round_trips_and_keeps_other_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("screens.json");
        let sys = two_monitor_system();
        remember(&path, &sys.monitors, sys.layout.clone()).unwrap();
        remember(&path, &sys.monitors[..1], Layout { monitors: sys.layout.monitors[..1].to_vec() }).unwrap();
        let profiles = load_from(&path).unwrap();
        assert_eq!(profiles.profiles.len(), 2);
        assert_eq!(profiles.find(&sys.monitors), Some(sys.layout));
    }

    #[test]
    fn a_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_from(&dir.path().join("screens.json")), None);
    }

    #[test]
    fn a_malformed_file_is_none_and_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screens.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), None);
        assert!(dir.path().join("screens.json.bak").exists());
    }
}
