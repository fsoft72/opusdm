//! Tags of a media file (title, artist, album, cover) in a plain struct.

use gstreamer as gst;

/// The tags the player shows. Empty fields are unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Encoded cover image (JPEG, PNG, ...), as stored in the file.
    pub cover: Option<Vec<u8>>,
}

impl TagInfo {
    /// Read the known tags out of a GStreamer tag list.
    pub fn from_taglist(list: &gst::TagListRef) -> Self {
        TagInfo {
            title: _text(list.get::<gst::tags::Title>().map(|v| v.get().to_string())),
            artist: _text(list.get::<gst::tags::Artist>().map(|v| v.get().to_string())),
            album: _text(list.get::<gst::tags::Album>().map(|v| v.get().to_string())),
            cover: _cover(list),
        }
    }

    /// Fill the fields that are still empty with those of `other`.
    pub fn merge(&mut self, other: TagInfo) {
        if self.title.is_none() { self.title = other.title; }
        if self.artist.is_none() { self.artist = other.artist; }
        if self.album.is_none() { self.album = other.album; }
        if self.cover.is_none() { self.cover = other.cover; }
    }
}

/// `value` unless it is empty.
fn _text(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

/// Bytes of the first image tag (cover, else preview image).
fn _cover(list: &gst::TagListRef) -> Option<Vec<u8>> {
    let sample = list.get::<gst::tags::Image>().or_else(|| list.get::<gst::tags::PreviewImage>())?;
    let sample = sample.get();
    let buffer = sample.buffer()?;
    let map = buffer.map_readable().ok()?;
    Some(map.as_slice().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_tags() {
        if gst::init().is_err() { return; }
        let mut list = gst::TagList::new();
        {
            let l = list.get_mut().unwrap();
            l.add::<gst::tags::Title>(&"Song", gst::TagMergeMode::Append);
            l.add::<gst::tags::Artist>(&"Band", gst::TagMergeMode::Append);
        }
        let info = TagInfo::from_taglist(&list);
        assert_eq!(info.title.as_deref(), Some("Song"));
        assert_eq!(info.artist.as_deref(), Some("Band"));
        assert_eq!(info.album, None);
        assert_eq!(info.cover, None);
    }

    #[test]
    fn merge_keeps_existing_fields() {
        let mut a = TagInfo { title: Some("A".into()), ..Default::default() };
        a.merge(TagInfo { title: Some("B".into()), artist: Some("X".into()), ..Default::default() });
        assert_eq!(a.title.as_deref(), Some("A"));
        assert_eq!(a.artist.as_deref(), Some("X"));
    }
}
