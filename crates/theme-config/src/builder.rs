//! Capture the look of the current settings as an installed theme (used by
//! the one-time look migration). A thin mapping from `Settings` to a
//! [`ThemeDraft`]; the folder is written by `writer::write_theme`.

use crate::writer::{write_theme, BuildError, ThemeDraft};
use opusdm_ipc::themes::{theme_dir, valid_slug, ThemeManifest};
use opusdm_ipc::Settings;
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

/// The `author` and `version` of the installed theme `slug`, or `None`s when
/// it is not installed or its manifest cannot be read.
fn existing_credits(slug: &str) -> (Option<String>, Option<String>) {
    if !valid_slug(slug) {
        return (None, None);
    }
    let Ok(text) = std::fs::read_to_string(theme_dir(slug).join("manifest.json")) else { return (None, None) };
    let Ok(manifest) = serde_json::from_str::<ThemeManifest>(&text) else { return (None, None) };
    (manifest.author, manifest.version)
}

/// Install the look of the current settings (colours, appearance, fonts,
/// Lister and requester pictures) as theme `req.slug`, replacing any theme
/// with that slug. Returns the installed folder.
pub fn save_theme(req: &BuildRequest) -> Result<PathBuf, BuildError> {
    let s = req.settings;
    let (author, version) = existing_credits(req.slug);
    let draft = ThemeDraft {
        label: req.label.trim().to_string(),
        author,
        version,
        colors: Some(s.theme.colors.clone()),
        appearance: Some(s.appearance.clone()),
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
    use opusdm_ipc::desktop::{BackgroundEntry, ImageFit};
    use opusdm_ipc::look::{FontSpec, SurfaceImage};
    use opusdm_ipc::themes::{load_theme_data, themes_dir};

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
        settings.theme.background.entries =
            vec![BackgroundEntry::Image { path: picture(td.path(), "wall.jpg"), fit: ImageFit::Fill }];
        settings.surfaces.lister.image_path = Some(picture(td.path(), "l.png"));
        settings.fonts.list = FontSpec { family: "Iosevka".into(), size: 12 };

        let dir = save_theme(&BuildRequest { label: "My Theme", slug: "my-theme", source_slug: "src", settings: &settings }).unwrap();

        assert_eq!(dir, theme_dir("my-theme"));
        let data = load_theme_data("my-theme");
        assert_eq!(data.colors.unwrap().file_fg, "#abcdef");
        assert!(data.background.is_none(), "the wallpaper is a preference, not part of the theme");
        assert_eq!(data.fonts.unwrap().list.family, "Iosevka");
        assert!(data.appearance.is_some());
        for rel in ["icons/dir.png", "fonts/f.ttf", "sounds/a.wav", "backgrounds/lister.png"] {
            assert!(dir.join(rel).is_file(), "{rel}");
        }
        assert!(!dir.join("backgrounds/desktop.jpg").exists());
    }

    #[test]
    fn an_invalid_slug_is_rejected() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        let r = save_theme(&BuildRequest { label: "T", slug: "Bad Slug", source_slug: "src", settings: &Settings::default() });
        assert!(matches!(r, Err(BuildError::InvalidName(_))));
    }

    /// Saving over an installed theme keeps the author and version it had.
    #[test]
    fn saving_over_an_existing_theme_keeps_its_author_and_version() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("src");
        std::fs::write(theme_dir("src").join("manifest.json"), br#"{"name":"src","label":"Src","author":"me","version":"3"}"#).unwrap();
        let req = BuildRequest { label: "Src", slug: "src", source_slug: "src", settings: &Settings::default() };
        save_theme(&req).unwrap();
        let m: opusdm_ipc::themes::ThemeManifest = serde_json::from_str(&std::fs::read_to_string(theme_dir("src").join("manifest.json")).unwrap()).unwrap();
        assert_eq!((m.author.as_deref(), m.version.as_deref()), (Some("me"), Some("3")));
    }

    /// Acceptance: a saved, exported, wiped and re-imported theme restores its colours and pictures.
    #[test]
    fn save_export_wipe_import_apply_restores_colours_and_pictures() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        opusdm_ipc::themes::ensure_default_theme();
        let mut settings = Settings::default();
        settings.theme.colors.file_bg = "#102030".into();
        settings.surfaces.requester = SurfaceImage { image_path: Some(picture(td.path(), "req.png")), fit: ImageFit::Fit };
        settings.fonts.ui = FontSpec { family: "Cantarell".into(), size: 10 };
        save_theme(&BuildRequest { label: "Round Trip", slug: "round-trip", source_slug: "breeze-dark", settings: &settings }).unwrap();
        let zip = td.path().join("round-trip.zip");
        crate::exporter::export_theme("round-trip", &zip).unwrap();

        std::fs::remove_dir_all(themes_dir()).unwrap();
        let slug = crate::importer::import_theme_archive(&zip).unwrap();
        let merged = crate::apply::resolve_look(&slug, &load_theme_data(&slug), &Settings::default());

        assert_eq!(merged.theme.colors.file_bg, "#102030");
        assert_eq!(std::fs::read(merged.surfaces.requester.image_path.unwrap()).unwrap(), b"req.png");
        assert_eq!(merged.surfaces.requester.fit, ImageFit::Fit);
        assert_eq!(merged.fonts.ui.family, "Cantarell");
        assert!(theme_dir("round-trip").join("icons/dir.png").is_file(), "icons travel with the theme");
    }
}
