//! Library of `opusdm-input-config`: the settings model, the parsers for the
//! X tools, the commands that apply a change and the live / revert session.
//! It has no GTK code, so the hub can reuse it to reapply the saved values.

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

pub use error::InputError;
pub use model::*;
