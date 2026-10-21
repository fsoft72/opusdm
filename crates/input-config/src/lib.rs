//! Library of `opusdm-input-config`: the settings model, the parsers for the
//! X tools, the commands that apply a change, the live / revert session and
//! the worker thread that runs it, and the GTK tabs (`ui`), which the Input
//! window and the Advanced tab of `opusdm-settings` share. Only `ui` needs GTK;
//! the rest is what the hub reuses to reapply the saved values.

pub mod apply;
pub mod backend;
pub mod change;
pub mod commands;
pub mod error;
pub mod fake;
pub mod labwc;
pub mod model;
pub mod parse;
pub mod session;
pub mod store;
pub mod ui;
pub mod wayfire;
pub mod worker;

pub use error::InputError;
pub use model::*;
