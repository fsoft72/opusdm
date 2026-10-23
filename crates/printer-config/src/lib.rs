//! Library of `opusdm-printer-manager`: the worker that runs the CUPS calls,
//! the editing session and the add printer wizard (tested without a
//! display), and the GTK window (`window`), which the Printers program and
//! the Advanced tab of `opusdm-settings` share. See
//! `docs/superpowers/specs/2026-10-23-printer-manager-design.md`.

#[cfg(test)]
mod gtk_test;
mod pages;
pub mod session;
pub mod window;
pub mod wizard;
pub mod worker;
