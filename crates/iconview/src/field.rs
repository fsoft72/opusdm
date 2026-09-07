//! The icon field: a `Fixed` in a `ScrolledWindow`, one `IconItem` per entry,
//! positioned by a pluggable `Arrange`. Interaction is added in later steps.

use crate::arrange::{Arrange, FreeArrange, GridArrange, ItemBox};
use crate::item::IconItem;
use crate::types::{IconEntry, LayoutMode};
use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

struct Inner {
    scroller: gtk::ScrolledWindow,
    fixed: gtk::Fixed,
    items: RefCell<Vec<IconItem>>,
    entries: RefCell<Vec<IconEntry>>,
    mode: Cell<LayoutMode>,
    icon_size: Cell<i32>,
    arrange: RefCell<Box<dyn Arrange>>,
    free_positions: RefCell<HashMap<usize, (f64, f64)>>,
}

/// The reusable icon view. Cheap to clone (shared `Rc`).
#[derive(Clone)]
pub struct IconField {
    inner: Rc<Inner>,
}

impl IconField {
    /// A new field in grid mode, 64px icons.
    pub fn new() -> IconField {
        let fixed = gtk::Fixed::new();
        let scroller = gtk::ScrolledWindow::builder().child(&fixed).build();
        scroller.set_hexpand(true);
        scroller.set_vexpand(true);

        let inner = Rc::new(Inner {
            scroller,
            fixed,
            items: RefCell::new(Vec::new()),
            entries: RefCell::new(Vec::new()),
            mode: Cell::new(LayoutMode::Grid),
            icon_size: Cell::new(64),
            arrange: RefCell::new(Box::new(GridArrange::new())),
            free_positions: RefCell::new(HashMap::new()),
        });
        let field = IconField { inner };

        // Re-flow when the visible width changes or the field is first shown.
        let this = field.clone();
        field.inner.scroller.hadjustment().connect_page_size_notify(move |_| this.relayout());
        let this = field.clone();
        field.inner.fixed.connect_map(move |_| this.relayout());

        field
    }

    /// Outer widget for embedding.
    pub fn widget(&self) -> &gtk::Widget {
        self.inner.scroller.upcast_ref()
    }

    /// Replace all entries. Rebuilds the item widgets and re-lays-out.
    pub fn set_entries(&self, entries: &[IconEntry]) {
        for it in self.inner.items.borrow().iter() {
            self.inner.fixed.remove(it.widget());
        }
        let size = self.inner.icon_size.get();
        let mut items = Vec::with_capacity(entries.len());
        for e in entries {
            let it = IconItem::new(e, size);
            self.inner.fixed.put(it.widget(), 0.0, 0.0);
            items.push(it);
        }
        *self.inner.items.borrow_mut() = items;
        *self.inner.entries.borrow_mut() = entries.to_vec();
        self.inner.free_positions.borrow_mut().clear();
        self.relayout();
    }

    /// Switch arrangement strategy.
    pub fn set_mode(&self, mode: LayoutMode) {
        self.inner.mode.set(mode);
        self.rebuild_arranger();
        self.relayout();
    }

    /// Resize every item's icon box.
    pub fn set_icon_size(&self, px: i32) {
        self.inner.icon_size.set(px);
        for it in self.inner.items.borrow().iter() {
            it.set_icon_size(px);
        }
        self.relayout();
    }

    /// Number of entries currently shown.
    pub fn entry_count(&self) -> usize {
        self.inner.items.borrow().len()
    }

    pub(crate) fn items(&self) -> Vec<IconItem> {
        self.inner.items.borrow().clone()
    }

    fn rebuild_arranger(&self) {
        let boxed: Box<dyn Arrange> = match self.inner.mode.get() {
            LayoutMode::Grid => Box::new(GridArrange::new()),
            LayoutMode::Free => {
                let mut fa = FreeArrange::new();
                fa.known = self.inner.free_positions.borrow().clone();
                Box::new(fa)
            }
        };
        *self.inner.arrange.borrow_mut() = boxed;
    }

    /// Recompute positions and move every item.
    pub(crate) fn relayout(&self) {
        let items = self.inner.items.borrow();
        if items.is_empty() {
            self.inner.fixed.set_size_request(0, 0);
            return;
        }
        let boxes: Vec<ItemBox> = items
            .iter()
            .map(|it| {
                let (w, h) = it.measure();
                ItemBox { w, h }
            })
            .collect();
        let page = self.inner.scroller.hadjustment().page_size();
        let viewport_w = if page > 1.0 { page } else { self.inner.scroller.width().max(1) as f64 };
        if matches!(self.inner.mode.get(), LayoutMode::Free) {
            self.rebuild_arranger();
        }
        let placement = self.inner.arrange.borrow().place(&boxes, viewport_w);
        for (it, &(x, y)) in items.iter().zip(placement.positions.iter()) {
            self.inner.fixed.move_(it.widget(), x, y);
        }
        self.inner
            .fixed
            .set_size_request(placement.virtual_w as i32, placement.virtual_h as i32);
    }
}

impl Default for IconField {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::IconKind;
    use std::path::PathBuf;

    fn entries(n: usize) -> Vec<IconEntry> {
        (0..n)
            .map(|i| IconEntry {
                name: format!("file{i}.txt"),
                path: PathBuf::from(format!("/tmp/file{i}.txt")),
                kind: IconKind::File,
                mode: 0o100_644,
                ext: Some("txt".into()),
            })
            .collect()
    }

    #[test]
    fn set_entries_creates_one_widget_each() {
        if gtk::init().is_err() {
            return;
        }
        let f = IconField::new();
        f.set_entries(&entries(5));
        assert_eq!(f.entry_count(), 5);
        let mut child_count = 0;
        let mut c = f.inner.fixed.first_child();
        while let Some(w) = c {
            child_count += 1;
            c = w.next_sibling();
        }
        assert_eq!(child_count, 5);

        f.set_entries(&entries(2));
        assert_eq!(f.entry_count(), 2);
    }

    #[test]
    fn mode_and_size_changes_do_not_panic() {
        if gtk::init().is_err() {
            return;
        }
        let f = IconField::new();
        f.set_entries(&entries(3));
        f.set_mode(LayoutMode::Free);
        f.set_icon_size(96);
        f.set_mode(LayoutMode::Grid);
    }
}
