//! Font size of the text documents (Markdown and source): a percentage of the
//! normal size, changed by the bar buttons and shared by both views. The size
//! is a CSS rule on the `viewer-doc-text` class, so the line numbers follow.

use gtk4 as gtk;
use std::cell::Cell;

/// CSS class of the text views whose font is scaled.
pub const DOC_TEXT_CLASS: &str = "viewer-doc-text";
/// Smallest, normal and biggest size, and the change of one button press, in percent.
const MIN_PERCENT: u32 = 50;
const DEFAULT_PERCENT: u32 = 100;
const MAX_PERCENT: u32 = 300;
const STEP_PERCENT: u32 = 10;

/// The current size and the style provider that applies it.
pub struct FontScale {
    percent: Cell<u32>,
    provider: gtk::CssProvider,
}

impl FontScale {
    /// The normal size, with its provider installed on the default display
    /// above the application look.
    pub fn new() -> FontScale {
        let provider = gtk::CssProvider::new();
        match gtk::gdk::Display::default() {
            Some(display) => gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1),
            None => opusdm_ipc::err_log!("viewer", "no display: the text size cannot change"),
        }
        let scale = FontScale { percent: Cell::new(DEFAULT_PERCENT), provider };
        scale._apply();
        scale
    }

    /// Make the text one step bigger (or smaller).
    pub fn change(&self, bigger: bool) {
        self.percent.set(step_percent(self.percent.get(), bigger));
        self._apply();
    }

    /// `  -  120%` for the info bar, nothing at the normal size.
    pub fn label(&self) -> String {
        let percent = self.percent.get();
        if percent == DEFAULT_PERCENT { String::new() } else { format!("  -  {percent}%") }
    }

    /// Load the rule of the current size.
    fn _apply(&self) {
        self.provider.load_from_data(&css(self.percent.get()));
    }
}

/// The size one step bigger (or smaller) than `percent`, kept in range.
pub fn step_percent(percent: u32, bigger: bool) -> u32 {
    let next = if bigger { percent.saturating_add(STEP_PERCENT) } else { percent.saturating_sub(STEP_PERCENT) };
    next.clamp(MIN_PERCENT, MAX_PERCENT)
}

/// The CSS rule that gives the text views `percent` of the normal size.
pub fn css(percent: u32) -> String {
    format!("textview.{DOC_TEXT_CLASS} {{ font-size: {percent}%; }}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_go_up_and_down() {
        assert_eq!(step_percent(100, true), 110);
        assert_eq!(step_percent(100, false), 90);
    }

    #[test]
    fn steps_stay_in_range() {
        assert_eq!(step_percent(MAX_PERCENT, true), MAX_PERCENT);
        assert_eq!(step_percent(MIN_PERCENT, false), MIN_PERCENT);
        assert_eq!(step_percent(5, true), MIN_PERCENT);
    }

    #[test]
    fn css_targets_the_text_class() {
        assert_eq!(css(120), "textview.viewer-doc-text { font-size: 120%; }");
    }
}
