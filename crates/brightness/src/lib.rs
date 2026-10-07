//! Library of `opusdm-brightness`: the monitor brightness behind one small
//! `Backend` trait (laptop backlight, DDC/CI through `ddcutil`, software
//! gamma through `xrandr`), the detection of the first one that works and a
//! worker thread that applies the slider values off the UI thread. It has no
//! GTK code.

pub mod backend;
pub mod error;
pub mod fake;
pub mod parse;
pub mod service;

pub use backend::{detect, detect_from, Backend, Candidate, Detected};
pub use error::BrightnessError;
pub use service::{spawn, Controller, Event};

/// The lowest brightness the slider and the backends accept. Zero would turn
/// the screen black (or off) with no way to see the slider again.
pub const MIN_PERCENT: u8 = 10;

/// The highest brightness.
pub const MAX_PERCENT: u8 = 100;

/// Limit `percent` to `MIN_PERCENT..=MAX_PERCENT`.
pub fn clamp_percent(percent: u8) -> u8 {
    percent.clamp(MIN_PERCENT, MAX_PERCENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_the_screen_visible() {
        assert_eq!(clamp_percent(0), MIN_PERCENT);
        assert_eq!(clamp_percent(55), 55);
        assert_eq!(clamp_percent(200), MAX_PERCENT);
    }
}
