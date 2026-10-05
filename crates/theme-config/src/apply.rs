//! Pure resolution of a theme into the look: a theme fully determines the
//! colours, appearance (except light / dark), fonts and Lister / requester
//! pictures, with defaults for every part it does not set. User preferences
//! (light / dark, behaviour, desktop background) are copied through. No GTK,
//! no I/O beyond resolving picture paths - fully unit-testable.

use opusdm_ipc::look::{SurfaceImage, Surfaces, ThemeSurface};
use opusdm_ipc::themes::CustomThemeData;
use opusdm_ipc::{IconSetId, Settings, Theme, CURRENT_LOOK_VERSION};

/// `prefs` with every theme-owned field replaced by what theme `slug`
/// (`data`) sets, or by the default when it sets nothing. The result is
/// stamped with [`CURRENT_LOOK_VERSION`].
pub fn resolve_look(slug: &str, data: &CustomThemeData, prefs: &Settings) -> Settings {
    let mut appearance = data.appearance.clone().unwrap_or_default();
    appearance.theme_mode = prefs.appearance.theme_mode;

    let mut surfaces = Surfaces::default();
    if let Some(s) = &data.surfaces {
        merge_surface(&mut surfaces.lister, s.lister.as_ref(), slug);
        merge_surface(&mut surfaces.requester, s.requester.as_ref(), slug);
    }

    Settings {
        theme: Theme {
            icon_set: IconSetId::Custom(slug.to_string()),
            colors: data.colors.clone().unwrap_or_default(),
            background: prefs.theme.background.clone(),
        },
        appearance,
        behavior: prefs.behavior.clone(),
        fonts: data.fonts.clone().unwrap_or_default(),
        surfaces,
        look_version: CURRENT_LOOK_VERSION,
    }
}

/// Whether `a` and `b` look the same: equal colours, fonts, surfaces and
/// appearance apart from light / dark (a preference).
pub fn same_look(a: &Settings, b: &Settings) -> bool {
    let mut b_appearance = b.appearance.clone();
    b_appearance.theme_mode = a.appearance.theme_mode;
    a.theme.colors == b.theme.colors && a.appearance == b_appearance && a.fonts == b.fonts && a.surfaces == b.surfaces
}

/// Overwrite the parts of `target` that `theme` sets. An image name that is
/// not a bare file name is ignored, so a theme can never point outside its
/// own `backgrounds/` folder.
pub(crate) fn merge_surface(target: &mut SurfaceImage, theme: Option<&ThemeSurface>, slug: &str) {
    let Some(theme) = theme else { return };
    if let Some(fit) = theme.fit {
        target.fit = fit;
    }
    let Some(name) = theme.image.as_deref().filter(|n| opusdm_ipc::themes::is_plain_file_name(n)) else {
        return;
    };
    target.image_path = Some(opusdm_ipc::themes::background_image_path(slug, name));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use opusdm_ipc::desktop::ImageFit;
    use opusdm_ipc::look::{FontSpec, Fonts, ThemeSurfaces};
    use opusdm_ipc::{Appearance, ColorSettings, ThemeMode};

    /// Preferences with every field away from its default.
    fn prefs() -> Settings {
        let mut s = Settings::default();
        s.theme.colors.file_fg = "#010203".into();
        s.theme.background.entries = vec![opusdm_ipc::BackgroundEntry::default_gradient()];
        s.appearance = Appearance { type_indicators: true, monospace_list: true, theme_window: true, theme_mode: ThemeMode::Light };
        s.behavior.auto_refresh = !s.behavior.auto_refresh;
        s.fonts.ui.size = 9;
        s.surfaces.requester.fit = ImageFit::Fit;
        s
    }

    #[test]
    fn a_theme_that_sets_nothing_gives_the_default_look_and_keeps_the_preferences() {
        let p = prefs();
        let s = resolve_look("t", &CustomThemeData::default(), &p);
        assert_eq!(s.theme.icon_set, IconSetId::Custom("t".to_string()));
        assert_eq!(s.theme.colors, ColorSettings::default());
        assert_eq!(s.appearance, Appearance { theme_mode: ThemeMode::Light, ..Appearance::default() });
        assert_eq!(s.fonts, Fonts::default());
        assert_eq!(s.surfaces, Surfaces::default());
        assert_eq!(s.theme.background, p.theme.background);
        assert_eq!(s.behavior, p.behavior);
        assert_eq!(s.look_version, CURRENT_LOOK_VERSION);
    }

    #[test]
    fn theme_values_replace_the_look_but_never_light_or_dark() {
        let mut data = CustomThemeData::default();
        data.colors = Some(ColorSettings { file_fg: "#ff00ff".to_string(), ..ColorSettings::default() });
        data.appearance = Some(Appearance { monospace_list: true, theme_mode: ThemeMode::Dark, ..Appearance::default() });
        data.fonts = Some(Fonts { list: FontSpec { family: "Iosevka".into(), size: 12 }, ui: FontSpec::default() });
        let s = resolve_look("t", &data, &prefs());
        assert_eq!(s.theme.colors.file_fg, "#ff00ff");
        assert!(s.appearance.monospace_list && !s.appearance.type_indicators);
        assert_eq!(s.appearance.theme_mode, ThemeMode::Light);
        assert_eq!(s.fonts.list.family, "Iosevka");
    }

    #[test]
    fn surface_image_resolves_under_the_theme_backgrounds_folder() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let mut data = CustomThemeData::default();
        data.surfaces = Some(ThemeSurfaces {
            lister: Some(ThemeSurface { image: Some("l.png".into()), fit: Some(ImageFit::Tile) }),
            requester: None,
        });
        let s = resolve_look("t", &data, &prefs());
        assert!(s.surfaces.lister.image_path.unwrap().ends_with("themes/t/backgrounds/l.png"));
        assert_eq!(s.surfaces.lister.fit, ImageFit::Tile);
        assert_eq!(s.surfaces.requester, SurfaceImage::default(), "a surface the theme omits is the default");
    }

    #[test]
    fn image_names_that_are_paths_are_ignored() {
        let mut data = CustomThemeData::default();
        data.surfaces = Some(ThemeSurfaces {
            lister: Some(ThemeSurface { image: Some("../../x.png".into()), fit: None }),
            requester: None,
        });
        let s = resolve_look("t", &data, &Settings::default());
        assert_eq!(s.surfaces.lister.image_path, None);
    }

    #[test]
    fn same_look_ignores_preferences_but_not_look_values() {
        let a = prefs();
        let mut b = a.clone();
        b.appearance.theme_mode = ThemeMode::Dark;
        b.behavior.cache_size += 1;
        b.theme.background.entries = vec![opusdm_ipc::BackgroundEntry::Solid { color: "#000001".into() }];
        b.look_version = 99;
        assert!(same_look(&a, &b));
        b.theme.colors.dir_bg = "#000002".into();
        assert!(!same_look(&a, &b));
        let mut c = a.clone();
        c.appearance.monospace_list = false;
        assert!(!same_look(&a, &c));
    }
}
