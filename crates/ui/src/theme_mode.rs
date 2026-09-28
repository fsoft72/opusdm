//! GTK-wide dark/light theme preference.

use gtk4 as gtk;
use opusdm_ipc::ThemeMode;

/// Ask GTK for the dark or light variant of the active theme
/// (`gtk-application-prefer-dark-theme`), so every widget - buttons,
/// entries, dialog chrome included - is themed consistently by the real
/// theme engine, rather than only the specific classes OpusDM hand-paints
/// in its own CSS (file list rows, status bar, and - when enabled - the
/// window background).
pub fn apply_theme_mode(mode: ThemeMode) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(mode == ThemeMode::Dark);
    }
}

/// Fixed background/foreground palette for [`chrome_css`]. Not
/// user-configurable (unlike `opusdm_ipc::ColorSettings`, which colours the
/// Lister's own file-list rows and status bar) - this only has to look
/// right against itself, in every window, on every system, regardless of
/// which (if any) system GTK theme is installed.
struct Palette {
    window_bg: &'static str,
    surface_bg: &'static str,
    surface_hover: &'static str,
    surface_border: &'static str,
    text: &'static str,
}

const DARK: Palette = Palette {
    window_bg: "#1e1f22",
    surface_bg: "#2b2d31",
    surface_hover: "#35373c",
    surface_border: "#3f4147",
    text: "#e3e3e3",
};

const LIGHT: Palette = Palette {
    window_bg: "#f2f2f2",
    surface_bg: "#ffffff",
    surface_hover: "#ececec",
    surface_border: "#d0d0d0",
    text: "#1a1a1a",
};

/// CSS for the generic widgets OpusDM never hand-paints itself (window
/// background, entries, drop-downs, buttons, list-box rows, popovers) -
/// applied on top of (and instead of relying on) whatever GTK theme happens
/// to be installed, so every OpusDM window looks the same dark/light way
/// everywhere. Element selectors only (no OpusDM-specific classes), so the
/// more specific `.row-dir`/`.row-file`/`.stat-source`/`.stat-dest` rules in
/// `generate_css` still win where they apply.
pub fn chrome_css(mode: ThemeMode) -> String {
    let p = match mode {
        ThemeMode::Dark => &DARK,
        ThemeMode::Light => &LIGHT,
    };
    format!(
        "window {{ background-color: {wbg}; color: {fg}; }} \
         entry, dropdown, dropdown > button, spinbutton, button {{ \
             background-color: {sbg}; color: {fg}; \
             border: 1px solid {border}; border-radius: 4px; \
         }} \
         button:hover, dropdown:hover, dropdown > button:hover {{ background-color: {hover}; }} \
         list-box, list-box row {{ background-color: {sbg}; color: {fg}; }} \
         popover, popover.background, popover contents {{ background-color: {sbg}; color: {fg}; }} \
         scrolledwindow, viewport {{ background-color: {wbg}; }} ",
        wbg = p.window_bg,
        sbg = p.surface_bg,
        hover = p.surface_hover,
        border = p.surface_border,
        fg = p.text,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_and_light_chrome_css_use_different_window_backgrounds() {
        let dark = chrome_css(ThemeMode::Dark);
        let light = chrome_css(ThemeMode::Light);
        assert!(dark.contains(DARK.window_bg));
        assert!(!dark.contains(LIGHT.window_bg));
        assert!(light.contains(LIGHT.window_bg));
        assert!(!light.contains(DARK.window_bg));
    }

    #[test]
    fn chrome_css_covers_the_generic_widgets_opusdm_does_not_hand_paint() {
        let css = chrome_css(ThemeMode::Dark);
        for selector in ["window {", "entry,", "button", "list-box", "popover", "dropdown"] {
            assert!(css.contains(selector), "missing rule for {selector}");
        }
    }

    #[test]
    fn surface_background_differs_from_window_background_in_both_modes() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = match mode {
                ThemeMode::Dark => &DARK,
                ThemeMode::Light => &LIGHT,
            };
            assert_ne!(p.window_bg, p.surface_bg, "{mode:?} must contrast window vs. component background");
        }
    }
}
