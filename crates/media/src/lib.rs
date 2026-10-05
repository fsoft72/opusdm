//! `opusdm-media`: the playback engine shared by `opusdm-player` and, later, the
//! video desktop background. GStreamer only: no window, no OpusDM dependency.
//! See `docs/superpowers/specs/2026-10-05-player-design.md`.

mod engine;
mod error;
mod event;
mod tags;
mod uri;

pub use engine::{Engine, EngineOptions, VideoSink};
pub use error::{MediaError, CODEC_PACKAGES};
pub use event::{Event, PlayState, StreamInfo};
pub use tags::TagInfo;
pub use uri::to_uri;
