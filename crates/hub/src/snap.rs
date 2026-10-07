//! Window snapping: owns the watcher of `opusdm-snap`, started with the hub
//! and switched on and off live by the desktop settings.

use opusdm_snap::Watcher;
use std::sync::OnceLock;

/// The running watcher, set once by [`start`].
static WATCHER: OnceLock<Watcher> = OnceLock::new();

/// Start the watcher, snapping on when `enabled`. A second call only applies
/// `enabled`. A failure to start is logged by the watcher and snapping stays
/// off.
pub fn start(enabled: bool) {
    if WATCHER.get().is_some() {
        set_enabled(enabled);
        return;
    }
    if let Some(watcher) = Watcher::start(enabled) {
        let _ = WATCHER.set(watcher);
    }
}

/// Switch snapping on or off. Does nothing when the watcher is not running.
pub fn set_enabled(enabled: bool) {
    if let Some(watcher) = WATCHER.get() {
        watcher.set_enabled(enabled);
    }
}
