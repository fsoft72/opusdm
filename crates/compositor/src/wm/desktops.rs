//! Virtual desktops: how many there are and which one is shown.

/// Accepted range of the desktop count, inclusive.
pub const DESKTOP_COUNT_RANGE: (u32, u32) = (1, 16);

/// The virtual desktops: their count and the 0-based index of the shown one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Desktops {
    /// Number of desktops, inside `DESKTOP_COUNT_RANGE`.
    count: u32,
    /// Index of the shown desktop, always below `count`.
    current: u32,
}

impl Desktops {
    /// `count` desktops (clamped to `DESKTOP_COUNT_RANGE`), the first one shown.
    pub fn new(count: u32) -> Desktops {
        Desktops { count: _clamp_count(count), current: 0 }
    }

    /// Number of desktops.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Index of the shown desktop.
    pub fn current(&self) -> u32 {
        self.current
    }

    /// Shows the desktop `index`. False, and nothing changes, when `index`
    /// is not below the count.
    pub fn switch_to(&mut self, index: u32) -> bool {
        if index >= self.count { return false; }
        self.current = index;
        true
    }

    /// Sets the number of desktops (clamped to `DESKTOP_COUNT_RANGE`) and
    /// returns it; a shown desktop that no longer exists becomes the last one.
    pub fn set_count(&mut self, count: u32) -> u32 {
        self.count = _clamp_count(count);
        self.current = self.clamp_index(self.current);
        self.count
    }

    /// `index` when it is a desktop, else the last desktop.
    pub fn clamp_index(&self, index: u32) -> u32 {
        index.min(self.count.saturating_sub(1))
    }
}

/// `count` clamped to `DESKTOP_COUNT_RANGE`.
fn _clamp_count(count: u32) -> u32 {
    count.clamp(DESKTOP_COUNT_RANGE.0, DESKTOP_COUNT_RANGE.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_stays_inside_the_count() {
        let mut d = Desktops::new(4);
        assert!(d.switch_to(3));
        assert!(!d.switch_to(4));
        assert_eq!(d.current(), 3);
    }

    #[test]
    fn shrinking_clamps_the_current_desktop_and_the_count() {
        let mut d = Desktops::new(4);
        d.switch_to(3);
        assert_eq!(d.set_count(2), 2);
        assert_eq!(d.current(), 1);
        assert_eq!(d.set_count(0), 1);
        assert_eq!(Desktops::new(40).count(), 16);
    }
}
