//! `~/.config/opusdm/notifications.json`: where popups go, how long they
//! stay, Do Not Disturb and the muted applications. Only `opusdm-notify`
//! writes it; the panel changes DND and the mute list over D-Bus.

use crate::placement::Corner;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default popup time in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u32 = 6000;
/// Allowed popup times, in milliseconds.
pub const TIMEOUT_RANGE_MS: std::ops::RangeInclusive<u32> = 1000..=60_000;
/// Name of the file under the OpusDM configuration folder.
const FILE_NAME: &str = "notifications.json";

/// The notification settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Corner of the monitor the popups stack from.
    pub corner: Corner,
    /// Popup time when the application asks for the server default.
    pub timeout_ms: u32,
    /// Do Not Disturb: no popups (critical ones still show).
    pub dnd: bool,
    /// Application names that go to the history only.
    pub mute: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config { corner: Corner::TopRight, timeout_ms: DEFAULT_TIMEOUT_MS, dnd: false, mute: Vec::new() }
    }
}

impl Config {
    /// The popup time kept inside [`TIMEOUT_RANGE_MS`].
    pub fn timeout_ms(&self) -> u32 {
        self.timeout_ms.clamp(*TIMEOUT_RANGE_MS.start(), *TIMEOUT_RANGE_MS.end())
    }

    /// Whether `app_name` is muted (names compare without case).
    pub fn is_muted(&self, app_name: &str) -> bool {
        self.mute.iter().any(|m| m.eq_ignore_ascii_case(app_name))
    }

    /// Mute `app_name`; `false` when it already was.
    pub fn mute(&mut self, app_name: &str) -> bool {
        if app_name.is_empty() || self.is_muted(app_name) { return false; }
        self.mute.push(app_name.to_string());
        true
    }

    /// Unmute `app_name`; `false` when it was not muted.
    pub fn unmute(&mut self, app_name: &str) -> bool {
        let before = self.mute.len();
        self.mute.retain(|m| !m.eq_ignore_ascii_case(app_name));
        self.mute.len() != before
    }
}

/// Path of the settings file.
pub fn path() -> PathBuf {
    opusdm_ipc::settings::config_dir().join(FILE_NAME)
}

/// Read the settings; defaults when the file is missing or broken (a broken
/// file is kept as `.bak`).
pub fn load() -> Config {
    opusdm_ipc::settings::load_json(&path(), "opusdm-notify", "notification settings")
}

/// Write the settings; a failure is logged.
pub fn save(config: &Config) {
    if let Err(e) = opusdm_ipc::settings::save_json(&path(), config) {
        opusdm_ipc::err_log!("opusdm-notify", "cannot save {}: {e}", path().display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let c: Config = serde_json::from_str(r#"{ "corner": "bottom-left" }"#).unwrap();
        assert_eq!(c, Config { corner: Corner::BottomLeft, ..Config::default() });
    }

    #[test]
    fn the_timeout_is_clamped() {
        assert_eq!(Config { timeout_ms: 5, ..Config::default() }.timeout_ms(), 1000);
        assert_eq!(Config { timeout_ms: 999_999, ..Config::default() }.timeout_ms(), 60_000);
    }

    #[test]
    fn mute_and_unmute_ignore_case() {
        let mut c = Config::default();
        assert!(c.mute("Spotify"));
        assert!(!c.mute("spotify"));
        assert!(c.is_muted("SPOTIFY"));
        assert!(c.unmute("spotify"));
        assert!(!c.unmute("spotify"));
        assert!(!c.mute(""));
    }
}
