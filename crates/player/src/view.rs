//! Which view the window shows for the opened file.

use opusdm_media::StreamInfo;

/// The two views of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Video,
    Audio,
}

/// Video view when the file has a video stream, otherwise the audio view (cover
/// and tags). Before the streams are known the audio view is shown.
pub fn choose(streams: Option<StreamInfo>) -> ViewKind {
    match streams {
        Some(s) if s.video => ViewKind::Video,
        _ => ViewKind::Audio,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_when_there_is_a_video_stream() {
        assert_eq!(choose(Some(StreamInfo { video: true, audio: true })), ViewKind::Video);
        assert_eq!(choose(Some(StreamInfo { video: true, audio: false })), ViewKind::Video);
    }

    #[test]
    fn audio_otherwise() {
        assert_eq!(choose(Some(StreamInfo { video: false, audio: true })), ViewKind::Audio);
        assert_eq!(choose(None), ViewKind::Audio);
    }
}
