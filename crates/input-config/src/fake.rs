//! A `Backend` that records what is applied, for tests of the session, the
//! hub logic and the UI helpers.

use std::cell::{Cell, RefCell};

use crate::backend::{Backend, Status};
use crate::change::Change;
use crate::{InputError, InputSettings};

/// Records every applied change; can be told to fail.
pub struct FakeBackend {
    current: InputSettings,
    status: Status,
    applied: RefCell<Vec<Change>>,
    failing: Cell<bool>,
    signature: RefCell<String>,
    refreshes: Cell<usize>,
}

impl FakeBackend {
    /// A backend whose system has `current` and every group available.
    pub fn new(current: InputSettings) -> Self {
        Self {
            current,
            status: Status::all_ok(),
            applied: RefCell::new(Vec::new()),
            failing: Cell::new(false),
            signature: RefCell::new(String::new()),
            refreshes: Cell::new(0),
        }
    }

    /// The changes applied so far.
    pub fn applied(&self) -> Vec<Change> {
        self.applied.borrow().clone()
    }

    /// Make every `apply` fail (or work again).
    pub fn set_failing(&self, failing: bool) {
        self.failing.set(failing);
    }

    /// Set what `signature` returns.
    pub fn set_signature(&self, signature: &str) {
        *self.signature.borrow_mut() = signature.to_string();
    }

    /// How many times `refresh` was called.
    pub fn refresh_count(&self) -> usize {
        self.refreshes.get()
    }
}

impl Backend for FakeBackend {
    /// All groups available.
    fn status(&self) -> Status {
        self.status.clone()
    }

    /// The settings given to `new`.
    fn read_current(&self) -> InputSettings {
        self.current.clone()
    }

    /// Record the change, or fail when told to (the change is still recorded).
    fn apply(&self, change: &Change) -> Result<(), InputError> {
        self.applied.borrow_mut().push(change.clone());
        if self.failing.get() {
            return Err(InputError::CommandFailed { command: "fake".to_string(), stderr: "forced failure".to_string() });
        }
        Ok(())
    }

    /// The value set with `set_signature`.
    fn signature(&self) -> String {
        self.signature.borrow().clone()
    }

    /// Count the call.
    fn refresh(&self) {
        self.refreshes.set(self.refreshes.get() + 1);
    }
}
