//! Key bindings: what a key does depends on whether a picture, a PDF or a comic, a video or a text shows.

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
    VolumeUp,
    VolumeDown,
    Mute,
    RotateCw,
    RotateCcw,
    /// PDF pages.
    NextPage,
    PrevPage,
    FirstPage,
    LastPage,
    /// Open or close the page panel of a PDF.
    TogglePages,
    /// Smooth (interpolated) or sharp pixels when zoomed.
    Interpolation,
    /// Show or hide the line numbers of a source text.
    LineNumbers,
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
    let source = kind == MediaKind::Source;
    let text = kind == MediaKind::Markdown || source;
    let pdf = kind.is_paged();
    let action = match key {
        Key::plus | Key::KP_Add | Key::equal if !video && !text => Action::ZoomIn,
        Key::minus | Key::KP_Subtract if !video && !text => Action::ZoomOut,
        Key::_0 | Key::KP_0 if !video && !text => Action::Fit,
        Key::_1 | Key::KP_1 if !video && !text => Action::Actual,
        Key::r if !video && !text => Action::RotateCw,
        Key::R if !video && !text => Action::RotateCcw,
        Key::period | Key::KP_Decimal if !video && !text => Action::Interpolation,
        Key::Left | Key::KP_Left | Key::Up | Key::KP_Up if pdf => Action::PrevPage,
        Key::Right | Key::KP_Right | Key::Down | Key::KP_Down if pdf => Action::NextPage,
        Key::Left | Key::KP_Left => if video { Action::SeekBack } else { Action::Prev },
        Key::Right | Key::KP_Right => if video { Action::SeekForward } else { Action::Next },
        Key::space => if video { Action::TogglePlay } else { Action::Next },
        Key::Up | Key::KP_Up if video => Action::VolumeUp,
        Key::Down | Key::KP_Down if video => Action::VolumeDown,
        Key::m | Key::M if video => Action::Mute,
        Key::Page_Down | Key::KP_Page_Down if pdf => Action::NextPage,
        Key::Page_Up | Key::KP_Page_Up if pdf => Action::PrevPage,
        Key::Home | Key::KP_Home if pdf => Action::FirstPage,
        Key::End | Key::KP_End if pdf => Action::LastPage,
        Key::F9 if pdf => Action::TogglePages,
        Key::l | Key::L if source => Action::LineNumbers,
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

/// Page index (0-based) a page action leads to in a document of `count` pages,
/// from page `current`; `None` when the page does not change.
pub fn page_after(action: Action, current: usize, count: usize) -> Option<usize> {
    if count == 0 { return None; }
    let last = count - 1;
    let target = match action {
        Action::NextPage => (current + 1).min(last),
        Action::PrevPage => current.saturating_sub(1),
        Action::FirstPage => 0,
        Action::LastPage => last,
        _ => return None,
    };
    (target != current).then_some(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use MediaKind::{Comic, Image, Markdown, Pdf, Source, Video};

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
        assert_eq!(map(Key::period, Image), Some(Action::Interpolation));
        for key in [Key::plus, Key::minus, Key::_0, Key::_1, Key::r, Key::R, Key::period] {
            assert_eq!(map(key, Video), None);
        }
    }

    #[test]
    fn a_markdown_text_moves_but_does_not_zoom_or_rotate() {
        assert_eq!(map(Key::Right, Markdown), Some(Action::Next));
        assert_eq!(map(Key::p, Markdown), Some(Action::Prev));
        assert_eq!(map(Key::f, Markdown), Some(Action::Fullscreen));
        for key in [Key::plus, Key::minus, Key::_0, Key::_1, Key::r, Key::R, Key::period, Key::Up, Key::Down, Key::Page_Down] {
            assert_eq!(map(key, Markdown), None);
        }
    }

    #[test]
    fn a_source_text_moves_like_markdown_and_toggles_line_numbers() {
        assert_eq!(map(Key::Right, Source), Some(Action::Next));
        assert_eq!(map(Key::space, Source), Some(Action::Next));
        assert_eq!(map(Key::p, Source), Some(Action::Prev));
        for key in [Key::plus, Key::minus, Key::_0, Key::_1, Key::r, Key::R, Key::period, Key::Up, Key::Down, Key::Page_Down] {
            assert_eq!(map(key, Source), None);
        }
        assert_eq!(map(Key::l, Source), Some(Action::LineNumbers));
        for kind in [Image, Video, Pdf, Markdown] {
            assert_eq!(map(Key::l, kind), None);
        }
    }

    #[test]
    fn volume_and_mute_are_for_videos_only() {
        assert_eq!(map(Key::Up, Video), Some(Action::VolumeUp));
        assert_eq!(map(Key::Down, Video), Some(Action::VolumeDown));
        assert_eq!(map(Key::m, Video), Some(Action::Mute));
        for key in [Key::Up, Key::Down, Key::m] {
            assert_eq!(map(key, Image), None);
        }
    }

    #[test]
    fn page_keys_are_for_pdf_only() {
        assert_eq!(map(Key::Page_Down, Pdf), Some(Action::NextPage));
        assert_eq!(map(Key::KP_Page_Down, Pdf), Some(Action::NextPage));
        assert_eq!(map(Key::Page_Up, Pdf), Some(Action::PrevPage));
        assert_eq!(map(Key::Home, Pdf), Some(Action::FirstPage));
        assert_eq!(map(Key::End, Pdf), Some(Action::LastPage));
        assert_eq!(map(Key::F9, Pdf), Some(Action::TogglePages));
        for kind in [Image, Video] {
            for key in [Key::Page_Down, Key::Page_Up, Key::Home, Key::End, Key::F9] {
                assert_eq!(map(key, kind), None);
            }
        }
    }

    #[test]
    fn arrows_turn_the_pages_of_a_pdf() {
        for key in [Key::Left, Key::KP_Left, Key::Up, Key::KP_Up] {
            assert_eq!(map(key, Pdf), Some(Action::PrevPage));
        }
        for key in [Key::Right, Key::KP_Right, Key::Down, Key::KP_Down] {
            assert_eq!(map(key, Pdf), Some(Action::NextPage));
        }
    }

    #[test]
    fn a_comic_has_the_page_keys_of_a_pdf() {
        for key in [Key::Page_Down, Key::Page_Up, Key::Home, Key::End, Key::F9, Key::Left, Key::Right, Key::Up, Key::Down, Key::space, Key::plus, Key::r] {
            assert_eq!(map(key, Comic), map(key, Pdf), "{key:?}");
        }
    }

    #[test]
    fn a_pdf_moves_zooms_and_rotates_like_a_picture() {
        assert_eq!(map(Key::space, Pdf), Some(Action::Next));
        assert_eq!(map(Key::n, Pdf), Some(Action::Next));
        assert_eq!(map(Key::p, Pdf), Some(Action::Prev));
        assert_eq!(map(Key::plus, Pdf), Some(Action::ZoomIn));
        assert_eq!(map(Key::r, Pdf), Some(Action::RotateCw));
    }

    #[test]
    fn page_after_stays_inside_the_document() {
        assert_eq!(page_after(Action::NextPage, 0, 3), Some(1));
        assert_eq!(page_after(Action::NextPage, 2, 3), None);
        assert_eq!(page_after(Action::PrevPage, 1, 3), Some(0));
        assert_eq!(page_after(Action::PrevPage, 0, 3), None);
        assert_eq!(page_after(Action::FirstPage, 2, 3), Some(0));
        assert_eq!(page_after(Action::LastPage, 0, 3), Some(2));
        assert_eq!(page_after(Action::LastPage, 2, 3), None);
        assert_eq!(page_after(Action::NextPage, 0, 0), None);
        assert_eq!(page_after(Action::ZoomIn, 0, 3), None);
    }

    #[test]
    fn common_keys() {
        for kind in [Image, Video, Pdf, Markdown, Source] {
            assert_eq!(map(Key::s, kind), Some(Action::Slideshow));
            assert_eq!(map(Key::f, kind), Some(Action::Fullscreen));
            assert_eq!(map(Key::Delete, kind), Some(Action::Delete));
            assert_eq!(map(Key::Escape, kind), Some(Action::Escape));
            assert_eq!(map(Key::q, kind), Some(Action::Quit));
            assert_eq!(map(Key::z, kind), None);
        }
    }
}
