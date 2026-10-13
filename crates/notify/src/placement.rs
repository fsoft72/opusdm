//! Where the popups go: a stack that grows from one corner of the usable
//! area of a monitor. Pure geometry.

use serde::{Deserialize, Serialize};

/// Width of a popup in pixels.
pub const POPUP_WIDTH: i32 = 360;
/// Distance between the stack and the edges of the area.
pub const MARGIN: i32 = 12;
/// Space between two popups.
pub const GAP: i32 = 8;
/// Popups on screen at once; the others wait their turn.
pub const MAX_VISIBLE: usize = 5;

/// A rectangle: x, y, width, height.
pub type Rect = (i32, i32, i32, i32);

/// The corner the stack starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    #[default]
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

impl Corner {
    /// Whether the stack hangs from the top edge.
    pub fn is_top(self) -> bool {
        matches!(self, Corner::TopRight | Corner::TopLeft)
    }

    /// Whether the stack leans on the right edge.
    pub fn is_right(self) -> bool {
        matches!(self, Corner::TopRight | Corner::BottomRight)
    }
}

/// Distance of each popup from the corner's horizontal edge, given their
/// heights, the one nearest the corner first.
pub fn offsets(heights: &[i32]) -> Vec<i32> {
    let mut at = MARGIN;
    heights
        .iter()
        .map(|h| {
            let mine = at;
            at += h + GAP;
            mine
        })
        .collect()
}

/// Top left corner of a `width` x `height` popup `offset` pixels from the
/// horizontal edge of `corner` inside `area`.
pub fn position(area: Rect, corner: Corner, width: i32, height: i32, offset: i32) -> (i32, i32) {
    let (ax, ay, aw, ah) = area;
    let x = if corner.is_right() { ax + aw - MARGIN - width } else { ax + MARGIN };
    let y = if corner.is_top() { ay + offset } else { ay + ah - offset - height };
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = (0, 30, 1920, 1050);

    #[test]
    fn offsets_stack_with_a_gap() {
        assert_eq!(offsets(&[100, 80, 60]), vec![MARGIN, MARGIN + 100 + GAP, MARGIN + 180 + 2 * GAP]);
        assert!(offsets(&[]).is_empty());
    }

    #[test]
    fn the_four_corners() {
        assert_eq!(position(AREA, Corner::TopRight, 360, 100, 12), (1920 - 12 - 360, 30 + 12));
        assert_eq!(position(AREA, Corner::TopLeft, 360, 100, 12), (12, 42));
        assert_eq!(position(AREA, Corner::BottomRight, 360, 100, 12), (1548, 30 + 1050 - 12 - 100));
        assert_eq!(position(AREA, Corner::BottomLeft, 360, 100, 120), (12, 1080 - 120 - 100));
    }

    #[test]
    fn a_second_monitor_uses_its_own_origin() {
        assert_eq!(position((1920, 0, 1280, 1024), Corner::TopLeft, 360, 50, 12), (1932, 12));
    }

    #[test]
    fn corners_parse_from_kebab_case() {
        let c: Corner = serde_json::from_str("\"bottom-right\"").unwrap();
        assert_eq!(c, Corner::BottomRight);
    }
}
