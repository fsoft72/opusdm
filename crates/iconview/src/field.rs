//! The icon field: a `Fixed` in a `ScrolledWindow`, one `IconItem` per entry,
//! positioned by a pluggable `Arrange`. Interaction is added in later steps.

use crate::arrange::{Arrange, FreeArrange, GridArrange, ItemBox};
use crate::item::IconItem;
use crate::types::{IconEntry, LayoutMode};
use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
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
    selection: RefCell<BTreeSet<usize>>,
    anchor: Cell<Option<usize>>,
    cursor: Cell<Option<usize>>,
    on_selection_changed: RefCell<Vec<Box<dyn Fn()>>>,
    on_activate: RefCell<Vec<Box<dyn Fn(usize)>>>,
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
            selection: RefCell::new(BTreeSet::new()),
            anchor: Cell::new(None),
            cursor: Cell::new(None),
            on_selection_changed: RefCell::new(Vec::new()),
            on_activate: RefCell::new(Vec::new()),
        });
        let field = IconField { inner };

        // Re-flow when the visible width changes or the field is first shown.
        let this = field.clone();
        field.inner.scroller.hadjustment().connect_page_size_notify(move |_| this.relayout());
        let this = field.clone();
        field.inner.fixed.connect_map(move |_| this.relayout());

        // Left-click selection.
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        let this = field.clone();
        click.connect_pressed(move |g, n_press, x, y| {
            let hit = this.item_at(x, y);
            let state = g.current_event_state();
            let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            match hit {
                Some(i) if n_press >= 2 => this.emit_activate(i),
                Some(i) if ctrl => this.toggle(i),
                Some(i) if shift => this.select_range_to(i),
                Some(i) => this.select_exclusive(i),
                None if !ctrl && !shift => this.clear_selection(),
                None => {}
            }
        });
        field.inner.fixed.add_controller(click);

        // Rubber-band selection: a drag that begins on empty space.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gtk::gdk::BUTTON_PRIMARY);
        let start: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
        let this = field.clone();
        let start_b = start.clone();
        drag.connect_drag_begin(move |_, x, y| {
            if this.item_at(x, y).is_some() {
                start_b.set(None);
                return;
            }
            start_b.set(Some((x, y)));
        });
        let this = field.clone();
        let start_u = start.clone();
        drag.connect_drag_update(move |g, ox, oy| {
            let Some((sx, sy)) = start_u.get() else {
                return;
            };
            let additive = g
                .current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let rx = sx.min(sx + ox);
            let ry = sy.min(sy + oy);
            this.rubber_band_to((rx, ry, ox.abs(), oy.abs()), additive);
        });
        field.inner.fixed.add_controller(drag);

        // Keyboard navigation.
        field.inner.fixed.set_focusable(true);
        let key = gtk::EventControllerKey::new();
        let this = field.clone();
        key.connect_key_pressed(move |_, keyval, _, state| {
            use gtk::gdk::Key;
            let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            match keyval {
                Key::a | Key::A if ctrl => {
                    this.select_all();
                    gtk::glib::Propagation::Stop
                }
                Key::Escape => {
                    this.clear_selection();
                    gtk::glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter => {
                    if let Some(i) = this.cursor() {
                        this.emit_activate(i);
                    }
                    gtk::glib::Propagation::Stop
                }
                Key::space => {
                    if let Some(i) = this.cursor() {
                        this.toggle(i);
                    }
                    gtk::glib::Propagation::Stop
                }
                Key::F2 => {
                    if let Some(i) = this.cursor() {
                        if let Some(it) = this.items().get(i) {
                            it.start_edit();
                        }
                    }
                    gtk::glib::Propagation::Stop
                }
                Key::Left => {
                    this.move_cursor(-1, 0, shift, ctrl);
                    gtk::glib::Propagation::Stop
                }
                Key::Right => {
                    this.move_cursor(1, 0, shift, ctrl);
                    gtk::glib::Propagation::Stop
                }
                Key::Up => {
                    this.move_cursor(0, -1, shift, ctrl);
                    gtk::glib::Propagation::Stop
                }
                Key::Down => {
                    this.move_cursor(0, 1, shift, ctrl);
                    gtk::glib::Propagation::Stop
                }
                _ => gtk::glib::Propagation::Proceed,
            }
        });
        field.inner.fixed.add_controller(key);

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
        self.inner.selection.borrow_mut().clear();
        self.inner.anchor.set(None);
        self.inner.cursor.set(None);
        self.relayout();
        self.emit_selection_changed();
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

    /// Indices of selected items, ascending.
    pub fn selected_indices(&self) -> Vec<usize> {
        self.inner.selection.borrow().iter().copied().collect()
    }

    /// Absolute paths of selected items.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let entries = self.inner.entries.borrow();
        self.inner
            .selection
            .borrow()
            .iter()
            .filter_map(|&i| entries.get(i).map(|e| e.path.clone()))
            .collect()
    }

    /// Register a selection-changed listener.
    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) {
        self.inner.on_selection_changed.borrow_mut().push(Box::new(f));
    }

    /// Register an activate (open) listener.
    pub fn connect_activate(&self, f: impl Fn(usize) + 'static) {
        self.inner.on_activate.borrow_mut().push(Box::new(f));
    }

    fn emit_selection_changed(&self) {
        self.sync_selection_classes();
        for cb in self.inner.on_selection_changed.borrow().iter() {
            cb();
        }
    }

    pub(crate) fn emit_activate(&self, i: usize) {
        for cb in self.inner.on_activate.borrow().iter() {
            cb(i);
        }
    }

    fn sync_selection_classes(&self) {
        let sel = self.inner.selection.borrow();
        let cur = self.inner.cursor.get();
        for (i, it) in self.inner.items.borrow().iter().enumerate() {
            it.set_selected(sel.contains(&i));
            it.set_cursor(cur == Some(i));
        }
    }

    pub(crate) fn select_exclusive(&self, i: usize) {
        {
            let mut sel = self.inner.selection.borrow_mut();
            sel.clear();
            sel.insert(i);
        }
        self.inner.anchor.set(Some(i));
        self.inner.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(crate) fn toggle(&self, i: usize) {
        {
            let mut sel = self.inner.selection.borrow_mut();
            if !sel.remove(&i) {
                sel.insert(i);
            }
        }
        self.inner.anchor.set(Some(i));
        self.inner.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(crate) fn select_range_to(&self, i: usize) {
        let a = self.inner.anchor.get().unwrap_or(i);
        let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
        {
            let mut sel = self.inner.selection.borrow_mut();
            sel.clear();
            for k in lo..=hi {
                sel.insert(k);
            }
        }
        self.inner.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(crate) fn select_all(&self) {
        let n = self.inner.items.borrow().len();
        {
            let mut sel = self.inner.selection.borrow_mut();
            sel.clear();
            for k in 0..n {
                sel.insert(k);
            }
        }
        self.emit_selection_changed();
    }

    pub(crate) fn clear_selection(&self) {
        self.inner.selection.borrow_mut().clear();
        self.emit_selection_changed();
    }

    pub(crate) fn set_cursor(&self, i: usize) {
        self.inner.cursor.set(Some(i));
        self.sync_selection_classes();
    }

    pub(crate) fn cursor(&self) -> Option<usize> {
        self.inner.cursor.get()
    }

    /// Select every item whose bounds intersect `rect` (x, y, w, h in the
    /// `Fixed`'s coordinate space). When `additive`, keep the prior selection.
    pub(crate) fn rubber_band_to(&self, rect: (f64, f64, f64, f64), additive: bool) {
        let (rx, ry, rw, rh) = rect;
        let mut sel = if additive {
            self.inner.selection.borrow().clone()
        } else {
            BTreeSet::new()
        };
        for (i, it) in self.inner.items.borrow().iter().enumerate() {
            if let Some(b) = it.widget().compute_bounds(&self.inner.fixed) {
                let ix = b.x() as f64;
                let iy = b.y() as f64;
                let iw = b.width() as f64;
                let ih = b.height() as f64;
                let hit = ix < rx + rw && ix + iw > rx && iy < ry + rh && iy + ih > ry;
                if hit {
                    sel.insert(i);
                }
            }
        }
        *self.inner.selection.borrow_mut() = sel;
        self.emit_selection_changed();
    }

    /// Move the keyboard cursor by (`dx`, `dy`) in {-1, 0, 1}. `extend` grows
    /// the selection from the anchor; `move_only` leaves the selection alone.
    pub(crate) fn move_cursor(&self, dx: i32, dy: i32, extend: bool, move_only: bool) {
        let n = self.inner.items.borrow().len();
        if n == 0 {
            return;
        }
        let cur = self.inner.cursor.get().unwrap_or(0);
        let next = if matches!(self.inner.mode.get(), LayoutMode::Grid) {
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

    fn grid_step(&self, cur: usize, dx: i32, dy: i32, n: usize) -> Option<usize> {
        let cols = self.current_columns().max(1);
        let mut idx = cur as i64;
        idx += dx as i64;
        idx += dy as i64 * cols as i64;
        if idx < 0 || idx as usize >= n {
            return None;
        }
        Some(idx as usize)
    }

    fn free_step(&self, cur: usize, dx: i32, dy: i32) -> Option<usize> {
        let items = self.inner.items.borrow();
        let from = items.get(cur)?.widget().compute_bounds(&self.inner.fixed)?;
        let from = (from.x() as f64, from.y() as f64);
        let cands: Vec<(usize, (f64, f64))> = items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let b = it.widget().compute_bounds(&self.inner.fixed)?;
                Some((i, (b.x() as f64, b.y() as f64)))
            })
            .collect();
        crate::arrange::nearest_in_direction(from, &cands, dx, dy)
    }

    fn current_columns(&self) -> usize {
        let items = self.inner.items.borrow();
        if items.len() < 2 {
            return 1;
        }
        let y0 = items[0]
            .widget()
            .compute_bounds(&self.inner.fixed)
            .map(|b| b.y())
            .unwrap_or(0.0);
        let mut cols = 1;
        for it in items.iter().skip(1) {
            let y = it
                .widget()
                .compute_bounds(&self.inner.fixed)
                .map(|b| b.y())
                .unwrap_or(0.0);
            if (y - y0).abs() < 1.0 {
                cols += 1;
            } else {
                break;
            }
        }
        cols
    }

    /// The item index under widget-space point (`x`, `y`), if any.
    fn item_at(&self, x: f64, y: f64) -> Option<usize> {
        let items = self.inner.items.borrow();
        for (i, it) in items.iter().enumerate() {
            if let Some(bounds) = it.widget().compute_bounds(&self.inner.fixed) {
                if bounds.contains_point(&gtk::graphene::Point::new(x as f32, y as f32)) {
                    return Some(i);
                }
            }
        }
        None
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

    #[test]
    fn selection_helpers_and_signal() {
        if gtk::init().is_err() {
            return;
        }
        let f = IconField::new();
        f.set_entries(&entries(5));
        let hits = Rc::new(Cell::new(0));
        let h = hits.clone();
        f.connect_selection_changed(move || h.set(h.get() + 1));

        f.select_exclusive(1);
        assert_eq!(f.selected_indices(), vec![1]);
        f.toggle(3);
        assert_eq!(f.selected_indices(), vec![1, 3]);
        f.select_range_to(4); // anchor is 3 after toggle
        assert_eq!(f.selected_indices(), vec![3, 4]);
        f.select_exclusive(0);
        assert_eq!(f.selected_indices(), vec![0]);
        f.clear_selection();
        assert!(f.selected_indices().is_empty());
        f.select_all();
        assert_eq!(f.selected_indices(), vec![0, 1, 2, 3, 4]);

        assert!(hits.get() >= 6);
    }

    #[test]
    fn selected_paths_track_entries() {
        if gtk::init().is_err() {
            return;
        }
        let f = IconField::new();
        f.set_entries(&entries(3));
        f.select_exclusive(2);
        assert_eq!(f.selected_paths(), vec![PathBuf::from("/tmp/file2.txt")]);
    }

    #[test]
    fn rubber_band_selects_intersecting_items() {
        if gtk::init().is_err() {
            return;
        }
        let f = IconField::new();
        f.set_entries(&entries(6));
        f.inner.scroller.set_size_request(400, 400);
        f.relayout();
        f.rubber_band_to((0.0, 0.0, 10_000.0, 10_000.0), false);
        assert_eq!(f.selected_indices().len(), 6);
        f.rubber_band_to((99_999.0, 99_999.0, 1.0, 1.0), false);
        assert!(f.selected_indices().is_empty());
    }

    #[test]
    fn move_cursor_steps_in_grid() {
        if gtk::init().is_err() {
            return;
        }
        let f = IconField::new();
        f.set_entries(&entries(6));
        f.inner.scroller.set_size_request(400, 400);
        f.relayout();
        f.set_cursor(0);
        f.move_cursor(1, 0, false, true); // right
        assert_eq!(f.cursor(), Some(1));
    }
}
