//! Pure merge logic for applying a custom theme: a theme's `theme.json`
//! fields overwrite the corresponding current value only where the theme
//! actually set them; everything else keeps the user's current setting.
//! No GTK, no I/O beyond resolving a background image's absolute path -
//! fully unit-testable.

use opusdm_ipc::desktop::Background;
use opusdm_ipc::themes::CustomThemeData;
use opusdm_ipc::{Appearance, Behavior, DesktopSettings, IconSetId, Settings, Theme};

/// Everything a successful "apply a custom theme" produces, ready to send
/// as `SetTheme` + (if changed) `SetDesktopSettings`.
pub struct MergedTheme {
    pub theme: Theme,
    pub appearance: Appearance,
    pub behavior: Behavior,
    pub desktop: DesktopSettings,
}

/// Merge `slug`'s `data` over `current`/`current_desktop`. Only fields
/// `data` sets are changed; everything else is copied from `current`
/// unmodified.
pub fn merge_custom_theme(
    slug: &str,
    data: &CustomThemeData,
    current: &Settings,
    current_desktop: &DesktopSettings,
) -> MergedTheme {
    let mut colors = current.theme.colors.clone();
    if let Some(c) = &data.colors {
        colors = c.clone();
    }

    let mut background = current.theme.background.clone();
    if let Some(b) = &data.background {
        merge_background(&mut background, b, slug);
    }

    let mut appearance = current.appearance.clone();
    if let Some(a) = &data.appearance {
        appearance = a.clone();
    }

    let mut behavior = current.behavior.clone();
    if let Some(b) = &data.behavior {
        behavior = b.clone();
    }

    let mut desktop = current_desktop.clone();
    if let Some(m) = &data.desktop {
        if let Some(v) = m.icon_size {
            desktop.icon_size = v;
        }
        if let Some(v) = m.grid_spacing {
            desktop.grid_spacing = v;
        }
        if let Some(v) = m.grid_gap {
            desktop.grid_gap = v;
        }
    }

    MergedTheme {
        theme: Theme { icon_set: IconSetId::Custom(slug.to_string()), colors, background },
        appearance,
        behavior,
        desktop,
    }
}

fn merge_background(bg: &mut Background, theme_bg: &opusdm_ipc::themes::ThemeBackground, slug: &str) {
    if let Some(mode) = theme_bg.mode {
        bg.mode = mode;
    }
    if let Some(solid) = &theme_bg.solid {
        bg.solid = solid.clone();
    }
    if let Some(start) = &theme_bg.gradient_start {
        bg.gradient_start = start.clone();
    }
    if let Some(end) = &theme_bg.gradient_end {
        bg.gradient_end = end.clone();
    }
    if let Some(angle) = theme_bg.gradient_angle {
        bg.gradient_angle = angle;
    }
    if let Some(fit) = theme_bg.image_fit {
        bg.image_fit = fit;
    }
    if let Some(filename) = &theme_bg.image {
        bg.image_path = Some(opusdm_ipc::themes::background_image_path(slug, filename));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use opusdm_ipc::desktop::{BackgroundMode, ImageFit};
    use opusdm_ipc::themes::{CustomThemeData, ThemeBackground, ThemeDesktopMetrics};
    use opusdm_ipc::{Appearance, ColorSettings, DesktopSettings, IconSetId, Settings};

    fn base_settings() -> Settings {
        let mut s = Settings::default();
        s.appearance.monospace_list = true;
        s.behavior.auto_refresh = true;
        s
    }

    fn base_desktop() -> DesktopSettings {
        let mut d = DesktopSettings::default();
        d.icon_size = 64;
        d.grid_spacing = 96;
        d.grid_gap = 4;
        d
    }

    #[test]
    fn empty_theme_data_changes_nothing_but_the_icon_set() {
        let data = CustomThemeData::default();
        let settings = base_settings();
        let desktop = base_desktop();
        let merged = merge_custom_theme("mytheme", &data, &settings, &desktop);

        assert_eq!(merged.theme.icon_set, IconSetId::Custom("mytheme".to_string()));
        assert_eq!(merged.theme.colors, settings.theme.colors);
        assert_eq!(merged.theme.background, settings.theme.background);
        assert_eq!(merged.appearance, settings.appearance);
        assert_eq!(merged.behavior, settings.behavior);
        assert_eq!(merged.desktop, desktop);
    }

    #[test]
    fn theme_supplied_colors_win_others_keep_current() {
        let mut data = CustomThemeData::default();
        data.colors = Some(ColorSettings { file_fg: "#ff00ff".to_string(), ..ColorSettings::default() });
        let settings = base_settings();
        let desktop = base_desktop();
        let merged = merge_custom_theme("t", &data, &settings, &desktop);

        assert_eq!(merged.theme.colors.file_fg, "#ff00ff");
        assert_eq!(merged.theme.colors.dir_fg, ColorSettings::default().dir_fg);
        assert_eq!(merged.appearance, settings.appearance);
    }

    #[test]
    fn theme_supplied_desktop_metrics_overwrite_only_the_fields_set() {
        let mut data = CustomThemeData::default();
        data.desktop = Some(ThemeDesktopMetrics { icon_size: Some(128), grid_spacing: None, grid_gap: None });
        let desktop = base_desktop();
        let merged = merge_custom_theme("t", &data, &Settings::default(), &desktop);

        assert_eq!(merged.desktop.icon_size, 128);
        assert_eq!(merged.desktop.grid_spacing, desktop.grid_spacing);
        assert_eq!(merged.desktop.grid_gap, desktop.grid_gap);
        assert_eq!(merged.desktop.desktop_dir, desktop.desktop_dir, "themes never touch desktop_dir");
    }

    #[test]
    fn theme_supplied_appearance_and_behavior_fields_overwrite_only_what_is_set() {
        let mut data = CustomThemeData::default();
        data.appearance = Some(Appearance { type_indicators: true, monospace_list: false, theme_window: false });
        let settings = base_settings();
        let merged = merge_custom_theme("t", &data, &settings, &base_desktop());

        assert!(merged.appearance.type_indicators);
        assert_eq!(merged.behavior, settings.behavior, "behavior untouched when theme omits it");
    }

    #[test]
    fn image_background_resolves_the_filename_under_the_theme_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let mut data = CustomThemeData::default();
        data.background = Some(ThemeBackground {
            mode: Some(BackgroundMode::Image),
            image: Some("wall.jpg".to_string()),
            image_fit: Some(ImageFit::Fit),
            ..Default::default()
        });
        let merged = merge_custom_theme("t", &data, &Settings::default(), &base_desktop());

        assert_eq!(merged.theme.background.mode, BackgroundMode::Image);
        assert_eq!(merged.theme.background.image_fit, ImageFit::Fit);
        let path = merged.theme.background.image_path.unwrap();
        assert!(path.ends_with("themes/t/backgrounds/wall.jpg"));
    }

    #[test]
    fn background_solid_and_gradient_fields_overwrite_independently() {
        let mut data = CustomThemeData::default();
        data.background = Some(ThemeBackground { solid: Some("#123456".to_string()), ..Default::default() });
        let mut current = Settings::default();
        current.theme.background.gradient_start = "#abcdef".to_string();
        let merged = merge_custom_theme("t", &data, &current, &base_desktop());

        assert_eq!(merged.theme.background.solid, "#123456");
        assert_eq!(merged.theme.background.gradient_start, "#abcdef");
    }
}
