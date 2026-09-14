//! Keyboard navigation and coordinate stepping for `IconField`.

use super::IconField;
use crate::types::LayoutMode;

impl IconField {
    /// Move the keyboard cursor by (`dx`, `dy`) in {-1, 0, 1}. `extend` grows
    /// the selection from the anchor; `move_only` leaves the selection alone.
    pub(crate) fn move_cursor(&self, dx: i32, dy: i32, extend: bool, move_only: bool) {
        let n = self.inner.layout.entries.borrow().len();
        if n == 0 {
            return;
        }
        let cur = self.inner.selection.cursor.get().unwrap_or(0);
        let next = if matches!(self.inner.layout.mode.get(), LayoutMode::Grid) {
            self.grid_step(cur, dx, dy, n)
        } else {
            self.free_step(cur, dx, dy)
        };
        let Some(next) = next else {
            return;
        };
        if extend {
            self.select_range_to(next);
        } else if move_only {
            self.set_cursor(next);
        } else {
            self.select_exclusive(next);
        }
    }

    pub(super) fn grid_step(&self, cur: usize, dx: i32, dy: i32, n: usize) -> Option<usize> {
        let cols = self.current_columns().max(1);
        let mut idx = cur as i64;
        idx += dx as i64;
        idx += dy as i64 * cols as i64;
        if idx < 0 || idx as usize >= n {
            return None;
        }
        Some(idx as usize)
    }

    pub(super) fn free_step(&self, cur: usize, dx: i32, dy: i32) -> Option<usize> {
        let rects = self.inner.layout.item_rects.borrow();
        let &(fx, fy, _, _) = rects.get(cur)?;
        let cands: Vec<(usize, (f64, f64))> =
            rects.iter().enumerate().map(|(i, &(x, y, _, _))| (i, (x, y))).collect();
        crate::arrange::nearest_in_direction((fx, fy), &cands, dx, dy)
    }

    pub(super) fn current_columns(&self) -> usize {
        let rects = self.inner.layout.item_rects.borrow();
        if rects.len() < 2 {
            return 1;
        }
        let y0 = rects[0].1;
        let mut cols = 1;
        for &(_, y, _, _) in rects.iter().skip(1) {
            if (y - y0).abs() < 1.0 {
                cols += 1;
            } else {
                break;
            }
        }
        cols
    }

    /// The item index under `Fixed`-space point (`x`, `y`), if any.
    pub(super) fn item_at(&self, x: f64, y: f64) -> Option<usize> {
        // Scan back to front: when rectangles overlap (dragged-together icons
        // in Free mode), the last-placed one is drawn on top and wins the hit.
        for (i, &(ix, iy, iw, ih)) in self.inner.layout.item_rects.borrow().iter().enumerate().rev() {
            if x >= ix && x < ix + iw && y >= iy && y < iy + ih {
                return Some(i);
            }
        }
        None
    }
}
