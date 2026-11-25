//! Geometry of the server side frame of a decorated window: the insets
//! between the frame and the client, the title bar layout (buttons and
//! title text) and the hit test of a point on the frame.
//!
//! Pure functions on plain rectangles, with no Smithay types, so they are
//! unit tested without a display. The frame is the rectangle the window
//! manager places (cascade, maximize, snap, `window/set-geometry`) and the
//! IPC / foreign-toplevel clients see; the client sits inside it, moved by
//! the insets.

// The title bar layout and the hit test are used by the title bar input and
// drawing (stage 3 Task 7); only the insets are used by the placement now.
#![allow(dead_code)]

use crate::decor::theme::DecorTheme;
use crate::geometry::{Edges, Rect};
use crate::state::LOG_TAG;

/// Height of the title bar of a decorated window, in pixels.
pub const TITLE_HEIGHT: i32 = 24;

/// Width and height of a title bar button, in pixels.
pub const TITLE_BUTTON_SIZE: i32 = 16;

/// Space between two title bar buttons, in pixels.
pub const TITLE_BUTTON_SPACING: i32 = 2;

/// Space between the title bar edges and the outer buttons: the same as
/// above and below a button, so a button sits in a square of the bar.
pub const TITLE_BUTTON_MARGIN: i32 = (TITLE_HEIGHT - TITLE_BUTTON_SIZE) / 2;

/// Space taken by the frame on each side of the client, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Insets {
    /// Top border plus the title bar.
    pub top: i32,
    /// Left border.
    pub left: i32,
    /// Right border.
    pub right: i32,
    /// Bottom border.
    pub bottom: i32,
}

/// A button of the title bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleButton {
    /// Closes the window (`close`).
    Close,
    /// Minimizes the window (`iconify`).
    Iconify,
    /// Toggles maximize (`max`).
    Maximize,
}

/// What a point of a frame is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameZone {
    /// The title bar outside the buttons (move, double click).
    Title,
    /// A button of the title bar.
    Button(TitleButton),
    /// The border: the edges a drag there resizes.
    Border(Edges),
    /// The client area (or the whole window when it has no frame).
    Client,
}

/// The buttons of the title bar, labwc syntax: the names before `:` on the
/// left (left to right), the names after it on the right (left to right).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TitleLayout {
    /// Buttons on the left of the title, left to right.
    pub left: Vec<TitleButton>,
    /// Buttons on the right of the title, left to right.
    pub right: Vec<TitleButton>,
}

impl TitleLayout {
    /// The layout written `s` (`compositor.json` `title_layout`, default
    /// `"close,iconify,max:"`): comma separated names `close`, `iconify`
    /// and `max`, left of `:` on the left, right of it on the right (no `:`:
    /// all on the left). Empty names are ignored; an unknown name or a
    /// button given twice is skipped with an `err_log!`.
    pub fn parse(s: &str) -> TitleLayout {
        let (left, right) = s.split_once(':').unwrap_or((s, ""));
        let mut layout = TitleLayout::default();
        for (side, names) in [(false, left), (true, right)] {
            for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                let Some(button) = _button_named(name) else {
                    opusdm_ipc::err_log!(LOG_TAG, "title layout {s:?}: unknown button {name:?}, skipped");
                    continue;
                };
                if layout.left.contains(&button) || layout.right.contains(&button) {
                    opusdm_ipc::err_log!(LOG_TAG, "title layout {s:?}: button {name:?} given twice, skipped");
                    continue;
                }
                if side { layout.right.push(button) } else { layout.left.push(button) }
            }
        }
        layout
    }
}

/// The button of the title layout name `name`; `None` for an unknown name.
fn _button_named(name: &str) -> Option<TitleButton> {
    match name {
        "close" => Some(TitleButton::Close),
        "iconify" => Some(TitleButton::Iconify),
        "max" => Some(TitleButton::Maximize),
        _ => None,
    }
}

/// The insets of a window: `TITLE_HEIGHT` plus the border width of `theme`
/// on top and the border width on the other sides when it is `decorated`;
/// all zero when it is not decorated or is `fullscreen`.
pub fn insets(decorated: bool, fullscreen: bool, theme: &DecorTheme) -> Insets {
    if !decorated || fullscreen { return Insets::default(); }
    let border = i32::try_from(theme.border_width).unwrap_or(i32::MAX);
    Insets { top: TITLE_HEIGHT.saturating_add(border), left: border, right: border, bottom: border }
}

/// The client rectangle inside the `frame` with `insets` (its size may be
/// zero or negative when the frame is smaller than the insets).
pub fn client_rect(frame: Rect, insets: Insets) -> Rect {
    Rect {
        x: frame.x.saturating_add(insets.left),
        y: frame.y.saturating_add(insets.top),
        w: frame.w.saturating_sub(insets.left).saturating_sub(insets.right),
        h: frame.h.saturating_sub(insets.top).saturating_sub(insets.bottom),
    }
}

/// The frame rectangle around the `client` with `insets`.
pub fn frame_rect(client: Rect, insets: Insets) -> Rect {
    Rect {
        x: client.x.saturating_sub(insets.left),
        y: client.y.saturating_sub(insets.top),
        w: client.w.saturating_add(insets.left).saturating_add(insets.right),
        h: client.h.saturating_add(insets.top).saturating_add(insets.bottom),
    }
}

/// The title bar of `frame`: between the side borders, under the top
/// border, `TITLE_HEIGHT` high. `None` when the insets leave no title bar
/// (a window without a frame).
pub fn title_bar_rect(frame: Rect, insets: Insets) -> Option<Rect> {
    let top_border = insets.top.saturating_sub(TITLE_HEIGHT);
    if top_border < 0 { return None; }
    let w = frame.w.saturating_sub(insets.left).saturating_sub(insets.right);
    Some(Rect { x: frame.x.saturating_add(insets.left), y: frame.y.saturating_add(top_border), w, h: TITLE_HEIGHT })
}

/// The rectangles of the title bar buttons of `frame` in `layout`: the left
/// buttons from the left end of the bar, the right ones ending at its right
/// end, `TITLE_BUTTON_MARGIN` from the ends and `TITLE_BUTTON_SPACING`
/// apart, centered on the bar height. Empty without a title bar.
pub fn button_rects(frame: Rect, insets: Insets, layout: &TitleLayout) -> Vec<(TitleButton, Rect)> {
    let Some(bar) = title_bar_rect(frame, insets) else { return Vec::new() };
    let y = bar.y + TITLE_BUTTON_MARGIN;
    let step = TITLE_BUTTON_SIZE + TITLE_BUTTON_SPACING;
    let square = |x: i32| Rect { x, y, w: TITLE_BUTTON_SIZE, h: TITLE_BUTTON_SIZE };
    let mut rects = Vec::new();
    let mut x = bar.x + TITLE_BUTTON_MARGIN;
    for button in &layout.left {
        rects.push((*button, square(x)));
        x += step;
    }
    let mut x = bar.x + bar.w - TITLE_BUTTON_MARGIN - TITLE_BUTTON_SIZE;
    for button in layout.right.iter().rev() {
        rects.push((*button, square(x)));
        x -= step;
    }
    rects
}

/// The rectangle of the title text of `frame` in `layout`: the title bar
/// between the left and the right buttons (`TITLE_BUTTON_MARGIN` from
/// them); the text is centered in it. `None` without a title bar; the
/// width is never negative.
pub fn title_text_rect(frame: Rect, insets: Insets, layout: &TitleLayout) -> Option<Rect> {
    let bar = title_bar_rect(frame, insets)?;
    let side = |count: usize| -> i32 {
        let count = i32::try_from(count).unwrap_or(i32::MAX);
        if count == 0 { return TITLE_BUTTON_MARGIN; }
        TITLE_BUTTON_MARGIN.saturating_mul(2).saturating_add(count.saturating_mul(TITLE_BUTTON_SIZE + TITLE_BUTTON_SPACING)) - TITLE_BUTTON_SPACING
    };
    let left = side(layout.left.len());
    let right = side(layout.right.len());
    let w = bar.w.saturating_sub(left).saturating_sub(right).max(0);
    Some(Rect { x: bar.x + left, y: bar.y, w, h: bar.h })
}

/// What `point` (in the coordinates of `frame`) is on, for a frame with
/// `insets` and the title bar `layout`: the border (with the edges a drag
/// there resizes; a point outside the frame counts as the border it is
/// past), a title bar button, the rest of the title bar, or the client.
/// A window without a frame (zero insets) is all `Client`.
pub fn frame_zone(frame: Rect, point: (i32, i32), insets: Insets, layout: &TitleLayout) -> FrameZone {
    if insets == Insets::default() { return FrameZone::Client; }
    let (x, y) = point;
    let top_border = insets.top.saturating_sub(TITLE_HEIGHT).max(0);
    let edges = Edges {
        top: y < frame.y.saturating_add(top_border),
        bottom: y >= frame.y.saturating_add(frame.h).saturating_sub(insets.bottom),
        left: x < frame.x.saturating_add(insets.left),
        right: x >= frame.x.saturating_add(frame.w).saturating_sub(insets.right),
    };
    if edges != Edges::default() { return FrameZone::Border(edges); }
    if y >= frame.y.saturating_add(insets.top) { return FrameZone::Client; }
    let on = button_rects(frame, insets, layout).into_iter().find(|(_, r)| _contains(*r, point));
    match on {
        Some((button, _)) => FrameZone::Button(button),
        None => FrameZone::Title,
    }
}

/// True when `point` is inside `rect` (left / top edges in, right / bottom out).
fn _contains(rect: Rect, point: (i32, i32)) -> bool {
    let (x, y) = point;
    x >= rect.x && y >= rect.y && x < rect.x.saturating_add(rect.w) && y < rect.y.saturating_add(rect.h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME: DecorTheme = DecorTheme { border_width: 4 };

    /// A 400x300 frame at (100, 50) with the default theme insets.
    const FRAME: Rect = Rect { x: 100, y: 50, w: 400, h: 300 };

    /// The default title layout, `"close,iconify,max:"`.
    fn default_layout() -> TitleLayout {
        TitleLayout::parse("close,iconify,max:")
    }

    #[test]
    fn a_decorated_window_has_the_title_and_the_border_as_insets() {
        assert_eq!(insets(true, false, &THEME), Insets { top: 28, left: 4, right: 4, bottom: 4 });
    }

    #[test]
    fn an_undecorated_or_fullscreen_window_has_no_insets() {
        assert_eq!(insets(false, false, &THEME), Insets::default());
        assert_eq!(insets(true, true, &THEME), Insets::default());
        assert_eq!(insets(false, true, &THEME), Insets::default());
    }

    #[test]
    fn client_and_frame_rects_are_inverse() {
        let i = insets(true, false, &THEME);
        let client = client_rect(FRAME, i);
        assert_eq!(client, Rect { x: 104, y: 78, w: 392, h: 268 });
        assert_eq!(frame_rect(client, i), FRAME);
        assert_eq!(client_rect(FRAME, Insets::default()), FRAME);
    }

    #[test]
    fn the_default_layout_puts_every_button_on_the_left() {
        let layout = default_layout();
        assert_eq!(layout.left, vec![TitleButton::Close, TitleButton::Iconify, TitleButton::Maximize]);
        assert!(layout.right.is_empty());
    }

    #[test]
    fn a_layout_splits_at_the_colon_and_skips_unknown_and_repeated_names() {
        let layout = TitleLayout::parse("iconify, shade :max,close,iconify");
        assert_eq!(layout.left, vec![TitleButton::Iconify]);
        assert_eq!(layout.right, vec![TitleButton::Maximize, TitleButton::Close]);
        assert_eq!(TitleLayout::parse(":close").left, vec![]);
        assert_eq!(TitleLayout::parse(":close").right, vec![TitleButton::Close]);
        assert_eq!(TitleLayout::parse("close").left, vec![TitleButton::Close]);
        assert_eq!(TitleLayout::parse(""), TitleLayout::default());
    }

    #[test]
    fn the_buttons_are_16_pixels_2_apart_and_centered_on_the_bar() {
        let i = insets(true, false, &THEME);
        let rects = button_rects(FRAME, i, &default_layout());
        // Bar at (104, 54), 392x24: buttons 4 px from its left end and its top.
        assert_eq!(rects, vec![
            (TitleButton::Close, Rect { x: 108, y: 58, w: 16, h: 16 }),
            (TitleButton::Iconify, Rect { x: 126, y: 58, w: 16, h: 16 }),
            (TitleButton::Maximize, Rect { x: 144, y: 58, w: 16, h: 16 }),
        ]);
        let right = button_rects(FRAME, i, &TitleLayout::parse(":iconify,close"));
        // Bar right end at 496: close ends 4 px before it, iconify left of close.
        assert_eq!(right, vec![
            (TitleButton::Close, Rect { x: 476, y: 58, w: 16, h: 16 }),
            (TitleButton::Iconify, Rect { x: 458, y: 58, w: 16, h: 16 }),
        ]);
    }

    #[test]
    fn the_title_text_sits_between_the_buttons() {
        let i = insets(true, false, &THEME);
        // Left: 4 + 3 * 18 - 2 + 4 = 60 from the bar start; right: 4 from its end.
        assert_eq!(title_text_rect(FRAME, i, &default_layout()), Some(Rect { x: 164, y: 54, w: 328, h: 24 }));
        assert_eq!(title_text_rect(FRAME, Insets::default(), &default_layout()), None);
    }

    #[test]
    fn a_point_on_the_close_button_of_a_400x300_frame() {
        let i = insets(true, false, &THEME);
        assert_eq!(frame_zone(FRAME, (110, 60), i, &default_layout()), FrameZone::Button(TitleButton::Close));
        assert_eq!(frame_zone(FRAME, (150, 70), i, &default_layout()), FrameZone::Button(TitleButton::Maximize));
    }

    #[test]
    fn a_point_on_the_title_bar_outside_the_buttons_is_the_title() {
        let i = insets(true, false, &THEME);
        assert_eq!(frame_zone(FRAME, (300, 60), i, &default_layout()), FrameZone::Title);
        // Between two buttons and under them, still on the bar.
        assert_eq!(frame_zone(FRAME, (125, 60), i, &default_layout()), FrameZone::Title);
        assert_eq!(frame_zone(FRAME, (110, 76), i, &default_layout()), FrameZone::Title);
    }

    #[test]
    fn a_point_on_the_right_border_and_the_corners() {
        let i = insets(true, false, &THEME);
        let right = Edges { right: true, ..Edges::default() };
        assert_eq!(frame_zone(FRAME, (498, 200), i, &default_layout()), FrameZone::Border(right));
        let top_left = Edges { top: true, left: true, ..Edges::default() };
        assert_eq!(frame_zone(FRAME, (101, 51), i, &default_layout()), FrameZone::Border(top_left));
        let bottom_right = Edges { bottom: true, right: true, ..Edges::default() };
        assert_eq!(frame_zone(FRAME, (499, 349), i, &default_layout()), FrameZone::Border(bottom_right));
        assert_eq!(frame_zone(FRAME, (300, 51), i, &default_layout()), FrameZone::Border(Edges { top: true, ..Edges::default() }));
    }

    #[test]
    fn a_point_inside_the_client_is_the_client() {
        let i = insets(true, false, &THEME);
        assert_eq!(frame_zone(FRAME, (300, 200), i, &default_layout()), FrameZone::Client);
        assert_eq!(frame_zone(FRAME, (104, 78), i, &default_layout()), FrameZone::Client);
    }

    #[test]
    fn a_window_without_a_frame_is_all_client() {
        assert_eq!(frame_zone(FRAME, (110, 60), Insets::default(), &default_layout()), FrameZone::Client);
        assert_eq!(button_rects(FRAME, Insets::default(), &default_layout()), vec![]);
    }
}
