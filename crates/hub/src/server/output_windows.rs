//! The `Run` output windows the Listers show: which Lister owns each one and
//! the window id it was given in `ToLister::OutputOpen`.

use std::collections::HashMap;

/// A run's step that opened an output window: `(run id, step index)`.
pub(super) type StepKey = (u64, usize);

/// One open output window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OutputWindow {
    /// The connection showing the window.
    pub(super) owner: u64,
    /// The id the Lister knows the window by.
    pub(super) id: u64,
}

/// Every open output window, by the step that opened it. Window ids are
/// handed out once each, so two steps never share one, however many steps
/// a run has.
pub(super) struct OutputWindows {
    windows: HashMap<StepKey, OutputWindow>,
    next_id: u64,
}

impl OutputWindows {
    /// No window open yet.
    pub(super) fn new() -> Self {
        Self { windows: HashMap::new(), next_id: 1 }
    }

    /// Step `key` opens a window on `owner`: give it a fresh id. A window
    /// the step already had is replaced.
    pub(super) fn open(&mut self, key: StepKey, owner: u64) -> OutputWindow {
        let window = OutputWindow { owner, id: self.next_id };
        self.next_id += 1;
        self.windows.insert(key, window);
        window
    }

    /// The window step `key` opened, if it is still open.
    pub(super) fn get(&self, key: StepKey) -> Option<OutputWindow> {
        self.windows.get(&key).copied()
    }

    /// Forget the window of step `key` (it ended, or its Lister is gone).
    pub(super) fn close(&mut self, key: StepKey) {
        self.windows.remove(&key);
    }

    /// Forget every window shown by connection `owner`.
    pub(super) fn forget_owner(&mut self, owner: u64) {
        self.windows.retain(|_, w| w.owner != owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_never_share_a_window_id() {
        let mut windows = OutputWindows::new();
        // `run_id * 1000 + step` gave both of these the id 2000.
        let a = windows.open((1, 1000), 7);
        let b = windows.open((2, 0), 7);
        assert_ne!(a.id, b.id);
        assert_eq!(windows.get((1, 1000)), Some(a));
        assert_eq!(windows.get((2, 0)), Some(b));
    }

    #[test]
    fn close_and_forget_owner_drop_windows() {
        let mut windows = OutputWindows::new();
        windows.open((1, 0), 7);
        windows.open((1, 1), 8);
        windows.open((2, 0), 7);
        windows.close((1, 1));
        assert_eq!(windows.get((1, 1)), None);
        windows.forget_owner(7);
        assert_eq!(windows.get((1, 0)), None);
        assert_eq!(windows.get((2, 0)), None);
    }
}
