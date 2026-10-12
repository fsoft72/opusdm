//! Library of `opusdm-network-manager`: the list sections, the editing
//! session and the worker that runs the NetworkManager calls. No GTK code,
//! so it is tested without a display. See
//! `docs/superpowers/specs/2026-10-12-network-manager-design.md`.

pub mod groups;
pub mod session;
pub mod worker;
