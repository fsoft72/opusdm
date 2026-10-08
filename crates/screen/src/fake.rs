//! A `ScreenBackend` that keeps the layout in memory and records what is
//! applied, for tests of the session, the hub logic and the UI.

use std::cell::{Cell, RefCell};

use crate::backend::ScreenBackend;
use crate::parse::Parsed;
use crate::{Layout, Mode, Monitor, MonitorConfig, ScreenError};

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

/// A monitor with the given modes (`(w, h, rates)`), the first preferred.
pub fn monitor(name: &str, edid: &str, modes: &[(u32, u32, &[f64])]) -> Monitor {
    Monitor {
        name: name.to_string(),
        edid_id: edid.to_string(),
        modes: modes.iter().map(|(w, h, r)| Mode { width: *w, height: *h, rates: r.to_vec() }).collect(),
        preferred: modes.first().map(|(w, h, _)| (*w, *h)),
    }
}

/// A laptop panel and an external monitor.
pub fn two_monitors() -> Vec<Monitor> {
    vec![
        monitor("eDP-1", "aaaa", &[(2560, 1600, &[240.0, 60.0]), (1920, 1080, &[60.0])]),
        monitor("HDMI-1", "bbbb", &[(2560, 1440, &[60.0]), (1920, 1080, &[60.0, 50.0])]),
    ]
}

/// A laptop panel (primary) and an external monitor on its right, both on.
/// Used by the tests and by `opusdm-screen --demo`.
pub fn two_monitor_system() -> Parsed {
    let monitors = two_monitors();
    let mut layout = Layout { monitors: monitors.iter().map(|m| MonitorConfig::default_for(m).unwrap()).collect() };
    layout.monitors[0].primary = true;
    layout.monitors[1].pos = (2560, 0);
    Parsed { monitors, layout }
}
