//! Discovery and parsing of user-installed themes under
//! `~/.config/opusdm/themes/<slug>/`. Pure data - no GTK, no archive
//! extraction (that lives in `opusdm-theme-config`, the only binary that
//! imports archives).

use crate::desktop::{BackgroundMode, ImageFit};
use crate::look::{Fonts, ThemeHub, ThemeSurfaces};
use crate::proto::{Appearance, ColorSettings};
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

/// Files of the always-installed default theme (`themes/breeze-dark/` in the
/// repo), embedded so the theme can be written out when it is missing.
const DEFAULT_THEME_FILES: &[(&str, &[u8])] = &[
    ("manifest.json", include_bytes!("../../../themes/breeze-dark/manifest.json")),
    ("icons/archive.png", include_bytes!("../../../themes/breeze-dark/icons/archive.png")),
    ("icons/audio.png", include_bytes!("../../../themes/breeze-dark/icons/audio.png")),
    ("icons/broken.png", include_bytes!("../../../themes/breeze-dark/icons/broken.png")),
    ("icons/code.png", include_bytes!("../../../themes/breeze-dark/icons/code.png")),
    ("icons/dir.png", include_bytes!("../../../themes/breeze-dark/icons/dir.png")),
    ("icons/exec.png", include_bytes!("../../../themes/breeze-dark/icons/exec.png")),
    ("icons/file.png", include_bytes!("../../../themes/breeze-dark/icons/file.png")),
    ("icons/image.png", include_bytes!("../../../themes/breeze-dark/icons/image.png")),
    ("icons/pdf.png", include_bytes!("../../../themes/breeze-dark/icons/pdf.png")),
    ("icons/symlink.png", include_bytes!("../../../themes/breeze-dark/icons/symlink.png")),
    ("icons/text.png", include_bytes!("../../../themes/breeze-dark/icons/text.png")),
    ("icons/video.png", include_bytes!("../../../themes/breeze-dark/icons/video.png")),
];

/// Make sure the default theme is installed: writes the embedded files
/// under `themes_dir()/<DEFAULT_THEME_SLUG>` when its `manifest.json` is
/// missing. Cheap when already installed, so callers may invoke it freely.
/// Failures are logged, not fatal (a read-only config dir just means icons
/// stay blank).
pub fn ensure_default_theme() {
    let dir = theme_dir(crate::theme::DEFAULT_THEME_SLUG);
    if dir.join("manifest.json").is_file() {
        return;
    }
    for (rel, bytes) in DEFAULT_THEME_FILES {
        let path = dir.join(rel);
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, bytes));
        if let Err(e) = result {
            crate::err_log!("opusdm", "cannot install default theme file {}: {e}", path.display());
            return;
        }
    }
}

/// Folder inside a theme that holds the wallpaper pictures it bundles.
pub const WALLPAPERS_DIR: &str = "wallpapers";

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

/// One theme as `opusdm-settings`'s picker shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomThemeSummary {
    pub slug: String,
    pub label: String,
    pub preview_path: Option<PathBuf>,
}

/// List every valid installed theme, sorted by display label. Skips (and
/// logs) any subdirectory with a missing, unparseable, or invalid
/// `manifest.json`. The default theme is installed first if
/// missing. An unreadable `themes_dir()` yields an empty list, not an error.
pub fn scan_themes() -> Vec<CustomThemeSummary> {
    ensure_default_theme();
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
            crate::err_log!("opusdm", "{} has an unparseable manifest.json", path.display());
            continue;
        };
        if !manifest.is_valid() {
            crate::err_log!("opusdm", "{} has an invalid manifest.json (name/label)", path.display());
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

/// `theme.json`'s legacy `background` section, written by themes made before
/// the wallpaper became a user preference. Only read: its `image` (a file name
/// in the theme's `backgrounds/` folder) is offered as one of the theme's
/// wallpapers by [`theme_wallpapers`]; the other fields are ignored.
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
/// only icons and nothing else. A theme owns the look only: the `behavior`
/// and `desktop` sections older themes carry are ignored when parsing, and
/// `appearance.theme_mode` is ignored when the theme is applied (light / dark
/// is a user preference).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomThemeData {
    pub colors: Option<ColorSettings>,
    pub appearance: Option<Appearance>,
    /// Legacy, read only: never written by OpusDM any more.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<ThemeBackground>,
    pub fonts: Option<Fonts>,
    pub surfaces: Option<ThemeSurfaces>,
    pub hub: Option<ThemeHub>,
}

/// Whether `name` is a bare file name: non-empty, not `.` / `..`, and with
/// no path separator. Theme files are only ever looked up inside their own
/// folders, so anything else in an imported `theme.json` is rejected.
pub fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

/// Parse `<slug>/theme.json`. A missing or unparseable file yields
/// [`CustomThemeData::default`] (every field `None`), the same tolerance
/// `Settings` already has for a hand-edited `settings.json`.
pub fn load_theme_data(slug: &str) -> CustomThemeData {
    let path = theme_dir(slug).join("theme.json");
    crate::settings::load_json(&path, "opusdm", "theme data")
}

/// The wallpaper pictures theme `slug` bundles: every picture file in its
/// `wallpapers/` folder, sorted by name, then the picture a legacy
/// `theme.json` `background.image` names in `backgrounds/` when it exists.
/// Links are skipped. An unknown theme yields an empty list.
pub fn theme_wallpapers(slug: &str) -> Vec<PathBuf> {
    if !valid_slug(slug) {
        return Vec::new();
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(theme_dir(slug).join(WALLPAPERS_DIR))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .filter(|p| crate::desktop::is_background_image(p))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    if let Some(legacy) = legacy_wallpaper(slug) {
        found.push(legacy);
    }
    found
}

/// The picture a legacy `theme.json` `background.image` names inside theme
/// `slug`'s `backgrounds/` folder, when it is a plain file name and the file
/// exists.
pub fn legacy_wallpaper(slug: &str) -> Option<PathBuf> {
    let background = load_theme_data(slug).background?;
    let name = background.image.filter(|n| is_plain_file_name(n))?;
    let path = background_image_path(slug, &name);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn plain_file_names_reject_paths_and_dots() {
        assert!(is_plain_file_name("wall.png"));
        assert!(!is_plain_file_name(""));
        assert!(!is_plain_file_name(".."));
        assert!(!is_plain_file_name("../x.png"));
        assert!(!is_plain_file_name("a/b.png"));
        assert!(!is_plain_file_name("a\\b.png"));
    }

    #[test]
    fn an_old_theme_json_without_fonts_or_surfaces_still_parses() {
        let data: CustomThemeData = serde_json::from_str(r#"{"colors":null}"#).unwrap();
        assert!(data.fonts.is_none());
        assert!(data.surfaces.is_none());
    }

    #[test]
    fn theme_json_reads_fonts_and_surfaces() {
        let data: CustomThemeData = serde_json::from_str(
            r#"{"fonts":{"list":{"family":"Iosevka","size":12}},
                "surfaces":{"requester":{"image":"r.png","fit":"center"}}}"#,
        )
        .unwrap();
        assert_eq!(data.fonts.unwrap().list.family, "Iosevka");
        assert_eq!(data.surfaces.unwrap().requester.unwrap().image.as_deref(), Some("r.png"));
    }

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
        assert_eq!(labels, vec!["Alpha Theme", "Breeze Dark", "Zeta Theme"]);
    }

    #[test]
    fn scan_themes_skips_missing_or_invalid_manifests() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(td.path(), "good", r#"{"name":"good","label":"Good"}"#, None);
        write_theme(td.path(), "no-label", r#"{"name":"no-label","label":""}"#, None);
        write_theme(td.path(), "bad-name", r#"{"name":"Bad Name","label":"x"}"#, None);
        std::fs::create_dir_all(td.path().join("opusdm").join("themes").join("no-manifest")).unwrap();
        let slugs: Vec<String> = scan_themes().into_iter().map(|t| t.slug).collect();
        assert_eq!(slugs, vec!["breeze-dark", "good"]);
    }

    #[test]
    fn scan_themes_on_missing_dir_installs_and_lists_the_default_theme() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        let found = scan_themes();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].slug, crate::theme::DEFAULT_THEME_SLUG);
        assert!(td.path().join("opusdm/themes/breeze-dark/icons/dir.png").is_file());
    }

    #[test]
    fn load_theme_data_full_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(
            td.path(),
            "full",
            r#"{"name":"full","label":"Full"}"#,
            Some(r##"{"colors":{"file_fg":"#abcdef"},"behavior":{"auto_refresh":true},"desktop":{"icon_size":80},"background":{"mode":"image","image":"bg.jpg"}}"##),
        );
        let data = load_theme_data("full");
        assert_eq!(data.colors.unwrap().file_fg, "#abcdef");
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
    fn theme_wallpapers_lists_sorted_pictures_then_the_legacy_one() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(td.path(), "walls", r#"{"name":"walls","label":"Walls"}"#, Some(r#"{"background":{"image":"desktop.jpg"}}"#));
        let dir = theme_dir("walls");
        std::fs::create_dir_all(dir.join(WALLPAPERS_DIR)).unwrap();
        std::fs::create_dir_all(dir.join("backgrounds")).unwrap();
        for name in ["b.png", "a.JPG", "notes.txt"] {
            std::fs::write(dir.join(WALLPAPERS_DIR).join(name), b"x").unwrap();
        }
        std::fs::write(dir.join("backgrounds").join("desktop.jpg"), b"x").unwrap();
        let names: Vec<String> = theme_wallpapers("walls")
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["wallpapers/a.JPG", "wallpapers/b.png", "backgrounds/desktop.jpg"]);
    }

    #[test]
    fn theme_wallpapers_ignores_a_missing_or_unsafe_legacy_picture() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(td.path(), "gone", r#"{"name":"gone","label":"Gone"}"#, Some(r#"{"background":{"image":"desktop.jpg"}}"#));
        write_theme(td.path(), "evil", r#"{"name":"evil","label":"Evil"}"#, Some(r#"{"background":{"image":"../gone/theme.json"}}"#));
        assert!(theme_wallpapers("gone").is_empty());
        assert!(theme_wallpapers("evil").is_empty());
        assert!(theme_wallpapers("Not A Slug").is_empty());
    }

    #[test]
    fn the_legacy_background_section_is_not_written_back_when_absent() {
        let json = serde_json::to_string(&CustomThemeData::default()).unwrap();
        assert!(!json.contains("background"));
    }

    #[test]
    fn background_image_path_join() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _td = with_config_home();
        let p = background_image_path("mytheme", "wall.png");
        assert!(p.ends_with("themes/mytheme/backgrounds/wall.png"));
    }
}
