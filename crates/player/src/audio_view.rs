//! The view of an audio only file: cover (or an icon), title, artist and album.

use gtk4 as gtk;
use gtk::{gdk, glib};
use gtk::prelude::*;
use opusdm_media::TagInfo;

/// Size of the cover in pixels.
const COVER_SIZE: i32 = 280;

/// The widgets that show the tags.
pub struct AudioView {
    pub root: gtk::Box,
    cover: gtk::Picture,
    icon: gtk::Image,
    title: gtk::Label,
    artist: gtk::Label,
    album: gtk::Label,
}

impl AudioView {
    /// An empty view; `fallback_title` (the file name) shows until the tags arrive.
    pub fn new(fallback_title: &str) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Center);
        let cover = gtk::Picture::new();
        cover.set_size_request(COVER_SIZE, COVER_SIZE);
        cover.set_content_fit(gtk::ContentFit::Contain);
        cover.set_visible(false);
        let icon = gtk::Image::from_icon_name("audio-x-generic-symbolic");
        icon.set_pixel_size(128);
        let title = gtk::Label::new(Some(fallback_title));
        title.add_css_class("title-2");
        title.set_wrap(true);
        let artist = gtk::Label::new(None);
        let album = gtk::Label::new(None);
        album.add_css_class("dim-label");
        for w in [cover.upcast_ref::<gtk::Widget>(), icon.upcast_ref(), title.upcast_ref(), artist.upcast_ref(), album.upcast_ref()] {
            root.append(w);
        }
        AudioView { root, cover, icon, title, artist, album }
    }

    /// Show the tags that are known; the cover replaces the icon when it decodes.
    pub fn set_tags(&self, tags: &TagInfo) {
        if let Some(t) = &tags.title { self.title.set_text(t); }
        self.artist.set_text(tags.artist.as_deref().unwrap_or(""));
        self.album.set_text(tags.album.as_deref().unwrap_or(""));
        let texture = tags.cover.as_ref().and_then(|bytes| gdk::Texture::from_bytes(&glib::Bytes::from(bytes)).ok());
        self.cover.set_paintable(texture.as_ref());
        self.cover.set_visible(texture.is_some());
        self.icon.set_visible(texture.is_none());
    }
}
