//! Library of `opusdm-screen`: the monitor model, the `xrandr` parser and
//! command builder, the layout modes (extend, mirror, single), the saved
//! profiles and the apply / revert session. It has no GTK code, so the hub can
//! reuse it to reapply a saved profile when a monitor is plugged in.

pub mod backend;
pub mod commands;
pub mod error;
pub mod fake;
pub mod geometry;
pub mod model;
pub mod modes;
pub mod parse;
pub mod reapply;
pub mod session;
pub mod store;

pub use error::ScreenError;
pub use model::*;
