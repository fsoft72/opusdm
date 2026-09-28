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
        "window {{ background-color: {wbg}; background-image: none; color: {fg}; }} \
         entry, dropdown, dropdown > button, spinbutton, button {{ \
             background-color: {sbg}; background-image: none; color: {fg}; \
             border: 1px solid {border}; border-radius: 4px; \
             box-shadow: none; \
         }} \
         button:hover, dropdown:hover, dropdown > button:hover {{ background-color: {hover}; }} \
         list-box, list-box row {{ background-color: {sbg}; background-image: none; color: {fg}; }} \
         popover, popover.background, popover contents {{ background-color: {sbg}; background-image: none; color: {fg}; }} \
         scrolledwindow, viewport {{ background-color: {wbg}; background-image: none; }} \
         box, grid, stack, frame {{ background-color: {wbg}; background-image: none; color: {fg}; }} \
         button > *, dropdown > *, headerbar > * {{ background-color: transparent; }} \
         columnview box, columnview grid, columnview stack, columnview frame, \
         list-box box, list-box grid, list-box stack, list-box frame, \
         popover box, popover grid, popover stack, popover frame {{ background-color: transparent; }} \
         label {{ color: {fg}; }} \
         checkbutton, radiobutton {{ color: {fg}; }} \
         headerbar {{ background-color: {wbg}; background-image: none; color: {fg}; }} \
         notebook > header {{ background-color: {wbg}; background-image: none; }} \
         notebook > header tab {{ background-color: {sbg}; background-image: none; color: {fg}; border: 1px solid {border}; }} \
         notebook > header tab:checked {{ background-color: {wbg}; background-image: none; }} ",
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
        for selector in [
            "window {", "entry,", "button", "list-box", "popover", "dropdown", "label", "notebook",
        ] {
            assert!(css.contains(selector), "missing rule for {selector}");
        }
    }

    #[test]
    fn interactive_controls_clear_the_theme_s_own_background_image() {
        // A `background-color` we set can still be entirely hidden by a
        // `background-image` the active theme sets on the same node (e.g.
        // a gradient or texture Breeze's GTK4 button style paints) -
        // background-image is a separate property drawn on top, so
        // winning the background-color property alone is not enough.
        let css = chrome_css(ThemeMode::Dark);
        assert!(css.contains("background-image: none"));
    }

    #[test]
    fn interactive_controls_reset_their_own_direct_children_back_to_transparent() {
        // The broad box/grid/stack/frame rule above would otherwise paint
        // the inner box a gtk::Button/DropDown builds for its icon+label
        // layout with the *window* colour, covering the control's own
        // *surface* colour underneath with a visible mismatched patch.
        let css = chrome_css(ThemeMode::Dark);
        assert!(css.contains("button > *") || css.contains("button>*"));
        assert!(css.contains("transparent"));
    }

    #[test]
    fn columnview_rows_are_not_covered_by_the_generic_container_paint() {
        // Same failure shape as the button/dropdown case above, one level
        // deeper: a ColumnView cell's icon+label layout is itself a box, so
        // the generic box/grid/stack/frame rule painted every Lister row
        // window-coloured, hiding `.row-dir`/`.row-file`/`.cell-selected`
        // (OpusDM's own, more specific per-row colouring) underneath it.
        let css = chrome_css(ThemeMode::Dark);
        for selector in ["columnview box", "list-box box", "popover box"] {
            assert!(css.contains(selector), "missing reset for {selector}");
        }
    }

    #[test]
    fn chrome_css_paints_generic_containers_directly_since_background_color_does_not_inherit() {
        // Regression: `background-color` is not an inherited CSS property
        // (unlike `color`), so `window { background-color }` alone only
        // paints the window's own node - every plain gtk::Box/Grid/Stack
        // a notebook page is built from keeps whatever opaque background
        // the active GTK theme gives it, showing through on top of the
        // window colour underneath. Structural containers must be painted
        // with the window colour explicitly, the same way interactive
        // controls are painted with the surface colour.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let css = chrome_css(mode);
            for selector in ["box", "grid", "stack", "frame"] {
                assert!(css.contains(selector), "{mode:?}: missing container rule for {selector}");
            }
        }
    }

    #[test]
    fn label_and_notebook_text_colour_is_set_explicitly_not_left_to_inheritance() {
        // Regression: `window { color }` alone is *inherited* by descendant
        // labels, but GTK's own theme rules directly match `label`/
        // `notebook` nodes - an explicit match always wins over an
        // inherited value regardless of provider priority, so relying on
        // inheritance left label text unreadable (light-on-light or
        // dark-on-dark) whenever the active GTK theme's own text colour
        // disagreed with ours (e.g. a system that forces prefer-dark via
        // XSettings regardless of what `apply_theme_mode` asks for).
        for (mode, text) in [(ThemeMode::Dark, DARK.text), (ThemeMode::Light, LIGHT.text)] {
            let css = chrome_css(mode);
            assert!(css.contains(&format!("label {{ color: {text}")), "{mode:?} label color");
            assert!(css.contains(&format!("notebook")), "{mode:?} notebook rule");
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
