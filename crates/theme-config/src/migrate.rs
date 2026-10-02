//! One-time move to the look model where the active theme is the only source
//! of the look. A `settings.json` older than `CURRENT_LOOK_VERSION` may hold
//! colours, appearance, fonts or pictures the user edited outside any theme;
//! those are saved as a new theme ("My Theme") that becomes the active one,
//! so nothing the user sees changes.

use crate::apply::{resolve_look, same_look};
use crate::builder::{save_theme, BuildRequest};
use crate::writer::BuildError;
use opusdm_ipc::look::SurfaceImage;
use opusdm_ipc::themes::{load_theme_data, theme_dir};
use opusdm_ipc::{IconSetId, Settings, CURRENT_LOOK_VERSION};

/// Display name of the theme the migration creates.
pub const MIGRATION_LABEL: &str = "My Theme";

/// Slug of the theme the migration creates, before a numeric suffix.
const MIGRATION_SLUG: &str = "my-theme";

/// What a migration produced.
#[derive(Debug)]
pub struct Migrated {
    /// The settings to persist, stamped with `CURRENT_LOOK_VERSION`.
    pub settings: Settings,
    /// Slug of the theme created for the user's own look, if one was needed.
    pub created: Option<String>,
}

/// Migrate `settings` when it is older than `CURRENT_LOOK_VERSION`; `None`
/// when it is already current. When its look differs from what the active
/// theme gives, a new theme holding that look (and the active theme's icons,
/// fonts, sounds and wallpapers) is installed and selected. The caller
/// persists the result.
pub fn migrate_look(settings: &Settings) -> Result<Option<Migrated>, BuildError> {
    if settings.look_version >= CURRENT_LOOK_VERSION {
        return Ok(None);
    }
    let IconSetId::Custom(active) = &settings.theme.icon_set;
    let resolved = resolve_look(active, &load_theme_data(active), settings);
    if same_look(settings, &resolved) {
        let mut stamped = settings.clone();
        stamped.look_version = CURRENT_LOOK_VERSION;
        return Ok(Some(Migrated { settings: stamped, created: None }));
    }

    let slug = free_migration_slug();
    let own_look = without_missing_pictures(settings);
    save_theme(&BuildRequest { label: MIGRATION_LABEL, slug: &slug, source_slug: active, settings: &own_look })?;
    let migrated = resolve_look(&slug, &load_theme_data(&slug), settings);
    Ok(Some(Migrated { settings: migrated, created: Some(slug) }))
}

/// `my-theme`, or `my-theme-2`, `my-theme-3`, ... when taken.
fn free_migration_slug() -> String {
    let mut n = 1;
    loop {
        let slug = if n == 1 { MIGRATION_SLUG.to_string() } else { format!("{MIGRATION_SLUG}-{n}") };
        if !theme_dir(&slug).exists() {
            return slug;
        }
        n += 1;
    }
}

/// `settings` with every Lister / requester picture that no longer exists
/// cleared, so a stale path cannot make the migration fail.
fn without_missing_pictures(settings: &Settings) -> Settings {
    let mut s = settings.clone();
    for surface in [&mut s.surfaces.lister, &mut s.surfaces.requester] {
        clear_if_missing(surface);
    }
    s
}

/// Drop `surface`'s picture when the file is gone.
fn clear_if_missing(surface: &mut SurfaceImage) {
    if surface.image_path.as_ref().is_some_and(|p| !p.is_file()) {
        surface.image_path = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{install_source, picture, scratch_config, ENV_LOCK};
    use opusdm_ipc::ThemeMode;

    /// Old-format settings using theme `slug`.
    fn old_settings(slug: &str) -> Settings {
        let mut s = Settings::default();
        s.theme.icon_set = IconSetId::Custom(slug.to_string());
        s
    }

    #[test]
    fn current_settings_are_left_alone() {
        let mut s = old_settings("src");
        s.look_version = CURRENT_LOOK_VERSION;
        s.theme.colors.file_fg = "#abcdef".into();
        assert!(migrate_look(&s).unwrap().is_none());
    }

    #[test]
    fn a_look_equal_to_the_active_theme_only_stamps_the_version() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("src");
        let mut s = old_settings("src");
        s.appearance.theme_mode = ThemeMode::Light;
        s.behavior.recursive_filter = true;
        let m = migrate_look(&s).unwrap().unwrap();
        assert!(m.created.is_none());
        assert_eq!(m.settings.look_version, CURRENT_LOOK_VERSION);
        assert_eq!(Settings { look_version: 0, ..m.settings }, s);
        assert!(!theme_dir(MIGRATION_SLUG).exists());
    }

    #[test]
    fn an_own_look_becomes_my_theme_carrying_the_active_theme_files() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        install_source("src");
        let mut s = old_settings("src");
        s.theme.colors.file_fg = "#abcdef".into();
        s.appearance.monospace_list = true;
        s.appearance.theme_mode = ThemeMode::Light;
        s.surfaces.lister.image_path = Some(picture(td.path(), "l.png"));
        s.surfaces.requester.image_path = Some("/no/such/r.png".into());

        let m = migrate_look(&s).unwrap().unwrap();

        assert_eq!(m.created.as_deref(), Some("my-theme"));
        assert_eq!(m.settings.theme.icon_set, IconSetId::Custom("my-theme".to_string()));
        assert_eq!(m.settings.theme.colors.file_fg, "#abcdef");
        assert!(m.settings.appearance.monospace_list);
        assert_eq!(m.settings.appearance.theme_mode, ThemeMode::Light);
        assert!(m.settings.surfaces.lister.image_path.unwrap().starts_with(theme_dir("my-theme")));
        assert_eq!(m.settings.surfaces.requester.image_path, None);
        assert_eq!(m.settings.look_version, CURRENT_LOOK_VERSION);
        assert!(theme_dir("my-theme").join("icons/dir.png").is_file());
    }

    #[test]
    fn a_taken_name_gets_a_numeric_suffix() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("src");
        install_source("my-theme");
        let mut s = old_settings("src");
        s.theme.colors.dir_bg = "#010101".into();
        assert_eq!(migrate_look(&s).unwrap().unwrap().created.as_deref(), Some("my-theme-2"));
    }
}
