//! Colour themes of the terminal, separate from the Lister/desktop themes:
//! built-in presets plus user themes read from
//! `~/.config/opusdm/terminal-themes/*.json`. Pure data, no GTK.

use opusdm_ipc::settings::{config_dir, read_json, save_json};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Theme used when the config names none or names one that is gone.
pub const DEFAULT_THEME: &str = "OpusDM Dark";

/// Number of colours in a terminal palette (8 normal + 8 bright).
pub const PALETTE_SIZE: usize = 16;

/// A terminal colour theme. Colours are `#rgb` or `#rrggbb` strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalTheme {
    /// Name shown in the Preferences dialog and stored in the config.
    pub name: String,
    /// Default text colour.
    pub foreground: String,
    /// Default background colour.
    pub background: String,
    /// Cursor colour; the foreground when missing.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Colour of the text under the cursor; the background when missing.
    #[serde(default)]
    pub cursor_foreground: Option<String>,
    /// Background of selected text; VTE's own default when missing.
    #[serde(default)]
    pub selection: Option<String>,
    /// Text colour of selected text; VTE's own default when missing.
    #[serde(default)]
    pub selection_foreground: Option<String>,
    /// The 16 ANSI colours: black, red, green, yellow, blue, magenta, cyan,
    /// white, then their bright variants.
    pub palette: Vec<String>,
}

/// An RGB colour with channels in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

/// Parse `#rgb` or `#rrggbb` into an [`Rgb`].
pub fn parse_hex(text: &str) -> Option<Rgb> {
    let hex = text.trim().strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) { return None; }
    let channel = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| f32::from(v) / 255.0);
    let (r, g, b) = match hex.len() {
        3 => {
            let doubled: Vec<String> = hex.chars().map(|c| format!("{c}{c}")).collect();
            (channel(&doubled[0])?, channel(&doubled[1])?, channel(&doubled[2])?)
        }
        6 => (channel(&hex[0..2])?, channel(&hex[2..4])?, channel(&hex[4..6])?),
        _ => return None,
    };
    Some(Rgb { red: r, green: g, blue: b })
}

/// Format an [`Rgb`] as `#rrggbb`.
pub fn to_hex(colour: Rgb) -> String {
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", channel(colour.red), channel(colour.green), channel(colour.blue))
}

/// File name for a user theme called `name`: lower-case letters and digits,
/// every other run of characters turned into one `-`.
pub fn theme_file_name(name: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    format!("{}.json", if slug.is_empty() { "theme" } else { slug })
}

/// Where the user theme called `name` is stored inside `dir`.
pub fn user_theme_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(theme_file_name(name))
}

/// Validate `theme` and write it into `dir` (created if needed), returning
/// the path of the file.
pub fn save_user_theme(dir: &Path, theme: &TerminalTheme) -> Result<PathBuf, String> {
    theme.validate()?;
    let path = user_theme_path(dir, &theme.name);
    save_json(&path, theme).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

impl TerminalTheme {
    /// Check that every colour parses and the palette is complete,
    /// describing the first problem found.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() { return Err("the theme has no name".into()); }
        if self.palette.len() != PALETTE_SIZE {
            return Err(format!("the palette has {} colours, {PALETTE_SIZE} expected", self.palette.len()));
        }
        let optional = [&self.cursor, &self.cursor_foreground, &self.selection, &self.selection_foreground];
        let colours = [&self.foreground, &self.background]
            .into_iter()
            .chain(optional.into_iter().flatten())
            .chain(self.palette.iter());
        for colour in colours {
            if parse_hex(colour).is_none() { return Err(format!("invalid colour {colour:?}")); }
        }
        Ok(())
    }
}

/// Build a preset from string slices (presets are known to be valid).
fn preset(name: &str, fg: &str, bg: &str, cursor: &str, selection: &str, palette: [&str; PALETTE_SIZE]) -> TerminalTheme {
    TerminalTheme {
        name: name.to_string(),
        foreground: fg.to_string(),
        background: bg.to_string(),
        cursor: Some(cursor.to_string()),
        cursor_foreground: Some(bg.to_string()),
        selection: Some(selection.to_string()),
        selection_foreground: None,
        palette: palette.iter().map(|c| c.to_string()).collect(),
    }
}

/// The themes compiled into the binary.
pub fn presets() -> Vec<TerminalTheme> {
    vec![
        preset(DEFAULT_THEME, "#eff0f1", "#1b1e20", "#eff0f1", "#3daee9", [
            "#232627", "#ed1515", "#11d116", "#f67400", "#1d99f3", "#9b59b6", "#1abc9c", "#fcfcfc",
            "#7f8c8d", "#c0392b", "#1cdc9a", "#fdbc4b", "#3daee9", "#8e44ad", "#16a085", "#ffffff",
        ]),
        preset("Tango Dark", "#d3d7cf", "#2e3436", "#d3d7cf", "#555753", [
            "#2e3436", "#cc0000", "#4e9a06", "#c4a000", "#3465a4", "#75507b", "#06989a", "#d3d7cf",
            "#555753", "#ef2929", "#8ae234", "#fce94f", "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec",
        ]),
        preset("Tango Light", "#2e3436", "#ffffff", "#2e3436", "#c4d7f1", [
            "#2e3436", "#cc0000", "#4e9a06", "#c4a000", "#3465a4", "#75507b", "#06989a", "#d3d7cf",
            "#555753", "#ef2929", "#8ae234", "#fce94f", "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec",
        ]),
        preset("Solarized Dark", "#839496", "#002b36", "#93a1a1", "#073642", [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
            "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ]),
        preset("Solarized Light", "#657b83", "#fdf6e3", "#586e75", "#eee8d5", [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
            "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ]),
        preset("Dracula", "#f8f8f2", "#282a36", "#f8f8f2", "#44475a", [
            "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
            "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
        ]),
        preset("Gruvbox Dark", "#ebdbb2", "#282828", "#ebdbb2", "#504945", [
            "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
            "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ]),
        preset("Nord", "#d8dee9", "#2e3440", "#d8dee9", "#434c5e", [
            "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
            "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
        ]),
        preset("One Dark", "#abb2bf", "#282c34", "#528bff", "#3e4451", [
            "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
            "#5c6370", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff",
        ]),
        preset("Classic Amiga", "#ffffff", "#0055aa", "#ff8800", "#ff8800", [
            "#000000", "#cc0000", "#00aa00", "#ffaa00", "#0055aa", "#aa00aa", "#00aaaa", "#ffffff",
            "#555555", "#ff5555", "#55ff55", "#ffff55", "#5599ff", "#ff55ff", "#55ffff", "#ffffff",
        ]),
    ]
}

/// `~/.config/opusdm/terminal-themes`: one JSON file per user theme.
pub fn user_themes_dir() -> PathBuf {
    config_dir().join("terminal-themes")
}

/// Read every valid `*.json` theme in `dir`, sorted by name. Invalid files
/// are logged and skipped; a missing directory gives no themes.
pub fn load_user_themes_from(dir: &Path) -> Vec<TerminalTheme> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut themes: Vec<TerminalTheme> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|p| {
            let theme: TerminalTheme = read_json(&p, "opusdm-terminal", "terminal theme")?;
            match theme.validate() {
                Ok(()) => Some(theme),
                Err(e) => {
                    opusdm_ipc::err_log!("opusdm-terminal", "skipping theme {}: {e}", p.display());
                    None
                }
            }
        })
        .collect();
    themes.sort_by_key(|t| t.name.to_lowercase());
    themes
}

/// Every available theme: the presets, then the user themes. A user theme
/// with the name of a preset replaces it.
pub fn all_themes() -> Vec<TerminalTheme> {
    merge(presets(), load_user_themes_from(&user_themes_dir()))
}

/// Presets followed by user themes, a user theme replacing a preset of the
/// same name in place.
fn merge(mut presets: Vec<TerminalTheme>, user: Vec<TerminalTheme>) -> Vec<TerminalTheme> {
    for theme in user {
        match presets.iter_mut().find(|p| p.name == theme.name) {
            Some(slot) => *slot = theme,
            None => presets.push(theme),
        }
    }
    presets
}

/// The theme called `name` in `themes`, or the default one.
pub fn find<'a>(themes: &'a [TerminalTheme], name: &str) -> &'a TerminalTheme {
    themes
        .iter()
        .find(|t| t.name == name)
        .or_else(|| themes.iter().find(|t| t.name == DEFAULT_THEME))
        .unwrap_or(&themes[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_valid() {
        for theme in presets() {
            assert_eq!(theme.validate(), Ok(()), "{}", theme.name);
        }
    }

    #[test]
    fn hex_colours_parse_in_short_and_long_form() {
        assert_eq!(parse_hex("#fff"), Some(Rgb { red: 1.0, green: 1.0, blue: 1.0 }));
        assert_eq!(parse_hex("#000000"), Some(Rgb { red: 0.0, green: 0.0, blue: 0.0 }));
        assert_eq!(parse_hex("ffffff"), None);
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#gggggg"), None);
    }

    #[test]
    fn hex_round_trips() {
        for hex in ["#000000", "#ffffff", "#3daee9", "#0055aa"] {
            assert_eq!(to_hex(parse_hex(hex).unwrap()), hex);
        }
    }

    #[test]
    fn theme_names_become_safe_file_names() {
        assert_eq!(theme_file_name("My Theme"), "my-theme.json");
        assert_eq!(theme_file_name("  Dracula (custom) "), "dracula-custom.json");
        assert_eq!(theme_file_name("../etc/passwd"), "etc-passwd.json");
        assert_eq!(theme_file_name("***"), "theme.json");
    }

    #[test]
    fn a_saved_theme_is_loaded_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut theme = presets().remove(2);
        theme.name = "Exported One".into();
        let path = save_user_theme(dir.path(), &theme).unwrap();
        assert_eq!(path, dir.path().join("exported-one.json"));
        assert_eq!(load_user_themes_from(dir.path()), vec![theme]);
    }

    #[test]
    fn an_invalid_theme_is_not_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut theme = presets().remove(0);
        theme.palette.clear();
        assert!(save_user_theme(dir.path(), &theme).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_short_palette_is_rejected() {
        let mut theme = presets().remove(0);
        theme.palette.pop();
        assert!(theme.validate().is_err());
    }

    #[test]
    fn user_themes_are_read_and_bad_files_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let mut good = presets().remove(1);
        good.name = "Mine".into();
        std::fs::write(dir.path().join("mine.json"), serde_json::to_string(&good).unwrap()).unwrap();
        std::fs::write(dir.path().join("broken.json"), "{ nope").unwrap();
        let mut bad = good.clone();
        bad.foreground = "red".into();
        std::fs::write(dir.path().join("bad.json"), serde_json::to_string(&bad).unwrap()).unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        let themes = load_user_themes_from(dir.path());
        assert_eq!(themes, vec![good]);
    }

    #[test]
    fn a_user_theme_replaces_the_preset_with_its_name() {
        let mut custom = presets().remove(0);
        custom.background = "#000000".into();
        let merged = merge(presets(), vec![custom.clone()]);
        assert_eq!(merged.len(), presets().len());
        assert_eq!(find(&merged, DEFAULT_THEME), &custom);
    }

    #[test]
    fn an_unknown_name_gives_the_default_theme() {
        let themes = presets();
        assert_eq!(find(&themes, "Gone").name, DEFAULT_THEME);
        assert_eq!(find(&themes, "Nord").name, "Nord");
    }
}
