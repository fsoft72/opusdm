//! CUPS access for OpusDM without GTK: a small IPP client over the CUPS
//! socket for the reads, the CUPS command line tools for the changes, a
//! watcher of the print queue and the state of the panel printer icon. See
//! `docs/superpowers/specs/2026-10-23-printer-manager-design.md`.

pub mod ipp;
pub mod model;
pub mod parse;
pub mod transport;
pub mod backend;
pub mod cups;
pub mod watch;
pub mod tray;
