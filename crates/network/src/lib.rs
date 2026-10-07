//! opusdm-network: Wi-Fi and Ethernet through NetworkManager, with no GTK.
//!
//! A pure model and parser, a `Backend` trait (real `NmBackend` over the
//! system bus, `FakeBackend` for tests) and a watcher that turns
//! NetworkManager signals into state snapshots.

pub mod backend;
pub mod error;
pub mod model;
pub mod nm;
pub mod parse;
pub mod watch;

pub use backend::{Backend, FakeBackend};
pub use error::NetError;
pub use model::{AccessPoint, NetState, Security};
pub use nm::NmBackend;
pub use watch::WatchMsg;
