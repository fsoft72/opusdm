//! Stand-in for `terminal.rs` when the `terminal` feature is off: the same
//! surface, but no shell. `toggle` only logs, so a missing terminal is
//! visible instead of silent.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use std::path::Path;
use std::rc::Rc;

/// Minimum drawer height, as in the real drawer.
pub const MIN_DRAWER_HEIGHT: i32 = 90;

/// Default drawer height, as in the real drawer.
pub const DEFAULT_DRAWER_HEIGHT: i32 = 260;

/// A drawer that is never shown.
pub struct TerminalDrawer {
    pub container: gtk::Box,
}

impl TerminalDrawer {
    /// An empty, hidden container.
    pub fn new() -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_visible(false);
        Rc::new(Self { container })
    }

    /// Nothing to wire.
    pub fn attach(self: &Rc<Self>, _ui: &Rc<Ui>) {}

    /// Nothing to keep in sync.
    pub fn sync_cwd(&self, _new_cwd: &Path) {}

    /// The terminal is not built in: say so in the log.
    pub fn toggle(self: &Rc<Self>) {
        opusdm_ipc::err_log!("opusdm-lister", "the terminal drawer is not built in (feature `terminal` is off)");
    }
}
