//! opusdm-bluetooth: the Bluetooth adapter and devices through BlueZ, with
//! no GTK.
//!
//! A pure model and parser, a `Backend` trait (real `BluezBackend` over the
//! system bus, `FakeBackend` for tests), a watcher that turns BlueZ signals
//! into state snapshots, and the pairing agent that hands BlueZ requests to
//! the GTK side. See `docs/superpowers/specs/2026-10-23-bluetooth-design.md`.

pub mod agent;
pub mod backend;
pub mod bluez;
pub mod error;
pub mod model;
pub mod parse;
pub mod rfkill;
pub mod watch;

pub use agent::{AgentEvent, AgentKind, AgentReply, AgentSetup};
pub use backend::{Backend, FakeBackend, Request};
pub use bluez::BluezBackend;
pub use error::BtError;
pub use model::{Adapter, BtState, Device, DeviceKind};
pub use rfkill::Rfkill;
pub use watch::WatchMsg;
