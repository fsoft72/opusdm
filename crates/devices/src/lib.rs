//! opusdm-devices: removable drives through udisks2, with no GTK.
//!
//! A pure model, filter and parser for the udisks2 object tree, a `Backend`
//! trait (real `UdisksBackend` over the system bus, `FakeBackend` for tests),
//! the eject sequence and a watcher that turns udisks2 signals into events.

pub mod backend;
pub mod diff;
pub mod error;
pub mod filter;
pub mod model;
pub mod ops;
pub mod parse;
pub mod udisks;
pub mod watch;

