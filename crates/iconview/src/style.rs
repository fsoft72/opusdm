//! Default styling for the icon view. Consumers call `install()` once; a host
//! theme can override any of these rules at a higher priority.

use gtk4 as gtk;
use std::sync::Once;

/// Baseline CSS for `IconItem`. Colours use named theme lookups so the widget
/// follows light/dark automatically.
pub const CSS: &str = "\
.icon-item { padding: 6px; border-radius: 6px; }
.icon-item.selected { background-color: alpha(@accent_bg_color, 0.35); }
.icon-item.cursor { outline: 1px dashed alpha(@accent_bg_color, 0.8); outline-offset: -2px; }
.icon-item-glyph { font-family: \"Symbols Nerd Font\", \"Nerd Font\", monospace; font-size: 32px; }
";

static ONCE: Once = Once::new();

/// Load `CSS` into the default display. Safe to call repeatedly.
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
    });
}
