//! A shareable one-way cancel flag.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A cloneable flag. Every clone observes the same state; once set it stays set.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether [`cancel`](Self::cancel) has been called on this token or a clone.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
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
}
