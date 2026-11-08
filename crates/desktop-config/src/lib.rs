//! Library surface of `opus-desktop-config`, reused by `opusdm-settings` for the
//! background list editor and its colour-entry widgets.

pub mod background_ui;
/// The Dock sections moved into `opusdm-dock`; kept here until the Settings
/// tool drops its Dock tab.
pub use opusdm_dock::settings_ui as dock_ui;
pub mod model;
pub mod ui;

#[cfg(test)]
mod gtk_test;
