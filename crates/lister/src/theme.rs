//! Theme stylesheet generation and CSS styling for Lister.

use opusdm_ipc::Settings;

/// Generate the CSS string for the settings-driven stylesheet.
pub fn generate_css(settings: &Settings) -> String {
    let c = &settings.colors;
    // Opt-in: carry the file colours into the window chrome as well, so a dark
    // preset does not leave a light toolbar around a dark list. Redefining the
    // two named colours also reaches the button gradients, which are written in
    // terms of @theme_bg_color. Must come first: GTK resolves @name against
    // definitions seen earlier in the stylesheet.
    let chrome_css = if settings.appearance.theme_window {
        format!(
            "@define-color theme_bg_color {bg}; @define-color theme_fg_color {fg}; \
             window {{ background-color: {bg}; color: {fg}; }} ",
            bg = c.file_bg,
            fg = c.file_fg
        )
    } else {
        String::new()
    };
    // The list ground has to be painted explicitly: row-dir/row-file only
    // colour the cells, so a dark preset would otherwise leave the empty area
    // below the last row in the GTK theme's own (usually light) colour.
    let list_css = format!(
        "columnview, columnview > listview {{ background-color: {}; }} ",
        c.file_bg
    );
    let mono_css = if settings.appearance.monospace_list {
        "columnview > listview > row > cell label { font-family: monospace; } "
    } else {
        ""
    };
    let base = format!(
        ".stat-source {{ background-color: {}; color: {}; }} \
         .stat-dest {{ background-color: {}; color: {}; }} \
         .stat-source, .stat-dest {{ padding: 0 4px; }} \
         .row-dir {{ background-color: {}; color: {}; }} \
         .row-file {{ background-color: {}; color: {}; }} \
         .pin-btn {{ min-height: 0; min-width: 0; padding: 2px 6px; }} \
         button.opus-btn {{ \
             min-height: 30px; \
             min-width: 32px; \
             padding: 3px 6px; \
             margin: 1px 2px; \
             border-radius: 3px; \
             border-top: 1px solid rgba(255, 255, 255, 0.4); \
             border-left: 1px solid rgba(255, 255, 255, 0.4); \
             border-right: 1px solid rgba(0, 0, 0, 0.6); \
             border-bottom: 1px solid rgba(0, 0, 0, 0.6); \
             background: linear-gradient(to bottom, alpha(@theme_bg_color, 0.95), alpha(@theme_bg_color, 0.75)); \
             box-shadow: 1px 1px 1px rgba(0, 0, 0, 0.2); \
         }} \
         button.opus-btn:hover {{ \
             border-top: 1px solid rgba(255, 255, 255, 0.7); \
             border-left: 1px solid rgba(255, 255, 255, 0.7); \
             border-right: 1px solid rgba(0, 0, 0, 0.7); \
             border-bottom: 1px solid rgba(0, 0, 0, 0.7); \
             background: linear-gradient(to bottom, alpha(@theme_bg_color, 1.0), alpha(@theme_bg_color, 0.85)); \
         }} \
         button.opus-btn:active, button.opus-btn:checked {{ \
             border-top: 1px solid rgba(0, 0, 0, 0.7); \
             border-left: 1px solid rgba(0, 0, 0, 0.7); \
             border-right: 1px solid rgba(255, 255, 255, 0.5); \
             border-bottom: 1px solid rgba(255, 255, 255, 0.5); \
             background: linear-gradient(to bottom, alpha(@theme_bg_color, 0.6), alpha(@theme_bg_color, 0.85)); \
             box-shadow: inset 1px 1px 2px rgba(0, 0, 0, 0.4); \
         }} \
         columnview > listview > row {{ padding: 0; margin: 0; border: none; min-height: 0; }} \
         columnview > listview > row > cell {{ padding: 0; min-height: 0; }} \
         /* Transparent border: without it the theme leaves a 1-2px dark gap
            between rows; with it the labels stack flush. */
         .row-dir, .row-file {{ padding: 0 4px; border: 1px solid transparent; }} \
         .cell-selected {{ background-color: {}; color: {}; }} \
         .filter-bar {{ background: alpha(@theme_bg_color, 0.8); border-bottom: 1px solid rgba(0, 0, 0, 0.2); padding: 2px 4px; }} \
         .dim-label {{ opacity: 0.7; font-size: 0.85em; }} \
         .command-palette {{ background: @theme_bg_color; }} \
         .palette-search {{ font-size: 1.1em; padding: 4px; }} \
         .palette-cat {{ font-size: 0.8em; font-weight: bold; opacity: 0.6; }} \
         .palette-title {{ font-weight: 500; }} \
         .palette-shortcut {{ font-size: 0.8em; opacity: 0.7; background: rgba(0, 0, 0, 0.15); border-radius: 3px; padding: 1px 4px; }} \
         .preview-root {{ background: @theme_bg_color; }} \
         .preview-header {{ background: alpha(@theme_bg_color, 0.95); border-bottom: 1px solid rgba(0,0,0,0.15); }} \
         .preview-title {{ font-weight: bold; font-size: 1.1em; }} \
         .terminal-drawer {{ background: #121417; border-top: 1px solid rgba(255, 255, 255, 0.15); }} \
         .terminal-header {{ background: #1a1d24; border-bottom: 1px solid rgba(255, 255, 255, 0.1); padding: 2px 6px; }} \
         .terminal-title {{ font-family: monospace; font-size: 0.85em; opacity: 0.9; color: #58a6ff; }} \
         .terminal-view {{ font-family: monospace; font-size: 0.9em; background: #121417; color: #c9d1d9; }} \
         .terminal-entry {{ font-family: monospace; font-size: 0.9em; background: #161b22; color: #58a6ff; }} \
         .terminal-prompt {{ font-family: monospace; font-weight: bold; color: #3fb950; }} \
         .success-text {{ color: #2ea043; font-weight: 500; }} \
         .error-text {{ color: #da3633; font-weight: bold; }}",
        c.source_bg, c.source_fg,
        c.dest_bg, c.dest_fg,
        c.dir_bg, c.dir_fg,
        c.file_bg, c.file_fg,
        c.selected_bg, c.selected_fg,
    );
    format!("{chrome_css}{list_css}{mono_css}{base}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_css_with_colors() {
        let settings = Settings::default();
        let css = generate_css(&settings);
        assert!(css.contains(".stat-source"));
        assert!(css.contains(".stat-dest"));
        assert!(css.contains(".row-dir"));
        assert!(css.contains(".row-file"));
    }

    #[test]
    fn every_preset_colour_reaches_the_stylesheet() {
        for preset in opusdm_ipc::ThemePreset::ALL {
            let settings = Settings {
                colors: preset.colors(),
                appearance: Default::default(),
                behavior: Default::default(),
            };
            let css = generate_css(&settings);
            let c = &settings.colors;
            for hex in [
                &c.source_bg, &c.source_fg, &c.dest_bg, &c.dest_fg,
                &c.dir_bg, &c.dir_fg, &c.file_bg, &c.file_fg,
                &c.selected_bg, &c.selected_fg,
            ] {
                assert!(css.contains(hex.as_str()), "{}: {hex} missing", preset.id());
            }
            // The list ground must be painted, or a dark preset shows the GTK
            // theme's light background below the last row.
            assert!(css.contains("columnview, columnview > listview"));
        }
    }

    #[test]
    fn monospace_rule_follows_the_setting() {
        // Matched on the selector: the terminal classes always use a monospace
        // font, so the bare declaration is present either way.
        const RULE: &str = "columnview > listview > row > cell label { font-family: monospace; }";
        let mut settings = Settings::default();
        assert!(!generate_css(&settings).contains(RULE));
        settings.appearance.monospace_list = true;
        assert!(generate_css(&settings).contains(RULE));
    }

    #[test]
    fn selection_colours_are_no_longer_hardcoded_to_the_gtk_theme() {
        let mut settings = Settings::default();
        settings.colors.selected_bg = "#123456".into();
        settings.colors.selected_fg = "#ABCDEF".into();
        let css = generate_css(&settings);
        assert!(css.contains(".cell-selected { background-color: #123456; color: #ABCDEF; }"));
        assert!(!css.contains("@theme_selected_bg_color"));
    }

    #[test]
    fn window_chrome_is_only_repainted_when_asked() {
        let mut settings = Settings {
            colors: opusdm_ipc::ThemePreset::Cyberpunk.colors(),
            appearance: Default::default(),
            behavior: Default::default(),
        };
        assert!(!generate_css(&settings).contains("@define-color"));

        settings.appearance.theme_window = true;
        let css = generate_css(&settings);
        assert!(css.contains("@define-color theme_bg_color #1A1B26;"));
        assert!(css.starts_with("@define-color"), "definitions must precede any use");
        assert!(css.contains("window { background-color: #1A1B26; color: #C0CAF5; }"));
    }

    /// Parse every preset's stylesheet with the real GTK CSS parser. A typo in
    /// a selector makes GTK drop the rule silently at runtime, so catching it
    /// here is the only cheap way to know the themes actually apply.
    /// Skipped when no display is available (gtk::init fails).
    #[test]
    fn every_preset_stylesheet_parses_without_error() {
        use gtk4 as gtk;
        if gtk::init().is_err() {
            return;
        }
        let provider = gtk::CssProvider::new();
        let errors = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        {
            let errors = errors.clone();
            provider.connect_parsing_error(move |_, section, err| {
                errors.borrow_mut().push(format!("{section}: {err}"));
            });
        }
        for preset in opusdm_ipc::ThemePreset::ALL {
            // Every toggle on, so the conditional rules are parsed too.
            let settings = Settings {
                colors: preset.colors(),
                appearance: opusdm_ipc::Appearance {
                    preset: preset.id().to_string(),
                    file_icons: true,
                    type_indicators: true,
                    monospace_list: true,
                    theme_window: true,
                },
                behavior: Default::default(),
            };
            provider.load_from_data(&generate_css(&settings));
            assert!(
                errors.borrow().is_empty(),
                "{}: {:?}",
                preset.id(),
                errors.borrow()
            );
        }
    }
}
