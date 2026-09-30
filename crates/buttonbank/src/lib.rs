//! Button bank windows and their editor. The crate knows nothing about the
//! hub: callers pass callbacks. See
//! `docs/superpowers/specs/2026-09-30-button-banks-design.md`.

pub mod editor;
pub mod grid;
pub mod window;
pub mod x11;
