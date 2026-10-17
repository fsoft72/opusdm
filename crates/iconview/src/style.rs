//! Default styling for the icon view. Consumers call `install()` once; a host
//! theme can override any of these rules at a higher priority.

use gtk4 as gtk;
use std::sync::Once;

/// Baseline CSS for `IconItem`. Colours use named theme lookups so the widget
/// follows light/dark automatically.
pub const CSS: &str = "\
.icon-item { padding: 6px; border-radius: 6px; }
.icon-item.selected { background-color: alpha(@accent_bg_color, 0.35); }
.icon-item.drop-target { background-color: alpha(@accent_bg_color, 0.55); outline: 2px solid @accent_bg_color; outline-offset: -2px; }
.icon-item.cursor { outline: 1px dashed alpha(@accent_bg_color, 0.8); outline-offset: -2px; }
.icon-item-badge { background-color: alpha(black, 0.6); color: white; border-radius: 8px; padding: 0 4px; font-size: 11px; }
";

/// Name of the environment variable that turns on the grid debug overlay.
const GRID_DEBUG_ENV: &str = "GRID_DEBUG";

/// Extra CSS enabled by `GRID_DEBUG=1`: a red box around each item and around
/// its glyph and label, so the real footprint and inner spacing are visible.
/// `outline` is used instead of `border` so it never changes the layout.
pub const DEBUG_CSS: &str = "\
.icon-item { outline: 1px solid red; outline-offset: -1px; }
.icon-item-glyph { outline: 1px solid rgba(255, 0, 0, 0.5); }
.icon-item label { outline: 1px solid rgba(255, 0, 0, 0.5); }
";

static ONCE: Once = Once::new();

/// True when `GRID_DEBUG` is set to a non-empty, non-"0" value.
fn grid_debug_enabled() -> bool {
    match std::env::var(GRID_DEBUG_ENV) {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => false,
    }
}

/// Load `CSS` into the default display, plus `DEBUG_CSS` when `GRID_DEBUG` is
/// set. Safe to call repeatedly.
pub fn install() {
    ONCE.call_once(|| {
        let Some(display) = gtk::gdk::Display::default() else {
            return;
        };
        let provider = gtk::CssProvider::new();
        provider.load_from_data(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        if !grid_debug_enabled() {
            return;
        }
        let debug_provider = gtk::CssProvider::new();
        debug_provider.load_from_data(DEBUG_CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &debug_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    });
}
