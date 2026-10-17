//! Preview thumbnails: freedesktop-cached, generated off the main thread.

pub mod cache;
pub mod generate;
pub mod png_text;
mod pool;
mod tracker;

pub use pool::{ThumbPool, ThumbRequest, ThumbResult};
pub(crate) use tracker::ThumbTracker;

/// What kind of preview a file gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbKind {
    Image,
    Pdf,
    Video,
}

/// The thumbnail kind for a file extension, or `None` when not previewable.
/// The `IconField`'s single decision point.
pub fn wanted(ext: Option<&str>) -> Option<ThumbKind> {
    ThumbKind::from_ext(ext)
}
