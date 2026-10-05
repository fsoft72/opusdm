//! Errors of the playback engine.

use thiserror::Error;

/// A failure to set up or run a playback pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MediaError {
    /// A GStreamer element or codec is not installed; `package` says what to install.
    #[error("missing GStreamer plugin: {name} (install {package})")]
    MissingPlugin { name: String, package: String },
    /// The file or stream cannot be played (unknown or damaged format).
    #[error("cannot play {0}")]
    Unsupported(String),
    /// Any other pipeline failure.
    #[error("{message}{}", .debug.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())]
    Pipeline { message: String, debug: Option<String> },
}

/// Packages that provide the codecs a missing-plugin error most likely needs.
pub const CODEC_PACKAGES: &str = "gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad or gstreamer1.0-libav";

impl MediaError {
    /// A `Pipeline` error with no debug text.
    pub fn pipeline(message: impl Into<String>) -> Self {
        MediaError::Pipeline { message: message.into(), debug: None }
    }

    /// Map a GStreamer bus error to the matching variant.
    pub(crate) fn from_gst(err: &gstreamer::glib::Error, debug: Option<String>, uri: &str) -> Self {
        use gstreamer::{CoreError, StreamError};
        if err.matches(CoreError::MissingPlugin) || err.matches(StreamError::CodecNotFound) {
            return MediaError::MissingPlugin { name: err.message().to_string(), package: CODEC_PACKAGES.to_string() };
        }
        let unsupported = [StreamError::TypeNotFound, StreamError::WrongType, StreamError::Demux, StreamError::Decode, StreamError::Format]
            .into_iter()
            .any(|k| err.matches(k));
        if unsupported { return MediaError::Unsupported(uri.to_string()); }
        MediaError::Pipeline { message: err.message().to_string(), debug }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_plugin_names_the_package() {
        let e = MediaError::MissingPlugin { name: "gtk4paintablesink".into(), package: "gstreamer1.0-gtk4".into() };
        assert_eq!(e.to_string(), "missing GStreamer plugin: gtk4paintablesink (install gstreamer1.0-gtk4)");
    }

    #[test]
    fn pipeline_shows_the_debug_text_when_present() {
        assert_eq!(MediaError::pipeline("boom").to_string(), "boom");
        let e = MediaError::Pipeline { message: "boom".into(), debug: Some("x.c:1".into()) };
        assert_eq!(e.to_string(), "boom (x.c:1)");
    }
}
