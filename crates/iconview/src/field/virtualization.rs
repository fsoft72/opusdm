//! Virtualization and layout synchronization for `IconField`.
//!
//! Handles viewport-based realization calculation (`wanted_indices`), widget
//! insertion/removal (`sync_realized`), and worst-case item box caching.

use super::{IconField, VIRTUALIZE_THRESHOLD};
use crate::arrange::{Arrange, FreeArrange, GridArrange, ItemBox};
use crate::item::IconItem;
use crate::types::{IconEntry, IconKind, LayoutMode};
use gtk4 as gtk;
use gtk::prelude::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

impl IconField {
    pub(super) fn rebuild_arranger(&self) {
        let boxed: Box<dyn Arrange> = match self.inner.layout.mode.get() {
            LayoutMode::Grid => Box::new(GridArrange::new()),
            LayoutMode::Free => {
                let mut fa = FreeArrange::new();
                fa.known = self.inner.layout.free_positions.borrow().clone();
                fa.cell = self.inner.layout.grid_spacing.get() as f64;
                fa.gap = self.inner.layout.grid_gap.get() as f64;
                // Never let the pitch fall below the icon box, or glyphs would
                // overlap and hit-testing would pick the wrong icon.
                fa.min_cell = self.inner.layout.icon_size.get() as f64;
                Box::new(fa)
            }
        };
        *self.inner.layout.arrange.borrow_mut() = boxed;
    }

    /// Recompute every entry's position (realized or not) and cache the item
    /// rectangles, then reconcile which entries need a live widget.
    pub(crate) fn relayout(&self) {
        let n = self.inner.layout.entries.borrow().len();
        if n == 0 {
            self.inner.layout.item_rects.borrow_mut().clear();
            self.inner.layout.fixed.set_size_request(0, 0);
            self.sync_realized();
            return;
        }
        // A realized item's true measured box; an unrealized one gets the
        // worst case so the arrangement's cell size (derived from the max
        // over all boxes - see `GridArrange`) never needs to change as items
        // realize or de-realize while scrolling.
        let worst = self.worst_case_box();
        let boxes: Vec<ItemBox> = {
            let realized = self.inner.layout.realized.borrow();
            (0..n)
                .map(|i| {
                    realized
                        .get(&i)
                        .map(|it| it.measure())
                        .filter(|&(w, h)| w > 1.0 && h > 1.0)
                        .map(|(w, h)| ItemBox { w, h })
                        .unwrap_or(worst)
                })
                .collect()
        };
        let page = self.inner.layout.scroller.hadjustment().page_size();
        let viewport_w = if page > 1.0 {
            page
        } else {
            self.inner.layout.scroller.width().max(1) as f64
        };
        if matches!(self.inner.layout.mode.get(), LayoutMode::Free) {
            self.rebuild_arranger();
        }
        let placement = self.inner.layout.arrange.borrow().place(&boxes, viewport_w);
        let rects: Vec<(f64, f64, f64, f64)> = placement
            .positions
            .iter()
            .zip(boxes.iter())
            .map(|(&(x, y), b)| (x, y, b.w, b.h))
            .collect();
        *self.inner.layout.item_rects.borrow_mut() = rects;
        self.inner
            .layout
            .fixed
            .set_size_request(placement.virtual_w as i32, placement.virtual_h as i32);
        self.sync_realized();
    }

    /// A single representative worst-case item box: `IconItem::new` caps the
    /// name label at two lines, so a long-enough unbreakable name reliably
    /// measures the tallest/widest any item ever gets, regardless of which
    /// entry it is. Cached per icon size (a throwaway probe widget, built
    /// only on a cache miss).
    pub(super) fn worst_case_box(&self) -> ItemBox {
        let size = self.inner.layout.icon_size.get();
        if let Some((cached_size, b)) = *self.inner.layout.size_cache.borrow() {
            if cached_size == size {
                return b;
            }
        }
        let probe_entry = IconEntry {
            name: "a-long-unbreakable-filename-used-only-to-measure-worst-case.ext".to_string(),
            path: PathBuf::new(),
            kind: IconKind::File,
            mode: 0,
            ext: None,
        };
        let probe = IconItem::new(&probe_entry, size, &self.inner.layout.icon_set.borrow());
        let (w, h) = probe.measure();
        let b = if w > 1.0 && h > 1.0 {
            ItemBox { w, h }
        } else {
            ItemBox {
                w: (size + 12) as f64,
                h: (size + 40) as f64,
            }
        };
        *self.inner.layout.size_cache.borrow_mut() = Some((size, b));
        b
    }

    /// Which entry indices should currently have a live widget: all of them
    /// below `VIRTUALIZE_THRESHOLD`, otherwise whatever intersects the
    /// visible viewport plus one page of padding above and below.
    pub(super) fn wanted_indices(&self, n: usize) -> BTreeSet<usize> {
        if n < VIRTUALIZE_THRESHOLD {
            return (0..n).collect();
        }
        let vadj = self.inner.layout.scroller.vadjustment();
        let page = vadj.page_size().max(1.0);
        let want_top = (vadj.value() - page).max(0.0);
        let want_bottom = vadj.value() + page + page;

        self.inner
            .layout
            .item_rects
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, &(_, y, _, h))| y + h >= want_top && y <= want_bottom)
            .map(|(i, _)| i)
            .collect()
    }

    /// Create or drop widgets so exactly the wanted indices are realized,
    /// and move every realized widget to its current cached rectangle.
    pub(crate) fn sync_realized(&self) {
        let n = self.inner.layout.entries.borrow().len();
        let wanted = self.wanted_indices(n);
        let mut realized = self.inner.layout.realized.borrow_mut();

        let stale: Vec<usize> = realized.keys().copied().filter(|i| !wanted.contains(i)).collect();
        for i in stale {
            if let Some(it) = realized.remove(&i) {
                self.inner.layout.fixed.remove(it.widget());
            }
        }

        let entries = self.inner.layout.entries.borrow();
        let rects = self.inner.layout.item_rects.borrow();
        let size = self.inner.layout.icon_size.get();
        let icon_set = self.inner.layout.icon_set.borrow();
        let sel = self.inner.selection.selected.borrow();
        let cur = self.inner.selection.cursor.get();
        let thumb_cache = self.inner.layout.thumb_cache.borrow();

        for &i in &wanted {
            if realized.contains_key(&i) {
                continue;
            }
            let Some(entry) = entries.get(i) else {
                continue;
            };
            let it = IconItem::new(entry, size, &icon_set);
            if let Some(tex) = thumb_cache.get(&i) {
                it.set_thumbnail(tex);
            }
            it.set_selected(sel.contains(&i));
            it.set_cursor(cur == Some(i));
            let (x, y, _, _) = rects.get(i).copied().unwrap_or((0.0, 0.0, 0.0, 0.0));
            self.inner.layout.fixed.put(it.widget(), x, y);
            let this = self.clone();
            it.connect_edit_committed(move |new_name| {
                for cb in this.inner.events.on_rename_requested.borrow().iter() {
                    cb(i, new_name.clone());
                }
            });
            realized.insert(i, it);
        }

        for (&i, it) in realized.iter() {
            if let Some(&(x, y, _, _)) = rects.get(i) {
                self.inner.layout.fixed.move_(it.widget(), x, y);
            }
        }
    }
}
