//! The options of `opusdm-terminal` that outlive one run, kept in
//! `~/.config/opusdm/terminal.json`: theme, font, scrollback, cursor and the
//! hotkeys of the terminal actions.

use crate::actions::Action;
use opusdm_ipc::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Scrollback kept by a new terminal unless the user picks another value.
pub const DEFAULT_SCROLLBACK_LINES: u32 = 2000;

/// Largest finite scrollback accepted (0 already means unlimited).
pub const MAX_SCROLLBACK_LINES: u32 = 1_000_000;

/// Font used when the config does not name one.
pub const DEFAULT_FONT: &str = "Monospace 11";

/// Shape of the text cursor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    #[default]
    Block,
    IBeam,
    Underline,
}

impl CursorShape {
    /// Every shape, in the order the Preferences dialog lists them.
    pub const ALL: [CursorShape; 3] = [CursorShape::Block, CursorShape::IBeam, CursorShape::Underline];

    /// Label shown in the Preferences dialog.
    pub fn label(self) -> &'static str {
        match self {
            CursorShape::Block => "Block",
            CursorShape::IBeam => "I-Beam",
            CursorShape::Underline => "Underline",
        }
    }
}

/// Persistent options of the terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Name of the colour theme (a built-in preset or a user theme file).
    pub theme: String,
    /// Pango font description, e.g. `"Monospace 11"`.
    pub font: String,
    /// Lines of scrollback per terminal; 0 means unlimited.
    pub scrollback_lines: u32,
    /// Shape of the text cursor.
    pub cursor_shape: CursorShape,
    /// Whether the cursor blinks.
    pub cursor_blink: bool,
    /// Whether the bell makes a sound.
    pub audible_bell: bool,
    /// Whether bold text uses the bright palette colours.
    pub bold_is_bright: bool,
    /// Accelerator (GTK syntax, e.g. `<Ctrl><Shift>o`) per action id. A
    /// missing entry falls back to the default; an empty string disables it.
    pub hotkeys: BTreeMap<String, String>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT_THEME.to_string(),
            font: DEFAULT_FONT.to_string(),
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            cursor_shape: CursorShape::default(),
            cursor_blink: true,
            audible_bell: false,
            bold_is_bright: true,
            hotkeys: BTreeMap::new(),
        }
    }
}

impl TerminalConfig {
    /// The config with every value inside its allowed range.
    fn sanitized(mut self) -> Self {
        self.scrollback_lines = self.scrollback_lines.min(MAX_SCROLLBACK_LINES);
        if self.font.trim().is_empty() {
            self.font = DEFAULT_FONT.to_string();
        }
        self
    }

    /// The accelerator bound to `action`: the user's entry when present
    /// (possibly empty, meaning "no hotkey"), otherwise the default.
    pub fn hotkey(&self, action: Action) -> String {
        match self.hotkeys.get(action.id()) {
            Some(accel) => accel.trim().to_string(),
            None => action.default_hotkey().to_string(),
        }
    }
}

/// The value VTE expects for a scrollback of `lines`: VTE takes -1 for an
/// unlimited buffer, while the config uses 0.
pub fn vte_scrollback(lines: u32) -> i64 {
    if lines == 0 { -1 } else { i64::from(lines) }
}

/// Path to `terminal.json` under the OpusDM config directory.
fn config_path() -> PathBuf {
    config_dir().join("terminal.json")
}

/// Load the config; a missing or malformed file gives the defaults.
pub fn load() -> TerminalConfig {
    load_from(&config_path())
}

/// Save the config, logging a failure (a lost option is not worth a crash).
pub fn save(config: &TerminalConfig) {
    if let Err(e) = save_to(&config_path(), config) {
        opusdm_ipc::err_log!("opusdm-terminal", "cannot save the options: {e}");
    }
}

/// Load the config from `path`.
fn load_from(path: &Path) -> TerminalConfig {
    load_json::<TerminalConfig>(path, "opusdm-terminal", "terminal options").sanitized()
}

/// Save the config to `path`.
fn save_to(path: &Path, config: &TerminalConfig) -> std::io::Result<()> {
    save_json(path, &config.clone().sanitized())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_from(&dir.path().join("terminal.json"));
        assert_eq!(config, TerminalConfig::default());
        assert_eq!(config.scrollback_lines, 2000);
    }

    #[test]
    fn the_config_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminal.json");
        let mut config = TerminalConfig { theme: "Dracula".into(), font: "Hack 13".into(), scrollback_lines: 0, ..Default::default() };
        config.hotkeys.insert("split-vertical".into(), "<Alt>v".into());
        save_to(&path, &config).unwrap();
        assert_eq!(load_from(&path), config);
    }

    #[test]
    fn a_huge_scrollback_is_clamped_and_an_empty_font_restored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminal.json");
        std::fs::write(&path, r#"{"scrollback_lines": 4000000000, "font": " "}"#).unwrap();
        let config = load_from(&path);
        assert_eq!(config.scrollback_lines, MAX_SCROLLBACK_LINES);
        assert_eq!(config.font, DEFAULT_FONT);
    }

    #[test]
    fn zero_scrollback_means_unlimited_for_vte() {
        assert_eq!(vte_scrollback(0), -1);
        assert_eq!(vte_scrollback(2000), 2000);
    }

    #[test]
    fn hotkeys_fall_back_to_the_defaults_and_can_be_disabled() {
        let mut config = TerminalConfig::default();
        assert_eq!(config.hotkey(Action::SplitVertical), Action::SplitVertical.default_hotkey());
        config.hotkeys.insert(Action::SplitVertical.id().into(), String::new());
        assert_eq!(config.hotkey(Action::SplitVertical), "");
    }
}
