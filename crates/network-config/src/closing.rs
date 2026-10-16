//! What the window does when it is closed with changes not saved: it closes
//! at once, after the save ends well, or not at all.

/// The state of a window close asked while the editor holds changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloseState {
    /// The user chose Save: close once the save ends well.
    close_after_save: bool,
    /// The window may close without asking again.
    closing: bool,
}

impl CloseState {
    /// Whether a close request may go on now, given whether the editor holds
    /// changes not saved.
    pub fn may_close(&self, dirty: bool) -> bool {
        self.closing || !dirty
    }

    /// The user chose Discard: the next close goes on.
    pub fn discard(&mut self) {
        self.closing = true;
    }

    /// The user chose Save and the save was sent: the window waits for it.
    pub fn save_then_close(&mut self) {
        self.close_after_save = true;
    }

    /// Whether the window waits for a save before closing.
    pub fn waits_for_save(&self) -> bool {
        self.close_after_save
    }

    /// A save ended well. Returns whether the window must close now.
    pub fn saved(&mut self) -> bool {
        if !self.close_after_save { return false; }
        self.close_after_save = false;
        self.closing = true;
        true
    }

    /// A job failed: the window stays open, so the user sees the error.
    pub fn failed(&mut self) {
        self.close_after_save = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_window_closes_and_a_dirty_one_asks() {
        let s = CloseState::default();
        assert!(s.may_close(false));
        assert!(!s.may_close(true));
    }

    #[test]
    fn save_waits_for_the_save_before_closing() {
        let mut s = CloseState::default();
        s.save_then_close();
        assert!(s.waits_for_save());
        assert!(!s.may_close(true), "the save is still running");
        assert!(s.saved(), "the window closes once saved");
        assert!(s.may_close(true));
        assert!(!s.waits_for_save());
    }

    #[test]
    fn a_failed_save_keeps_the_window_open() {
        let mut s = CloseState::default();
        s.save_then_close();
        s.failed();
        assert!(!s.waits_for_save());
        assert!(!s.saved(), "a later save does not close the window");
        assert!(!s.may_close(true));
    }

    #[test]
    fn a_save_not_asked_by_a_close_does_not_close() {
        let mut s = CloseState::default();
        assert!(!s.saved());
        assert!(!s.may_close(true));
    }

    #[test]
    fn discard_lets_the_close_go_on() {
        let mut s = CloseState::default();
        s.discard();
        assert!(s.may_close(true));
    }
}
