//! opusdm-network: Wi-Fi and Ethernet through NetworkManager, with no GTK.
//!
//! A pure model and parser, a `Backend` trait (real `NmBackend` over the
//! system bus, `FakeBackend` for tests) and a watcher that turns
//! NetworkManager signals into state snapshots.

pub mod agent;
pub mod backend;
pub mod error;
pub mod ipconfig;
pub mod keystore;
pub mod model;
pub mod nm;
pub mod parse;
pub mod profile;
pub mod settings;
pub mod watch;
pub mod wgkey;

pub use backend::{Backend, FakeBackend};
pub use error::NetError;
pub use model::{AccessPoint, NetState, Security};
pub use nm::NmBackend;
pub use watch::WatchMsg;
