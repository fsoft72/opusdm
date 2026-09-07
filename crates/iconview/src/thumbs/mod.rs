//! Preview thumbnails: freedesktop-cached, generated off the main thread.

pub mod cache;
pub mod generate;
pub mod png_text;

/// What kind of preview a file gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbKind {
    Image,
    Pdf,
    Video,
}
