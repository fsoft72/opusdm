//! The options of `opusdm-shot` that outlive one run, kept in
//! `~/.config/opusdm/shot.json`.

use crate::export::{self, Format, Options};
use opusdm_ipc::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Longest delay offered in the settings, in seconds.
pub const MAX_DELAY_SECS: u32 = 60;

/// File name pattern used until the user changes it. `YYYYMMDD` and `HHMMSS`
/// are replaced with the local date and time (see `export::expand_name`).
pub const DEFAULT_NAME: &str = "shot_YYYYMMDD_HHMMSS";

/// Quality offered for JPG and WebP until the user changes it.
pub const DEFAULT_QUALITY: u8 = 80;

/// Persistent options of the screenshot tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShotConfig {
    /// Seconds to wait before the screen is captured (0 = at once).
    pub delay_secs: u32,
    /// Folder where the images are saved (`~/Pictures/Screenshots` by default).
    pub dir: PathBuf,
    /// File name pattern, without the extension.
    pub name: String,
    /// Extension of the default format (`png`, `jpg` or `webp`).
    pub format: String,
    /// 1-100, used by JPG and lossy WebP.
    pub quality: u8,
    /// Copy the screenshot to the clipboard without being asked.
    pub auto_copy: bool,
}

impl Default for ShotConfig {
    fn default() -> Self {
        ShotConfig {
            delay_secs: 0,
            dir: default_dir(),
            name: DEFAULT_NAME.to_string(),
            format: Format::Png.extension().to_string(),
            quality: DEFAULT_QUALITY,
            auto_copy: false,
        }
    }
}

impl ShotConfig {
    /// The config with every value inside its allowed range.
    fn sanitized(mut self) -> Self {
        self.delay_secs = self.delay_secs.min(MAX_DELAY_SECS);
        self.quality = self.quality.clamp(export::MIN_QUALITY, export::MAX_QUALITY);
        if self.dir.as_os_str().is_empty() { self.dir = default_dir(); }
        if self.name.trim().is_empty() { self.name = DEFAULT_NAME.to_string(); }
        if self.format_enum().extension() != self.format { self.format = Format::Png.extension().to_string(); }
        self
    }

    /// The default format; PNG when the stored name is unknown.
    pub fn format_enum(&self) -> Format {
        Format::ALL.iter().copied().find(|f| f.extension() == self.format).unwrap_or(Format::Png)
    }

    /// The encoding options of the default format.
    pub fn options(&self) -> Options {
        Options { format: self.format_enum(), quality: self.quality, lossless: false }
    }
}

/// `~/Pictures/Screenshots`, or `./Screenshots` when there is no home folder.
fn default_dir() -> PathBuf {
    dirs::picture_dir().or_else(|| dirs::home_dir().map(|h| h.join("Pictures"))).unwrap_or_default().join("Screenshots")
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
    save_json(path, &config.clone().sanitized())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_from(&dir.path().join("shot.json"));
        assert_eq!(config.delay_secs, 0);
        assert_eq!(config.name, DEFAULT_NAME);
        assert_eq!(config.quality, 80);
        assert!(!config.auto_copy);
        assert_eq!(config.options().format, Format::Png);
        assert!(config.dir.ends_with("Screenshots"));
    }

    #[test]
    fn the_options_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.json");
        let config = ShotConfig { delay_secs: 5, dir: "/tmp/x".into(), name: "a_YYYYMMDD".into(), format: "webp".into(), quality: 55, auto_copy: true };
        save_to(&path, &config).unwrap();
        assert_eq!(load_from(&path), config);
        assert_eq!(config.options(), Options { format: Format::WebP, quality: 55, lossless: false });
    }

    #[test]
    fn an_old_file_with_only_the_delay_keeps_the_other_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.json");
        std::fs::write(&path, r#"{"delay_secs": 3}"#).unwrap();
        let config = load_from(&path);
        assert_eq!(config.delay_secs, 3);
        assert_eq!(config.name, DEFAULT_NAME);
    }

    #[test]
    fn out_of_range_values_are_fixed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.json");
        std::fs::write(&path, r#"{"delay_secs": 99999, "quality": 0, "name": "  ", "dir": "", "format": "bmp"}"#).unwrap();
        let config = load_from(&path);
        assert_eq!(config.delay_secs, MAX_DELAY_SECS);
        assert_eq!(config.quality, export::MIN_QUALITY);
        assert_eq!(config.name, DEFAULT_NAME);
        assert_eq!(config.format, "png");
        assert!(config.dir.ends_with("Screenshots"));
    }
}
