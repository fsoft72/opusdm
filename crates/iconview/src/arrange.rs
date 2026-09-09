//! Position math for `IconField`. Pure and GTK-free.
//!
//! `Arrange::place` maps a list of item sizes plus the viewport width to
//! absolute pixel positions inside the field's `Fixed`, and reports the total
//! virtual size so the surrounding `ScrolledWindow` knows its extent.

use std::collections::HashMap;

/// The measured size of one item.
#[derive(Debug, Clone, Copy)]
pub struct ItemBox {
    pub w: f64,
    pub h: f64,
}

/// Result of an arrangement pass.
#[derive(Debug, Clone)]
pub struct Placement {
    /// One (x, y) per input item, same order.
    pub positions: Vec<(f64, f64)>,
    pub virtual_w: f64,
    pub virtual_h: f64,
}

/// Strategy that positions items.
pub trait Arrange {
    /// Place `items` given a viewport `viewport_w` pixels wide.
    fn place(&self, items: &[ItemBox], viewport_w: f64) -> Placement;
}

/// Left-to-right, top-to-bottom flow into equal cells. Cell size is the max
/// item width/height plus the gap. Used by the Lister icon view.
#[derive(Debug, Clone, Copy)]
pub struct GridArrange {
    pub gap_x: f64,
    pub gap_y: f64,
}

impl GridArrange {
    /// A grid with the default 12px gaps.
    pub fn new() -> Self {
        Self { gap_x: 12.0, gap_y: 12.0 }
    }
}

impl Default for GridArrange {
    fn default() -> Self {
        Self::new()
    }
}

impl Arrange for GridArrange {
    fn place(&self, items: &[ItemBox], viewport_w: f64) -> Placement {
        if items.is_empty() {
            return Placement { positions: Vec::new(), virtual_w: 0.0, virtual_h: 0.0 };
        }
        let cell_w = items.iter().fold(0.0_f64, |m, b| m.max(b.w)) + self.gap_x;
        let cell_h = items.iter().fold(0.0_f64, |m, b| m.max(b.h)) + self.gap_y;
        let cols = ((viewport_w / cell_w).floor() as usize).max(1);
        let mut positions = Vec::with_capacity(items.len());
        for i in 0..items.len() {
            let col = i % cols;
            let row = i / cols;
            positions.push((col as f64 * cell_w, row as f64 * cell_h));
        }
        let rows = items.len().div_ceil(cols);
        Placement {
            positions,
            virtual_w: cols as f64 * cell_w,
            virtual_h: rows as f64 * cell_h,
        }
    }
}

/// Free placement: items with a known (x, y) sit there; the rest fill an
/// implicit grid from the top-left, skipping occupied cells. Used by the hub
/// desktop window.
///
/// The slot pitch is `cell.max(min_cell) + gap` on both axes and does **not**
/// depend on how wide the items measure: one long label must never space out
/// the whole desktop. Set `cell` below a label's width and neighbouring labels
/// overlap - that is the caller's explicit trade for a tighter grid.
#[derive(Debug, Clone)]
pub struct FreeArrange {
    pub known: HashMap<usize, (f64, f64)>,
    /// Room each icon gets before `gap`, in pixels (the `grid_spacing` setting).
    pub cell: f64,
    /// Dead space between cells on both axes, in pixels (the `grid_gap` setting).
    pub gap: f64,
    /// Hard floor for `cell`: the icon-box size, so glyphs never physically
    /// overlap and hit-testing stays unambiguous even at a tiny `cell`.
    pub min_cell: f64,
    pub margin: f64,
}

impl FreeArrange {
    /// Empty layout: every item auto-slots, 96px cells, 4px gap, 24px margin.
    pub fn new() -> Self {
        Self { known: HashMap::new(), cell: 96.0, gap: 4.0, min_cell: 0.0, margin: 24.0 }
    }
}

impl Default for FreeArrange {
    fn default() -> Self {
        Self::new()
    }
}

impl Arrange for FreeArrange {
    fn place(&self, items: &[ItemBox], viewport_w: f64) -> Placement {
        // Pitch is `cell` (clamped to the glyph floor) plus `gap`, the same on
        // both axes and independent of the item widths, so a single wide label
        // cannot stretch the grid. Labels wider than the pitch overlap.
        let pitch_x = self.cell.max(self.min_cell) + self.gap;
        let pitch_y = pitch_x;

        let cols = ((viewport_w / pitch_x).floor() as i64).max(1);
        // Cells already claimed by known positions (snapped to the slot grid).
        let mut taken: std::collections::HashSet<(i64, i64)> = self
            .known
            .values()
            .map(|&(x, y)| ((x / pitch_x).round() as i64, (y / pitch_y).round() as i64))
            .collect();
        let mut next = 0_i64;
        let mut positions = vec![(0.0, 0.0); items.len()];
        for (i, pos) in positions.iter_mut().enumerate() {
            if let Some(&known) = self.known.get(&i) {
                *pos = known;
                continue;
            }
            loop {
                let cell = (next % cols, next / cols);
                next += 1;
                if taken.insert(cell) {
                    *pos = (cell.0 as f64 * pitch_x, cell.1 as f64 * pitch_y);
                    break;
                }
            }
        }
        let mut max_x = viewport_w;
        let mut max_y = 0.0_f64;
        for (i, &(x, y)) in positions.iter().enumerate() {
            max_x = max_x.max(x + items[i].w + self.margin);
            max_y = max_y.max(y + items[i].h + self.margin);
        }
        Placement { positions, virtual_w: max_x, virtual_h: max_y }
    }
}

/// The candidate index closest to `from` lying in the (`dx`, `dy`) half-plane.
/// `dx`/`dy` are each `-1`, `0`, or `1`. `None` if nothing lies that way.
pub fn nearest_in_direction(
    from: (f64, f64),
    candidates: &[(usize, (f64, f64))],
    dx: i32,
    dy: i32,
) -> Option<usize> {
    candidates
        .iter()
        .filter(|(_, (x, y))| {
            let ok_x = if dx > 0 {
                *x > from.0
            } else if dx < 0 {
                *x < from.0
            } else {
                true
            };
            let ok_y = if dy > 0 {
                *y > from.1
            } else if dy < 0 {
                *y < from.1
            } else {
                true
            };
            ok_x && ok_y && (*x, *y) != from
        })
        .min_by(|(_, a), (_, b)| {
            let da = (a.0 - from.0).powi(2) + (a.1 - from.1).powi(2);
            let db = (b.0 - from.0).powi(2) + (b.1 - from.1).powi(2);
            da.total_cmp(&db)
        })
        .map(|(i, _)| *i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxes(n: usize, w: f64, h: f64) -> Vec<ItemBox> {
        (0..n).map(|_| ItemBox { w, h }).collect()
    }

    #[test]
    fn wraps_at_the_column_count() {
        let g = GridArrange::new();
        // cell_w = 100 + 12 = 112; viewport 400 -> floor(400/112) = 3 columns.
        let p = g.place(&boxes(7, 100.0, 80.0), 400.0);
        assert_eq!(p.positions.len(), 7);
        assert_eq!(p.positions[0], (0.0, 0.0));
        assert_eq!(p.positions[2], (224.0, 0.0));
        assert_eq!(p.positions[3], (0.0, 92.0));
        // 7 items, 3 per row -> 3 rows -> virtual_h = 3 * 92.
        assert_eq!(p.virtual_h, 276.0);
    }

    #[test]
    fn at_least_one_column_when_viewport_is_narrower_than_a_cell() {
        let g = GridArrange::new();
        let p = g.place(&boxes(3, 200.0, 60.0), 50.0);
        assert_eq!(p.positions[0], (0.0, 0.0));
        assert_eq!(p.positions[1], (0.0, 72.0));
        assert_eq!(p.positions[2], (0.0, 144.0));
    }

    #[test]
    fn empty_input_is_a_zero_placement() {
        let g = GridArrange::new();
        let p = g.place(&[], 400.0);
        assert!(p.positions.is_empty());
        assert_eq!(p.virtual_h, 0.0);
    }

    #[test]
    fn known_positions_are_used_verbatim() {
        let mut known = HashMap::new();
        known.insert(0, (240.0, 120.0));
        known.insert(1, (40.0, 40.0));
        let f = FreeArrange { known, cell: 96.0, gap: 4.0, min_cell: 0.0, margin: 24.0 };
        let p = f.place(&boxes(2, 64.0, 80.0), 800.0);
        assert_eq!(p.positions[0], (240.0, 120.0));
        assert_eq!(p.positions[1], (40.0, 40.0));
        assert!(p.virtual_w >= 800.0);
        assert!(p.virtual_h >= 120.0 + 80.0 + 24.0);
    }

    #[test]
    fn unknown_items_get_non_overlapping_slots() {
        let f = FreeArrange::new();
        let p = f.place(&boxes(3, 64.0, 80.0), 300.0);
        assert_eq!(p.positions[0], (0.0, 0.0));
        assert_ne!(p.positions[1], p.positions[0]);
        assert_ne!(p.positions[2], p.positions[1]);
    }

    #[test]
    fn unknown_items_skip_cells_taken_by_known_items() {
        let mut known = HashMap::new();
        known.insert(1, (0.0, 0.0)); // item 1 pinned to the first cell
        let f = FreeArrange { known, cell: 96.0, gap: 4.0, min_cell: 0.0, margin: 24.0 };
        let p = f.place(&boxes(2, 64.0, 80.0), 300.0);
        assert_eq!(p.positions[1], (0.0, 0.0));
        assert_ne!(p.positions[0], (0.0, 0.0));
    }

    /// When `cell` covers the item box, auto-slotted rectangles do not overlap.
    #[test]
    fn auto_slotted_items_do_not_overlap_when_the_cell_covers_them() {
        let f = FreeArrange { cell: 140.0, ..FreeArrange::new() }; // wider than the items
        let items = boxes(6, 130.0, 105.0);
        let p = f.place(&items, 1000.0);
        for a in 0..items.len() {
            for b in (a + 1)..items.len() {
                let (ax, ay) = p.positions[a];
                let (bx, by) = p.positions[b];
                let overlap_x = ax < bx + items[b].w && bx < ax + items[a].w;
                let overlap_y = ay < by + items[b].h && by < ay + items[a].h;
                assert!(
                    !(overlap_x && overlap_y),
                    "items {a} at {:?} and {b} at {:?} overlap",
                    p.positions[a],
                    p.positions[b],
                );
            }
        }
    }

    /// The pitch is `cell + gap` regardless of the item width: a wide label does
    /// not push the neighbours away, and a small `cell` packs icons tight.
    #[test]
    fn pitch_is_cell_plus_gap_independent_of_item_width() {
        let f = FreeArrange { cell: 40.0, gap: 6.0, ..FreeArrange::new() };
        let p = f.place(&boxes(3, 120.0, 30.0), 10_000.0); // labels far wider than the cell
        assert_eq!(p.positions[0], (0.0, 0.0));
        assert_eq!(p.positions[1], (46.0, 0.0));
        assert_eq!(p.positions[2], (92.0, 0.0));
    }

    /// `min_cell` is a hard floor: `cell` below it does not tighten the pitch
    /// past the icon-box size, so glyphs never physically overlap.
    #[test]
    fn min_cell_floors_the_pitch() {
        let f = FreeArrange { cell: 8.0, gap: 0.0, min_cell: 64.0, ..FreeArrange::new() };
        let p = f.place(&boxes(2, 200.0, 200.0), 10_000.0);
        assert_eq!(p.positions[1], (64.0, 0.0));
    }

    #[test]
    fn nearest_in_direction_picks_the_closest_to_the_right() {
        let cands = vec![(0, (0.0, 0.0)), (1, (100.0, 0.0)), (2, (100.0, 90.0)), (3, (-50.0, 0.0))];
        assert_eq!(nearest_in_direction((0.0, 0.0), &cands, 1, 0), Some(1));
        assert_eq!(nearest_in_direction((0.0, 0.0), &cands, 0, 1), Some(2));
        assert_eq!(nearest_in_direction((0.0, 0.0), &cands, -1, 0), Some(3));
    }
}
