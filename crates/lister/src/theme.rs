//! Theme stylesheet generation and CSS styling for Lister.

use opusdm_ipc::Settings;

/// Generate the CSS string for the settings-driven stylesheet.
pub fn generate_css(settings: &Settings) -> String {
    let c = &settings.colors;
    format!(
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
         .cell-selected {{ background-color: @theme_selected_bg_color; \
                            color: @theme_selected_fg_color; }} \
         .filter-bar {{ background: alpha(@theme_bg_color, 0.8); border-bottom: 1px solid rgba(0, 0, 0, 0.2); padding: 2px 4px; }} \
         .dim-label {{ opacity: 0.7; font-size: 0.85em; }} \
         .command-palette {{ background: @theme_bg_color; }} \
         .palette-search {{ font-size: 1.1em; padding: 4px; }} \
         .palette-cat {{ font-size: 0.8em; font-weight: bold; opacity: 0.6; }} \
         .palette-title {{ font-weight: 500; }} \
         .palette-shortcut {{ font-size: 0.8em; opacity: 0.7; background: rgba(0, 0, 0, 0.15); border-radius: 3px; padding: 1px 4px; }}",
        c.source_bg, c.source_fg,
        c.dest_bg, c.dest_fg,
        c.dir_bg, c.dir_fg,
        c.file_bg, c.file_fg,
    )
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
}
