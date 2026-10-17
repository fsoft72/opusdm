//! A shareable one-way cancel flag.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A cloneable flag. Every clone observes the same state; once set it stays set.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<Flags>);

/// The shared state of a [`CancelToken`].
#[derive(Default)]
struct Flags {
    cancelled: AtomicBool,
    /// Set by [`CancelToken::cancel_sparing_programs`]: a program the
    /// cancelled work waits on is left running instead of killed.
    spare_programs: AtomicBool,
}

impl CancelToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent. Wins over an earlier
    /// [`cancel_sparing_programs`](Self::cancel_sparing_programs): the
    /// programs are not spared any more.
    pub fn cancel(&self) {
        self.0.spare_programs.store(false, Ordering::SeqCst);
        self.0.cancelled.store(true, Ordering::SeqCst);
    }

    /// Request cancellation, but leave alone a program the work is waiting
    /// on: the work stops once that program ends by itself. Does nothing on
    /// a token already cancelled, so it never softens a [`cancel`](Self::cancel).
    pub fn cancel_sparing_programs(&self) {
        if self.is_cancelled() { return; }
        self.0.spare_programs.store(true, Ordering::SeqCst);
        self.0.cancelled.store(true, Ordering::SeqCst);
    }

    /// Whether [`cancel`](Self::cancel) has been called on this token or a clone.
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Whether the cancel asked to leave running programs alone.
    pub fn spares_programs(&self) -> bool {
        self.0.spare_programs.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_shares_state() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!a.is_cancelled());
        b.cancel();
        assert!(a.is_cancelled());
        assert!(b.is_cancelled());
    }

    #[test]
    fn a_sparing_cancel_cancels_and_spares_programs() {
        let a = CancelToken::new();
        let b = a.clone();
        b.cancel_sparing_programs();
        assert!(a.is_cancelled());
        assert!(a.spares_programs());
    }

    #[test]
    fn a_full_cancel_wins_over_a_sparing_one_in_either_order() {
        let t = CancelToken::new();
        t.cancel_sparing_programs();
        t.cancel();
        assert!(t.is_cancelled() && !t.spares_programs(), "a later cancel kills");
        let t = CancelToken::new();
        t.cancel();
        t.cancel_sparing_programs();
        assert!(t.is_cancelled() && !t.spares_programs(), "a later sparing cancel does not soften it");
    }
}
