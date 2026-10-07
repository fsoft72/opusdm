//! opusdm-audio: volume, mute and default device of the sound server, with no GTK.
//!
//! A pure model, volume maths, monitor filter and device diff, a `Backend`
//! trait (real `PulseBackend` over libpulse, `FakeBackend` for tests) and a
//! watcher that turns server events into `AudioEvent`s.

pub mod backend;
pub mod diff;
pub mod error;
pub mod filter;
pub mod model;
pub mod volume;

pub use backend::{Backend, FakeBackend};
pub use diff::{diff, AudioEvent};
pub use error::AudioError;
pub use model::{AudioDevice, Kind};
