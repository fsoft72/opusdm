//! Library of `opusdm-network-manager`: the list sections, the editing
//! session, the close state and the worker that runs the NetworkManager
//! calls (tested without a display), and the GTK window (`window`), which the
//! Network program and the Advanced tab of `opusdm-settings` share. See
//! `docs/superpowers/specs/2026-10-12-network-manager-design.md`.

pub mod closing;
mod dialogs;
mod editor;
mod form;
pub mod groups;
mod pages;
pub mod session;
mod vpn_pages;
pub mod window;
pub mod worker;

/// A field with an error gets a red outline, an error message red text and
/// the "not saved yet" banner the warning colour.
pub const APP_CSS: &str = "
.error:not(label) { outline: 1px solid #e01b24; outline-offset: -1px; }
label.error { color: #ff7b63; }
label.warning { color: #f6d32d; }
";
