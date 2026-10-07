//! A `Backend` that records what is set, for the tests of the detection and
//! of the worker.

use std::sync::{Arc, Mutex};

use crate::backend::Backend;
use crate::BrightnessError;

/// Keeps the brightness in memory and records every `set`; it can be made to
/// fail.
pub struct FakeBackend {
    current: Mutex<u8>,
    applied: Arc<Mutex<Vec<u8>>>,
    failing: bool,
}

impl FakeBackend {
    /// A backend that starts at `percent`.
    pub fn new(percent: u8) -> Self {
        Self { current: Mutex::new(percent), applied: Arc::new(Mutex::new(Vec::new())), failing: false }
    }

    /// A backend whose `get` and `set` always fail.
    pub fn failing() -> Self {
        Self { failing: true, ..Self::new(0) }
    }

    /// A handle on the list of the values set so far (valid after the
    /// backend was moved into a worker).
    pub fn applied_handle(&self) -> Arc<Mutex<Vec<u8>>> {
        self.applied.clone()
    }

    /// The error of a failing backend.
    fn error() -> BrightnessError {
        BrightnessError::CommandFailed { command: "fake".to_string(), stderr: "forced failure".to_string() }
    }
}

impl Backend for FakeBackend {
    /// "fake".
    fn name(&self) -> &'static str {
        "fake"
    }

    /// The value given to `new`, or the last one set.
    fn get(&self) -> Result<u8, BrightnessError> {
        if self.failing {
            return Err(Self::error());
        }
        Ok(*self.current.lock().unwrap())
    }

    /// Record the value, or fail when told to.
    fn set(&self, percent: u8) -> Result<(), BrightnessError> {
        if self.failing {
            return Err(Self::error());
        }
        *self.current.lock().unwrap() = percent;
        self.applied.lock().unwrap().push(percent);
        Ok(())
    }
}
