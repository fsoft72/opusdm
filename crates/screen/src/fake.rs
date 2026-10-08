//! A `ScreenBackend` that keeps the layout in memory and records what is
//! applied, for tests of the session, the hub logic and the UI.

use std::cell::{Cell, RefCell};

use crate::backend::ScreenBackend;
use crate::parse::Parsed;
use crate::{Layout, ScreenError};

/// Records every applied layout; can be told to fail.
pub struct FakeBackend {
    state: RefCell<Parsed>,
    applied: RefCell<Vec<Layout>>,
    failing: Cell<bool>,
}

impl FakeBackend {
    /// A system with these monitors, set as `layout`.
    pub fn new(parsed: Parsed) -> Self {
        Self { state: RefCell::new(parsed), applied: RefCell::new(Vec::new()), failing: Cell::new(false) }
    }

    /// The layouts applied so far (also the ones that failed).
    pub fn applied(&self) -> Vec<Layout> {
        self.applied.borrow().clone()
    }

    /// Make every `apply` fail (or work again).
    pub fn set_failing(&self, failing: bool) {
        self.failing.set(failing);
    }

    /// Plug in or out: the system now has these monitors and this layout.
    pub fn set_state(&self, parsed: Parsed) {
        *self.state.borrow_mut() = parsed;
    }
}

impl ScreenBackend for FakeBackend {
    /// The monitors and layout the system has now.
    fn query(&self) -> Result<Parsed, ScreenError> {
        Ok(self.state.borrow().clone())
    }

    /// Record the layout; when the backend is not failing it becomes the
    /// layout of the system.
    fn apply(&self, layout: &Layout) -> Result<(), ScreenError> {
        self.applied.borrow_mut().push(layout.clone());
        if self.failing.get() {
            return Err(ScreenError::CommandFailed { command: "fake".to_string(), stderr: "forced failure".to_string() });
        }
        self.state.borrow_mut().layout = layout.clone();
        Ok(())
    }
}

/// A laptop panel (on) and an external monitor (on, to its right), for tests.
#[cfg(test)]
pub(crate) fn two_monitor_system() -> Parsed {
    use crate::model::test_support::two_monitors;
    use crate::MonitorConfig;
    let monitors = two_monitors();
    let mut layout = Layout { monitors: monitors.iter().map(|m| MonitorConfig::default_for(m).unwrap()).collect() };
    layout.monitors[0].primary = true;
    layout.monitors[1].pos = (2560, 0);
    Parsed { monitors, layout }
}
