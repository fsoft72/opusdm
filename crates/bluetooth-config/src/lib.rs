//! Library of `opusdm-bluetooth-manager`: the window (`window`) that the
//! Bluetooth program and the Advanced tab of `opusdm-settings` share, the
//! "Add device" dialog, the worker that runs the BlueZ calls, and the
//! pairing agent dialogs (`agent_dialog`), also used by the panel. See
//! `docs/superpowers/specs/2026-10-23-bluetooth-design.md`.

pub mod agent_dialog;
#[cfg(test)]
mod gtk_test;
mod pages;
mod pair_dialog;
pub mod window;
pub mod worker;

pub use window::APP_CSS;
