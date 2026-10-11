//! opusdm-power: the battery through UPower and the power profiles through
//! power-profiles-daemon, with no GTK.
//!
//! A pure model and parser, a `Backend` trait (real `UPowerBackend` over the
//! system bus, `FakeBackend` for tests) and a watcher that turns the signals
//! of both services into state snapshots.

pub mod backend;
pub mod error;
pub mod model;
pub mod parse;
pub mod upower;
pub mod watch;

pub use backend::{Backend, FakeBackend};
pub use error::PowerError;
pub use model::{Battery, ChargeState, PowerState, Profile};
pub use upower::UPowerBackend;
pub use watch::WatchMsg;
