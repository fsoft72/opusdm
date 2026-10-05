//! Slideshow state: when the viewer advances by itself.

use crate::kind::MediaKind;

/// Seconds a picture stays up during the slideshow.
pub const SLIDESHOW_SECS: u64 = 4;

/// On / off switch and the advance rules.
#[derive(Debug, Default)]
pub struct Slideshow {
    on: bool,
}

impl Slideshow {
    /// Flip the switch; returns the new state.
    pub fn toggle(&mut self) -> bool {
        self.on = !self.on;
        self.on
    }

    /// True while the slideshow runs.
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// A timer is needed only for pictures; a video advances at its end.
    pub fn wants_timer(&self, kind: MediaKind) -> bool {
        self.on && kind == MediaKind::Image
    }

    /// A video that reached its end moves on when the slideshow runs.
    pub fn advance_on_end(&self) -> bool {
        self.on
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_and_toggles() {
        let mut s = Slideshow::default();
        assert!(!s.is_on());
        assert!(s.toggle());
        assert!(s.is_on());
        assert!(!s.toggle());
    }

    #[test]
    fn timer_only_for_pictures_while_on() {
        let mut s = Slideshow::default();
        assert!(!s.wants_timer(MediaKind::Image));
        s.toggle();
        assert!(s.wants_timer(MediaKind::Image));
        assert!(!s.wants_timer(MediaKind::Video));
    }

    #[test]
    fn video_end_advances_only_while_on() {
        let mut s = Slideshow::default();
        assert!(!s.advance_on_end());
        s.toggle();
        assert!(s.advance_on_end());
    }
}
