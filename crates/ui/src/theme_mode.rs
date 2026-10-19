//! GTK-wide dark/light theme preference.

use gtk4 as gtk;
use opusdm_ipc::{Settings, ThemeMode};

/// Force GTK's own built-in "Adwaita" theme (compiled into libgtk4 itself,
/// always available, no external theme files needed) instead of whatever
/// theme the desktop session happens to push via XSettings (Breeze, on a
/// KDE session - its GTK4 port draws buttons with a background-image
/// gradient `chrome_css` then has to fight with `background-image: none`,
/// and looked meaningfully different again inside a nested Xephyr sandbox
/// with no XSettings service at all). A known, simple, always-present base
/// theme is what `chrome_css` is designed to sit on top of - every OpusDM
/// window ends up looking identical regardless of the desktop it runs
/// under, rather than depending on that desktop's own theme's assumptions.
pub fn apply_theme_mode(mode: ThemeMode) {
    let Some(settings) = gtk::Settings::default() else { return };
    // GTK reloads the style of every window on a property write, even for
    // an unchanged value: write only what differs, or the desktop flickers.
    if settings.gtk_theme_name().as_deref() != Some("Adwaita") {
        settings.set_gtk_theme_name(Some("Adwaita"));
    }
    let dark = mode == ThemeMode::Dark;
    if settings.is_gtk_application_prefer_dark_theme() != dark {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
    set_interface_color_scheme(&settings, dark);
}

/// GtkSettings property GTK 4.20+ reads to pick the light or dark variant of
/// the theme; it ignores the deprecated `gtk-application-prefer-dark-theme`
/// (GTK 4.24 drew light buttons in the panel with only that one set).
const COLOR_SCHEME_PROPERTY: &str = "gtk-interface-color-scheme";

/// Enum nick of `GtkInterfaceColorScheme` for the wanted variant.
fn color_scheme_nick(dark: bool) -> &'static str {
    if dark { "dark" } else { "light" }
}

/// Set `gtk-interface-color-scheme` on GTK 4.20+ (a no-op on older GTK,
/// which lacks the property). Written only when it differs, like the other
/// settings in [`apply_theme_mode`].
fn set_interface_color_scheme(settings: &gtk::Settings, dark: bool) {
    use gtk::glib;
    use gtk::prelude::ObjectExt;
    let Some(pspec) = settings.find_property(COLOR_SCHEME_PROPERTY) else { return };
    let nick = color_scheme_nick(dark);
    let Some(class) = glib::EnumClass::with_type(pspec.value_type()) else {
        opusdm_ipc::err_log!("ui", "{COLOR_SCHEME_PROPERTY} is not an enum ({}), dark/light not applied", pspec.value_type());
        return;
    };
    let Some(value) = class.to_value_by_nick(nick) else {
        opusdm_ipc::err_log!("ui", "{COLOR_SCHEME_PROPERTY} has no value '{nick}', dark/light not applied");
        return;
    };
    let current = settings.property_value(COLOR_SCHEME_PROPERTY);
    if glib::EnumValue::from_value(&current).map(|(_, v)| v.nick()) == Some(nick) {
        return;
    }
    settings.set_property_from_value(COLOR_SCHEME_PROPERTY, &value);
}

#[cfg(test)]
mod color_scheme_tests {
    use super::*;

    #[test]
    fn color_scheme_nick_follows_the_theme_mode() {
        assert_eq!(color_scheme_nick(true), "dark");
        assert_eq!(color_scheme_nick(false), "light");
    }
}

/// Name of the accent colour [`chrome_css`] defines, for program CSS that
/// follows it: `outline: 2px solid @opusdm_accent;`.
pub const ACCENT_COLOR_NAME: &str = "opusdm_accent";

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
    accent: &'static str,
}

const DARK: Palette = Palette {
    window_bg: "#1e1f22",
    surface_bg: "#2b2d31",
    surface_hover: "#35373c",
    surface_border: "#3f4147",
    text: "#e3e3e3",
    accent: "#3584e4",
};

const LIGHT: Palette = Palette {
    window_bg: "#f2f2f2",
    surface_bg: "#ffffff",
    surface_hover: "#ececec",
    surface_border: "#d0d0d0",
    text: "#1a1a1a",
    accent: "#3584e4",
};

/// CSS for the generic widgets OpusDM never hand-paints itself (window
/// background, entries, drop-downs, buttons, list-box rows (CSS node `list`), popovers) -
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
        "@define-color {ACCENT_COLOR_NAME} {accent}; \
         window {{ background-color: {wbg}; background-image: none; color: {fg}; }} \
         entry, dropdown, dropdown > button, spinbutton, button {{ \
             background-color: {sbg}; background-image: none; color: {fg}; \
             border: 1px solid {border}; border-radius: 4px; \
             box-shadow: none; \
         }} \
         button:hover, dropdown:hover, dropdown > button:hover {{ background-color: {hover}; }} \
         list, list > row {{ background-color: {sbg}; background-image: none; color: {fg}; outline: none; }} \
         list > row:hover {{ background-color: {hover}; }} \
         list > row:selected {{ background-color: {sbg}; box-shadow: inset 0 0 0 1px {accent}; }} \
         popover listview, popover listview > row {{ background-color: {sbg}; background-image: none; color: {fg}; }} \
         popover listview > row:hover {{ background-color: {hover}; }} \
         popover listview > row:selected {{ background-color: {hover}; color: {fg}; }} \
         popover scrolledwindow, popover viewport {{ background-color: transparent; }} \
         popover, popover.background, popover contents {{ background-color: {sbg}; background-image: none; color: {fg}; }} \
         scrolledwindow, viewport {{ background-color: {wbg}; background-image: none; }} \
         box, grid, stack, frame {{ background-color: {wbg}; background-image: none; color: {fg}; }} \
         button > *, dropdown > *, headerbar > * {{ background-color: transparent; }} \
         button box, button grid, button stack, button frame, \
         dropdown box, dropdown grid, dropdown stack, dropdown frame, \
         columnview box, columnview grid, columnview stack, columnview frame, \
         list box, list grid, list stack, list frame, \
         popover box, popover grid, popover stack, popover frame {{ background-color: transparent; }} \
         label {{ color: {fg}; }} \
         checkbutton, radiobutton {{ color: {fg}; }} \
         checkbutton > radio, checkbutton > check {{ \
             background-color: {sbg}; background-image: none; color: {fg}; \
             border: 1px solid {border}; box-shadow: none; \
         }} \
         checkbutton > radio:checked, checkbutton > check:checked {{ \
             background-color: {accent}; border-color: {accent}; color: #ffffff; \
         }} \
         headerbar {{ background-color: {wbg}; background-image: none; color: {fg}; }} \
         notebook > header {{ background-color: {wbg}; background-image: none; }} \
         notebook > header tab {{ background-color: {sbg}; background-image: none; color: {fg}; border: 1px solid {border}; }} \
         notebook > header tab:checked {{ background-color: {wbg}; background-image: none; }} ",
        wbg = p.window_bg,
        sbg = p.surface_bg,
        hover = p.surface_hover,
        border = p.surface_border,
        fg = p.text,
        accent = p.accent,
    )
}

/// The full stylesheet of a stand-alone OpusDM program: the chrome for the
/// theme mode, the user's look (fonts, surfaces) and the program's own
/// `extra_css`, in that order so the program rules win.
pub fn app_css(settings: &Settings, extra_css: &str) -> String {
    format!("{} {} {extra_css}", chrome_css(settings.appearance.theme_mode), crate::ui_look_css(settings))
}

/// Load the settings of `app_name`, apply its theme mode and install
/// [`app_css`] for the default display. Every stand-alone program calls this
/// once at start-up instead of building its own provider.
pub fn install_app_css(app_name: &str, extra_css: &str) {
    let settings = opusdm_ipc::settings::load_settings(app_name);
    apply_theme_mode(settings.appearance.theme_mode);
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&app_css(&settings, extra_css));
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_css_puts_the_program_rules_after_chrome_and_look() {
        let settings = Settings::default();
        let css = app_css(&settings, ".mine { color: red; }");
        assert!(css.starts_with(&chrome_css(settings.appearance.theme_mode)));
        assert!(css.contains(&crate::ui_look_css(&settings)));
        assert!(css.ends_with(".mine { color: red; }"));
    }

    #[test]
    fn chrome_css_defines_the_accent_before_anything_uses_it() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let css = chrome_css(mode);
            assert!(css.starts_with(&format!("@define-color {ACCENT_COLOR_NAME} ")), "{css}");
        }
    }

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
            "window {", "entry,", "button", "list,", "popover", "dropdown", "label", "notebook",
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
    fn nested_dropdown_and_button_containers_stay_transparent() {
        // A DropDown is `dropdown > button > box > stack > ...`: `button > *`
        // only resets the first box, so the inner stack still got the
        // window colour and showed as a dark patch behind the label.
        let css = chrome_css(ThemeMode::Dark);
        for selector in ["dropdown stack", "dropdown box", "button stack", "button box"] {
            assert!(css.contains(selector), "missing reset for {selector}");
        }
    }

    #[test]
    fn list_box_rules_target_the_real_gtk4_css_node_name() {
        // GTK4's ListBox CSS node is `list` (rows are `row`), not
        // `list-box`: a `list-box` selector matches nothing, which left each
        // row theme-coloured while its inner box got the window colour.
        let css = chrome_css(ThemeMode::Dark);
        assert!(!css.contains("list-box"));
        assert!(css.contains("list > row:selected"));
    }

    #[test]
    fn columnview_rows_are_not_covered_by_the_generic_container_paint() {
        // Same failure shape as the button/dropdown case above, one level
        // deeper: a ColumnView cell's icon+label layout is itself a box, so
        // the generic box/grid/stack/frame rule painted every Lister row
        // window-coloured, hiding `.row-dir`/`.row-file`/`.cell-selected`
        // (OpusDM's own, more specific per-row colouring) underneath it.
        let css = chrome_css(ThemeMode::Dark);
        for selector in ["columnview box", "list box", "popover box"] {
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
            assert!(css.contains(&"notebook".to_string()), "{mode:?} notebook rule");
        }
    }

    #[test]
    fn dropdown_popups_and_toggle_indicators_follow_the_selected_palette() {
        // Regression: a gtk::DropDown's popup is a `listview` (not `list`)
        // and a radio/check button draws its indicator on a `radio`/`check`
        // child node - both kept the base Adwaita (dark) colours in Light
        // mode, showing dark rows and dark radio circles on a light window.
        for (mode, p) in [(ThemeMode::Dark, &DARK), (ThemeMode::Light, &LIGHT)] {
            let css = chrome_css(mode);
            assert!(css.contains(&format!("popover listview, popover listview > row {{ background-color: {}", p.surface_bg)), "{mode:?} listview");
            assert!(css.contains(&format!("checkbutton > radio, checkbutton > check {{ background-color: {}", p.surface_bg)), "{mode:?} radio/check");
            assert!(css.contains("checkbutton > radio:checked"), "{mode:?} checked radio");
        }
    }

    #[test]
    fn listview_rules_are_scoped_to_popovers() {
        // Regression: an unscoped `listview > row` rule also matched the
        // Lister's file list (`columnview > listview > row`), painting every
        // row with the surface colour over the theme's `file_bg` ground.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let css = chrome_css(mode);
            assert!(!css.contains(" listview, listview"), "{mode:?}: unscoped listview rule");
            assert!(!css.contains("} listview"), "{mode:?}: unscoped listview rule");
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
