//! What the engine reports to the UI.

use crate::{error::MediaError, tags::TagInfo};

/// Playback state as the UI sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Paused,
    Playing,
}

/// Which kinds of stream the opened file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StreamInfo {
    pub video: bool,
    pub audio: bool,
}

/// Messages from the engine, delivered on the GTK main thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    State(PlayState),
    /// Current position in nanoseconds.
    Position(u64),
    /// Total length in nanoseconds.
    Duration(u64),
    /// Tags found so far, merged.
    Tags(TagInfo),
    Streams(StreamInfo),
    /// End of the stream (the engine has already rewound, or restarted when looping).
    Eos,
    Error(MediaError),
}
