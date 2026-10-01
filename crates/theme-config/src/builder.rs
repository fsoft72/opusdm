//! "Save theme": capture the current settings as an installed theme. A thin
//! mapping from `Settings` to a [`ThemeDraft`]; the folder is written by
//! `writer::write_theme`.

use crate::writer::{write_theme, BuildError, ThemeDraft};
use opusdm_ipc::themes::{valid_slug, ThemeDesktopMetrics};
use opusdm_ipc::{DesktopSettings, Settings};
use std::path::PathBuf;

/// Everything Save needs.
pub struct BuildRequest<'a> {
    /// Display name.
    pub label: &'a str,
    /// Folder name and `manifest.json` `name`; must pass `valid_slug`.
    pub slug: &'a str,
    /// Installed theme whose `icons/`, `fonts/` and `sounds/` are carried along.
    pub source_slug: &'a str,
    pub settings: &'a Settings,
    pub desktop: &'a DesktopSettings,
}

/// Turn a display name into a theme slug: lower-case ASCII letters and
/// digits, runs of anything else collapsed to a single `-`. `None` when
/// nothing usable is left.
pub fn slugify(label: &str) -> Option<String> {
    let mut slug = String::new();
    for c in label.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    valid_slug(&slug).then_some(slug)
}

/// Install the current settings as theme `req.slug`, replacing any theme with
/// that slug. Returns the installed folder.
pub fn save_theme(req: &BuildRequest) -> Result<PathBuf, BuildError> {
    let s = req.settings;
    let draft = ThemeDraft {
        label: req.label.trim().to_string(),
        author: None,
        version: None,
        colors: Some(s.theme.colors.clone()),
        appearance: Some(s.appearance.clone()),
        behavior: Some(s.behavior.clone()),
        desktop: Some(ThemeDesktopMetrics {
            icon_size: Some(req.desktop.icon_size),
            grid_spacing: Some(req.desktop.grid_spacing),
            grid_gap: Some(req.desktop.grid_gap),
        }),
        background: Some(s.theme.background.clone()),
        fonts: Some(s.fonts.clone()),
        lister: Some(s.surfaces.lister.clone()),
        requester: Some(s.surfaces.requester.clone()),
    };
    write_theme(req.slug, &draft, Some(req.source_slug), &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{install_source, picture, scratch_config, ENV_LOCK};
    use opusdm_ipc::desktop::{BackgroundMode, ImageFit};
    use opusdm_ipc::look::{FontSpec, SurfaceImage};
    use opusdm_ipc::themes::{load_theme_data, theme_dir, themes_dir};

    #[test]
    fn slugify_lowercases_and_collapses_separators() {
        assert_eq!(slugify("  My Cool Theme! ").as_deref(), Some("my-cool-theme"));
        assert_eq!(slugify("a__b").as_deref(), Some("a-b"));
        assert_eq!(slugify("***"), None);
        assert_eq!(slugify(""), None);
    }

    #[test]
    fn saves_the_current_settings_as_a_complete_standalone_theme() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        install_source("src");
        let mut settings = Settings::default();
        settings.theme.colors.file_fg = "#abcdef".into();
        settings.theme.background.mode = BackgroundMode::Image;
        settings.theme.background.image_path = Some(picture(td.path(), "wall.jpg"));
        settings.theme.background.image_fit = ImageFit::Tile;
        settings.surfaces.lister.image_path = Some(picture(td.path(), "l.png"));
        settings.fonts.list = FontSpec { family: "Iosevka".into(), size: 12 };

        let dir = save_theme(&BuildRequest { label: "My Theme", slug: "my-theme", source_slug: "src", settings: &settings, desktop: &DesktopSettings::default() }).unwrap();

        assert_eq!(dir, theme_dir("my-theme"));
        let data = load_theme_data("my-theme");
        assert_eq!(data.colors.unwrap().file_fg, "#abcdef");
        assert_eq!(data.background.unwrap().image.as_deref(), Some("desktop.jpg"));
        assert_eq!(data.fonts.unwrap().list.family, "Iosevka");
        assert!(data.desktop.is_some() && data.appearance.is_some() && data.behavior.is_some());
        for rel in ["icons/dir.png", "fonts/f.ttf", "sounds/a.wav", "backgrounds/desktop.jpg", "backgrounds/lister.png"] {
            assert!(dir.join(rel).is_file(), "{rel}");
        }
    }

    #[test]
    fn an_invalid_slug_is_rejected() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        let r = save_theme(&BuildRequest { label: "T", slug: "Bad Slug", source_slug: "src", settings: &Settings::default(), desktop: &DesktopSettings::default() });
        assert!(matches!(r, Err(BuildError::InvalidName(_))));
    }

    /// Acceptance: a saved, exported, wiped and re-imported theme restores colours and pictures.
    #[test]
    fn save_export_wipe_import_apply_restores_colours_and_pictures() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        opusdm_ipc::themes::ensure_default_theme();
        let mut settings = Settings::default();
        settings.theme.colors.file_bg = "#102030".into();
        settings.theme.background.mode = BackgroundMode::Image;
        settings.theme.background.image_path = Some(picture(td.path(), "wall.png"));
        settings.surfaces.requester = SurfaceImage { image_path: Some(picture(td.path(), "req.png")), fit: ImageFit::Fit };
        settings.fonts.ui = FontSpec { family: "Cantarell".into(), size: 10 };
        save_theme(&BuildRequest { label: "Round Trip", slug: "round-trip", source_slug: "breeze-dark", settings: &settings, desktop: &DesktopSettings::default() }).unwrap();
        let zip = td.path().join("round-trip.zip");
        crate::exporter::export_theme("round-trip", &zip).unwrap();

        std::fs::remove_dir_all(themes_dir()).unwrap();
        let slug = crate::importer::import_theme_archive(&zip).unwrap();
        let merged = crate::apply::merge_custom_theme(&slug, &load_theme_data(&slug), &Settings::default(), &DesktopSettings::default());

        assert_eq!(merged.theme.colors.file_bg, "#102030");
        assert_eq!(merged.theme.background.mode, BackgroundMode::Image);
        assert_eq!(std::fs::read(merged.theme.background.image_path.unwrap()).unwrap(), b"wall.png");
        assert_eq!(std::fs::read(merged.surfaces.requester.image_path.unwrap()).unwrap(), b"req.png");
        assert_eq!(merged.surfaces.requester.fit, ImageFit::Fit);
        assert_eq!(merged.fonts.ui.family, "Cantarell");
        assert!(theme_dir("round-trip").join("icons/dir.png").is_file(), "icons travel with the theme");
    }
}
