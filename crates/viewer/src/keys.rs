//! Key bindings: what a key does depends on whether a picture or a video shows.

use crate::kind::MediaKind;
use gtk4::gdk::Key;

/// Everything a key can ask the window to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ZoomIn,
    ZoomOut,
    Fit,
    Actual,
    Next,
    Prev,
    SeekBack,
    SeekForward,
    TogglePlay,
    RotateCw,
    RotateCcw,
    Slideshow,
    Fullscreen,
    Delete,
    /// Leave full screen, or close the window when not in full screen.
    Escape,
    Quit,
}

/// The action of `key` while `kind` is on screen; `None` when the key is unused.
pub fn map(key: Key, kind: MediaKind) -> Option<Action> {
    let video = kind == MediaKind::Video;
    let action = match key {
        Key::plus | Key::KP_Add | Key::equal if !video => Action::ZoomIn,
        Key::minus | Key::KP_Subtract if !video => Action::ZoomOut,
        Key::_0 | Key::KP_0 if !video => Action::Fit,
        Key::_1 | Key::KP_1 if !video => Action::Actual,
        Key::r if !video => Action::RotateCw,
        Key::R if !video => Action::RotateCcw,
        Key::Left | Key::KP_Left => if video { Action::SeekBack } else { Action::Prev },
        Key::Right | Key::KP_Right => if video { Action::SeekForward } else { Action::Next },
        Key::space => if video { Action::TogglePlay } else { Action::Next },
        Key::n | Key::N => Action::Next,
        Key::p | Key::P => Action::Prev,
        Key::s | Key::S => Action::Slideshow,
        Key::f | Key::F => Action::Fullscreen,
        Key::Delete | Key::KP_Delete => Action::Delete,
        Key::Escape => Action::Escape,
        Key::q | Key::Q => Action::Quit,
        _ => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use MediaKind::{Image, Video};

    #[test]
    fn arrows_move_on_pictures_and_seek_on_videos() {
        assert_eq!(map(Key::Left, Image), Some(Action::Prev));
        assert_eq!(map(Key::Right, Image), Some(Action::Next));
        assert_eq!(map(Key::Left, Video), Some(Action::SeekBack));
        assert_eq!(map(Key::Right, Video), Some(Action::SeekForward));
    }

    #[test]
    fn space_is_next_on_pictures_and_play_on_videos() {
        assert_eq!(map(Key::space, Image), Some(Action::Next));
        assert_eq!(map(Key::space, Video), Some(Action::TogglePlay));
    }

    #[test]
    fn n_and_p_move_for_both_kinds() {
        for kind in [Image, Video] {
            assert_eq!(map(Key::n, kind), Some(Action::Next));
            assert_eq!(map(Key::p, kind), Some(Action::Prev));
        }
    }

    #[test]
    fn zoom_and_rotation_are_for_pictures_only() {
        assert_eq!(map(Key::plus, Image), Some(Action::ZoomIn));
        assert_eq!(map(Key::KP_Add, Image), Some(Action::ZoomIn));
        assert_eq!(map(Key::equal, Image), Some(Action::ZoomIn));
        assert_eq!(map(Key::minus, Image), Some(Action::ZoomOut));
        assert_eq!(map(Key::KP_Subtract, Image), Some(Action::ZoomOut));
        assert_eq!(map(Key::_0, Image), Some(Action::Fit));
        assert_eq!(map(Key::_1, Image), Some(Action::Actual));
        assert_eq!(map(Key::r, Image), Some(Action::RotateCw));
        assert_eq!(map(Key::R, Image), Some(Action::RotateCcw));
        for key in [Key::plus, Key::minus, Key::_0, Key::_1, Key::r, Key::R] {
            assert_eq!(map(key, Video), None);
        }
    }

    #[test]
    fn common_keys() {
        for kind in [Image, Video] {
            assert_eq!(map(Key::s, kind), Some(Action::Slideshow));
            assert_eq!(map(Key::f, kind), Some(Action::Fullscreen));
            assert_eq!(map(Key::Delete, kind), Some(Action::Delete));
            assert_eq!(map(Key::Escape, kind), Some(Action::Escape));
            assert_eq!(map(Key::q, kind), Some(Action::Quit));
            assert_eq!(map(Key::z, kind), None);
        }
    }
}
