//! Library of `opusdm-input-config`: the settings model, the parsers for the
//! X tools, the commands that apply a change and the live / revert session.
//! It has no GTK code, so the hub can reuse it to reapply the saved values.

pub mod error;
pub mod model;
pub mod parse;
pub mod store;

pub use error::InputError;
pub use model::*;
