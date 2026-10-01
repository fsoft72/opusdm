//! The "a function is running" indicator (F-41): how many functions this
//! Lister started that have not reported Done or Failed yet.

use std::cell::Cell;

/// Count of running functions started from this Lister.
#[derive(Default)]
pub struct Busy {
    running: Cell<u32>,
}

impl Busy {
    /// A function was handed to the hub.
    pub fn start(&self) {
        self.running.set(self.running.get().saturating_add(1));
    }

    /// A function reported Done or Failed. Never goes below zero.
    pub fn finish(&self) {
        self.running.set(self.running.get().saturating_sub(1));
    }

    /// How many are running.
    #[cfg(test)]
    pub fn running(&self) -> u32 {
        self.running.get()
    }

    /// The text shown next to the spinner, `None` when idle.
    pub fn label(&self) -> Option<String> {
        match self.running.get() {
            0 => None,
            1 => Some("Running a function...".to_string()),
            n => Some(format!("Running {n} functions...")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_runs_and_describes_them() {
        let busy = Busy::default();
        assert_eq!(busy.label(), None);
        busy.start();
        assert_eq!(busy.label().as_deref(), Some("Running a function..."));
        busy.start();
        assert_eq!((busy.running(), busy.label().as_deref()), (2, Some("Running 2 functions...")));
        busy.finish();
        busy.finish();
        assert_eq!(busy.label(), None);
    }

    #[test]
    fn a_stray_finish_never_underflows() {
        let busy = Busy::default();
        busy.finish();
        assert_eq!(busy.running(), 0);
        busy.start();
        assert_eq!(busy.running(), 1);
    }
}
