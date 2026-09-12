//! Detects whether a "Nerd Font" (a patched font covering the Private Use
//! Area codepoints `glyphs::glyph_for` returns) is actually installed.
//! Without one, Pango falls back to a "last resort" font that renders those
//! codepoints as a literal hex tofu box - worse than no icon at all - so
//! callers must check this before deciding whether to show a glyph.

use crate::glyphs::GLYPH_DIR;
use gtk4 as gtk;
use gtk::prelude::*;
use std::sync::OnceLock;

static AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Whether the current font configuration can render `glyphs::glyph_for`'s
/// codepoints. Computed once per process (font configuration does not
/// change while the app runs) using a throwaway, never-shown widget - safe
/// to call only after `gtk::init()` has succeeded.
pub fn nerd_font_available() -> bool {
    *AVAILABLE.get_or_init(probe)
}

fn probe() -> bool {
    let probe_widget = gtk::Label::new(None);
    let context = probe_widget.pango_context();
    let desc = context.font_description().unwrap_or_else(gtk::pango::FontDescription::new);
    match context.load_font(&desc) {
        Some(font) => font.has_char(GLYPH_DIR),
        None => false,
    }
}
