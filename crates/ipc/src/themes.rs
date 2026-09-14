//! Discovery and parsing of user-installed themes under
//! `~/.config/opusdm/themes/<slug>/`. Pure data - no GTK, no archive
//! extraction (that lives in `opusdm-theme-config`, the only binary that
//! imports archives).

use crate::desktop::{BackgroundMode, ImageFit};
use crate::proto::{Appearance, Behavior, ColorSettings};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `~/.config/opusdm/themes`. Same `config_dir()` resolution used
/// throughout the app.
pub fn themes_dir() -> PathBuf {
    crate::settings::config_dir().join("themes")
}

/// The directory for one installed theme.
pub fn theme_dir(slug: &str) -> PathBuf {
    themes_dir().join(slug)
}

/// A background image's path inside a theme's `backgrounds/` folder.
pub fn background_image_path(slug: &str, filename: &str) -> PathBuf {
    theme_dir(slug).join("backgrounds").join(filename)
}

/// A valid theme slug / `manifest.json` `name`: non-empty, lowercase
/// ASCII letters, digits and `-` only.
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `manifest.json`'s shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeManifest {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

impl ThemeManifest {
    fn is_valid(&self) -> bool {
        valid_slug(&self.name) && !self.label.trim().is_empty()
    }
}

/// One theme as `opusdm-theme`'s picker shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomThemeSummary {
    pub slug: String,
    pub label: String,
    pub preview_path: Option<PathBuf>,
}

/// List every valid installed theme, sorted by display label. Skips (and
/// logs) any subdirectory with a missing, unparseable, or invalid
/// `manifest.json`. An unreadable or absent `themes_dir()` yields an empty
/// list, not an error.
pub fn scan_themes() -> Vec<CustomThemeSummary> {
    let dir = themes_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        let Ok(content) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<ThemeManifest>(&content) else {
            eprintln!("opusdm: {} has an unparseable manifest.json", path.display());
            continue;
        };
        if !manifest.is_valid() {
            eprintln!("opusdm: {} has an invalid manifest.json (name/label)", path.display());
            continue;
        }
        let preview = path.join("preview.png");
        found.push(CustomThemeSummary {
            slug: manifest.name,
            label: manifest.label,
            preview_path: preview.is_file().then_some(preview),
        });
    }
    found.sort_by(|a, b| a.label.cmp(&b.label));
    found
}

/// `theme.json`'s `desktop` section: a subset of `DesktopSettings`,
/// deliberately excluding `desktop_dir` (an environment fact, not a look).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeDesktopMetrics {
    pub icon_size: Option<i32>,
    pub grid_spacing: Option<i32>,
    pub grid_gap: Option<i32>,
}

/// `theme.json`'s `background` section: mirrors
/// [`crate::desktop::Background`] but `image` is a filename relative to the
/// theme's `backgrounds/` folder, resolved to an absolute path only when
/// applying the theme.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeBackground {
    pub mode: Option<BackgroundMode>,
    pub solid: Option<String>,
    pub gradient_start: Option<String>,
    pub gradient_end: Option<String>,
    pub gradient_angle: Option<u16>,
    pub image: Option<String>,
    pub image_fit: Option<ImageFit>,
}

/// `theme.json`'s full shape. Every field optional - a theme can supply
/// only icons and nothing else.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomThemeData {
    pub colors: Option<ColorSettings>,
    pub appearance: Option<Appearance>,
    pub behavior: Option<Behavior>,
    pub desktop: Option<ThemeDesktopMetrics>,
    pub background: Option<ThemeBackground>,
}

/// Parse `<slug>/theme.json`. A missing or unparseable file yields
/// [`CustomThemeData::default`] (every field `None`), the same tolerance
/// `Settings` already has for a hand-edited `settings.json`.
pub fn load_theme_data(slug: &str) -> CustomThemeData {
    let path = theme_dir(slug).join("theme.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_config_home() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        td
    }

    fn write_theme(root: &std::path::Path, slug: &str, manifest: &str, theme_json: Option<&str>) {
        let dir = root.join("opusdm").join("themes").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
        if let Some(t) = theme_json {
            std::fs::write(dir.join("theme.json"), t).unwrap();
        }
    }

    #[test]
    fn valid_slug_rules() {
        assert!(valid_slug("breeze-dark"));
        assert!(valid_slug("a1"));
        assert!(!valid_slug(""));
        assert!(!valid_slug("Breeze-Dark"));
        assert!(!valid_slug("has/slash"));
        assert!(!valid_slug("has.dot"));
        assert!(!valid_slug("has space"));
    }

    #[test]
    fn scan_themes_finds_valid_dirs_and_sorts_by_label() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(td.path(), "zeta", r#"{"name":"zeta","label":"Zeta Theme"}"#, None);
        write_theme(td.path(), "alpha", r#"{"name":"alpha","label":"Alpha Theme"}"#, None);
        let found = scan_themes();
        let labels: Vec<&str> = found.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, vec!["Alpha Theme", "Zeta Theme"]);
    }

    #[test]
    fn scan_themes_skips_missing_or_invalid_manifests() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(td.path(), "good", r#"{"name":"good","label":"Good"}"#, None);
        write_theme(td.path(), "no-label", r#"{"name":"no-label","label":""}"#, None);
        write_theme(td.path(), "bad-name", r#"{"name":"Bad Name","label":"x"}"#, None);
        std::fs::create_dir_all(td.path().join("opusdm").join("themes").join("no-manifest")).unwrap();
        let found = scan_themes();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].slug, "good");
    }

    #[test]
    fn scan_themes_on_missing_dir_is_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _td = with_config_home();
        assert!(scan_themes().is_empty());
    }

    #[test]
    fn load_theme_data_full_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(
            td.path(),
            "full",
            r#"{"name":"full","label":"Full"}"#,
            Some(r##"{"colors":{"file_fg":"#abcdef"},"desktop":{"icon_size":80},"background":{"mode":"image","image":"bg.jpg"}}"##),
        );
        let data = load_theme_data("full");
        assert_eq!(data.colors.unwrap().file_fg, "#abcdef");
        assert_eq!(data.desktop.unwrap().icon_size, Some(80));
        assert_eq!(data.background.unwrap().image.as_deref(), Some("bg.jpg"));
    }

    #[test]
    fn load_theme_data_missing_file_is_all_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(td.path(), "icons-only", r#"{"name":"icons-only","label":"Icons Only"}"#, None);
        let data = load_theme_data("icons-only");
        assert!(data.colors.is_none());
        assert!(data.appearance.is_none());
        assert!(data.behavior.is_none());
        assert!(data.desktop.is_none());
        assert!(data.background.is_none());
    }

    #[test]
    fn the_shipped_breeze_dark_package_has_every_icon_and_a_valid_manifest() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let theme_dir = repo_root.join("themes").join("breeze-dark");
        let manifest: ThemeManifest =
            serde_json::from_str(&std::fs::read_to_string(theme_dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.name, "breeze-dark");
        assert!(manifest.is_valid());

        const EXPECTED: [&str; 12] = [
            "dir.png", "file.png", "symlink.png", "broken.png", "exec.png", "archive.png",
            "image.png", "audio.png", "video.png", "code.png", "text.png", "pdf.png",
        ];
        for name in EXPECTED {
            assert!(theme_dir.join("icons").join(name).is_file(), "missing icons/{name}");
        }
    }

    #[test]
    fn background_image_path_join() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _td = with_config_home();
        let p = background_image_path("mytheme", "wall.png");
        assert!(p.ends_with("themes/mytheme/backgrounds/wall.png"));
    }
}
