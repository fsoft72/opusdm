//! The icon field: a `Fixed` in a `ScrolledWindow`, one `IconItem` per entry,
//! positioned by a pluggable `Arrange`.
//!
//! Controllers and input gestures are in [`controllers`].
//! Keyboard navigation and coordinate stepping are in [`navigation`].
//! Layout virtualization and widget recycling are in [`virtualization`].
//!
//! Above [`VIRTUALIZE_THRESHOLD`] entries, only items in (or near) the
//! visible viewport get a live `IconItem` widget - see `sync_realized`.
//! Everything else (selection, rubber-band, keyboard nav, DnD, reposition)
//! already keys off `item_rects`/`entries` by index, not off live widgets,
//! so none of it needs to know whether an index is currently realized.

mod controllers;
mod navigation;
mod virtualization;

use crate::arrange::{Arrange, GridArrange, ItemBox};
use crate::item::IconItem;
use crate::types::{IconEntry, LayoutMode};
use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

/// A list of listener closures with signature `F`. Shared (`Rc`) so an
/// emitter can copy the list and call it without holding the borrow: a
/// listener may then register another listener without a panic.
type Listeners<F> = RefCell<Vec<Rc<F>>>;

/// The listeners in `list` right now, copied so none of them runs while the
/// list is borrowed.
pub(super) fn listeners<F: ?Sized>(list: &Listeners<F>) -> Vec<Rc<F>> {
    list.borrow().clone()
}

/// Pinned icons that were moved: each path with its new position.
type PinnedMoves = Vec<(PathBuf, (f64, f64))>;

/// Entry counts at or above this stop getting a live widget per item;
/// realization becomes viewport-driven instead (see `sync_realized`).
pub(super) const VIRTUALIZE_THRESHOLD: usize = 400;

/// Widget tree, content and arrangement: what to draw, and where. Also owns
/// the in-progress reposition drag, since that only ever mutates layout.
pub(super) struct LayoutState {
    pub(super) scroller: gtk::ScrolledWindow,
    pub(super) fixed: gtk::Fixed,
    /// Live widgets, keyed by entry index. Sparse once virtualized - see
    /// `sync_realized`.
    pub(super) realized: RefCell<HashMap<usize, IconItem>>,
    /// Thumbnail textures survive an item being de-realized and re-realized,
    /// so scrolling away and back doesn't re-request or lose a thumbnail.
    pub(super) thumb_cache: RefCell<HashMap<usize, gtk::gdk::Texture>>,
    /// A single worst-case item box (see `worst_case_box`), cached per icon
    /// size so the arrangement never reflows as items realize/de-realize.
    pub(super) size_cache: RefCell<Option<(i32, ItemBox)>>,
    pub(super) entries: RefCell<Vec<IconEntry>>,
    pub(super) mode: Cell<LayoutMode>,
    pub(super) icon_size: Cell<i32>,
    pub(super) icon_set: RefCell<opusdm_ipc::IconSetId>,
    /// Auto-slot cell size for `FreeArrange`, in pixels. Ignored in `Grid`
    /// mode. Default matches `FreeArrange::new`'s `cell`.
    pub(super) grid_spacing: Cell<i32>,
    /// Dead space between auto-slot cells for `FreeArrange`, in pixels. Ignored
    /// in `Grid` mode. Default matches `FreeArrange::new`'s `gap`.
    pub(super) grid_gap: Cell<i32>,
    pub(super) arrange: RefCell<Box<dyn Arrange>>,
    pub(super) free_positions: RefCell<HashMap<usize, (f64, f64)>>,
    /// Positions pinned by path (left-out desktop icons). Applied on top of
    /// the index-keyed `free_positions` by `apply_pinned`, so they survive a
    /// reload that clears or reorders the entries.
    pub(super) pinned: RefCell<HashMap<PathBuf, (f64, f64)>>,
    /// Last laid-out rectangle per item: (x, y, w, h) in the `Fixed`'s space.
    /// Filled by `relayout`; hit-testing and nav read this, not GTK geometry,
    /// so they work before the widget is realized.
    pub(super) item_rects: RefCell<Vec<(f64, f64, f64, f64)>>,
    pub(super) reposition_active: Cell<bool>,
    /// While a reposition drag is live: (item index, its position at drag start).
    pub(super) drag_bases: RefCell<Vec<(usize, (f64, f64))>>,
}

/// Selection and keyboard-cursor state, independent of how items are laid out.
pub(super) struct SelectionState {
    pub(super) selected: RefCell<BTreeSet<usize>>,
    pub(super) anchor: Cell<Option<usize>>,
    pub(super) cursor: Cell<Option<usize>>,
}

/// Registered listener closures, one list per event kind.
pub(super) struct Events {
    pub(super) on_selection_changed: Listeners<dyn Fn()>,
    pub(super) on_activate: Listeners<dyn Fn(usize)>,
    pub(super) on_background_activate: Listeners<dyn Fn()>,
    pub(super) on_drop_uris: Listeners<dyn Fn(Vec<PathBuf>, i32, bool)>,
    pub(super) on_context_menu: Listeners<dyn Fn(i32, f64, f64)>,
    pub(super) on_rename_requested: Listeners<dyn Fn(usize, String)>,
    pub(super) on_zoom_requested: Listeners<dyn Fn(i32)>,
    pub(super) on_move_icons: Listeners<dyn Fn()>,
    pub(super) on_pinned_moved: Listeners<dyn Fn(PinnedMoves)>,
}

pub(super) struct Inner {
    pub(super) layout: LayoutState,
    pub(super) selection: SelectionState,
    pub(super) events: Events,
    pub(super) thumb_pool: Option<crate::thumbs::ThumbPool>,
}

/// The reusable icon view. Cheap to clone (shared `Rc`).
#[derive(Clone)]
pub struct IconField {
    pub(super) inner: Rc<Inner>,
}

/// A weak handle to an `IconField`, held by the closures on widgets the
/// field owns so they do not keep it (and its thumbnail workers) alive.
#[derive(Clone)]
pub(super) struct WeakIconField(Weak<Inner>);

impl WeakIconField {
    /// The field, while some strong handle still holds it.
    pub(super) fn upgrade(&self) -> Option<IconField> {
        self.0.upgrade().map(|inner| IconField { inner })
    }
}

impl IconField {
    /// A weak handle to this field, for signal handlers.
    pub(super) fn downgrade(&self) -> WeakIconField {
        WeakIconField(Rc::downgrade(&self.inner))
    }

    /// A new field in grid mode, 64px icons.
    pub fn new() -> IconField {
        let fixed = gtk::Fixed::new();
        let scroller = gtk::ScrolledWindow::builder().child(&fixed).build();
        scroller.set_hexpand(true);
        scroller.set_vexpand(true);

        let (thumb_pool, thumb_rx) = if crate::thumbs::ThumbPool::disabled() {
            (None, None)
        } else {
            let (p, r) = crate::thumbs::ThumbPool::new();
            (Some(p), Some(r))
        };

        let inner = Rc::new(Inner {
            layout: LayoutState {
                scroller,
                fixed,
                realized: RefCell::new(HashMap::new()),
                thumb_cache: RefCell::new(HashMap::new()),
                size_cache: RefCell::new(None),
                entries: RefCell::new(Vec::new()),
                mode: Cell::new(LayoutMode::Grid),
                icon_size: Cell::new(crate::types::DEFAULT_ICON_SIZE),
                icon_set: RefCell::new(opusdm_ipc::IconSetId::default()),
                grid_spacing: Cell::new(96),
                grid_gap: Cell::new(4),
                arrange: RefCell::new(Box::new(GridArrange::new())),
                free_positions: RefCell::new(HashMap::new()),
                pinned: RefCell::new(HashMap::new()),
                item_rects: RefCell::new(Vec::new()),
                reposition_active: Cell::new(false),
                drag_bases: RefCell::new(Vec::new()),
            },
            selection: SelectionState {
                selected: RefCell::new(BTreeSet::new()),
                anchor: Cell::new(None),
                cursor: Cell::new(None),
            },
            events: Events {
                on_selection_changed: RefCell::new(Vec::new()),
                on_activate: RefCell::new(Vec::new()),
                on_background_activate: RefCell::new(Vec::new()),
                on_drop_uris: RefCell::new(Vec::new()),
                on_context_menu: RefCell::new(Vec::new()),
                on_rename_requested: RefCell::new(Vec::new()),
                on_zoom_requested: RefCell::new(Vec::new()),
                on_move_icons: RefCell::new(Vec::new()),
                on_pinned_moved: RefCell::new(Vec::new()),
            },
            thumb_pool,
        });
        let field = IconField { inner };
        crate::style::install();

        // Drain finished thumbnails on the main loop. The loop ends when the
        // field is gone: dropping it drops the pool, whose workers then close
        // the channel.
        if let Some(rx) = thumb_rx {
            let weak = field.downgrade();
            gtk::glib::spawn_future_local(async move {
                while let Ok(res) = rx.recv().await {
                    let Some(this) = weak.upgrade() else { break };
                    this.apply_thumb_result(res);
                }
            });
        }

        // Re-flow when the visible width changes or the field is first shown.
        let weak = field.downgrade();
        field.inner.layout.scroller.hadjustment().connect_page_size_notify(move |_| {
            if let Some(this) = weak.upgrade() { this.relayout(); }
        });
        let weak = field.downgrade();
        field.inner.layout.fixed.connect_map(move |_| {
            if let Some(this) = weak.upgrade() { this.relayout(); }
        });
        // Scrolling doesn't move anything, just changes which items need a
        // live widget once virtualized - a cheap resync, not a full relayout.
        let weak = field.downgrade();
        field.inner.layout.scroller.vadjustment().connect_value_changed(move |_| {
            if let Some(this) = weak.upgrade() { this.sync_realized(); }
        });

        // Install interaction controllers (click, drag, key, dnd, context menu, scroll, reposition).
        controllers::install_controllers(&field);

        field
    }

    /// Outer widget for embedding.
    pub fn widget(&self) -> &gtk::Widget {
        self.inner.layout.scroller.upcast_ref()
    }

    /// Abandon a reposition drag in progress. `drag_bases` holds entry indices,
    /// which mean something else once the entry list is replaced, so a reload
    /// must not let the drag commit.
    fn cancel_reposition(&self) {
        self.inner.layout.reposition_active.set(false);
        self.inner.layout.drag_bases.borrow_mut().clear();
    }

    /// Replace all entries. Rebuilds the item widgets and re-lays-out.
    pub fn set_entries(&self, entries: &[IconEntry]) {
        for it in self.inner.layout.realized.borrow_mut().drain() {
            self.inner.layout.fixed.remove(it.1.widget());
        }
        self.inner.layout.thumb_cache.borrow_mut().clear();
        self.cancel_reposition();
        *self.inner.layout.entries.borrow_mut() = entries.to_vec();
        self.inner.layout.free_positions.borrow_mut().clear();
        self.apply_pinned();
        self.inner.selection.selected.borrow_mut().clear();
        self.inner.selection.anchor.set(None);
        self.inner.selection.cursor.set(None);
        self.relayout();
        self.emit_selection_changed();
        self.request_thumbs();
    }

    /// Reconcile the field with a fresh listing of the same directory,
    /// preserving view state across the update. Unlike `set_entries`, the
    /// selection, anchor, cursor and free positions follow their entries by
    /// path (surviving reorders and renames land as a plain update), cached
    /// thumbnails are re-keyed to the new indices and reused, and only
    /// entries that actually changed get their widget rebuilt and their
    /// thumbnail re-requested. Use `set_entries` for a different directory.
    pub fn refresh_entries(&self, entries: &[IconEntry]) {
        self.cancel_reposition();
        let old = std::mem::replace(&mut *self.inner.layout.entries.borrow_mut(), entries.to_vec());
        let new_index_of: HashMap<&std::path::Path, usize> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.path.as_path(), i))
            .collect();
        let remap = |i: usize| old.get(i).and_then(|e| new_index_of.get(e.path.as_path()).copied());

        // Selection, anchor and cursor follow their entries by path.
        {
            let mut sel = self.inner.selection.selected.borrow_mut();
            *sel = sel.iter().filter_map(|&i| remap(i)).collect();
        }
        if let Some(a) = self.inner.selection.anchor.get() {
            self.inner.selection.anchor.set(remap(a));
        }
        if let Some(c) = self.inner.selection.cursor.get() {
            self.inner.selection.cursor.set(remap(c));
        }

        // Free positions follow their entries too.
        {
            let mut fp = self.inner.layout.free_positions.borrow_mut();
            let prev: Vec<_> = fp.drain().collect();
            for (i, pos) in prev {
                if let Some(ni) = remap(i) {
                    fp.insert(ni, pos);
                }
            }
        }

        self.apply_pinned();

        // Re-key cached thumbnails to the new indices; entries that kept
        // their path and modification time keep their thumbnail.
        {
            let mut cache = self.inner.layout.thumb_cache.borrow_mut();
            let prev: Vec<_> = cache.drain().collect();
            for (i, tex) in prev {
                let Some(ni) = remap(i) else { continue };
                // An edited file keeps its row but not its thumbnail.
                if thumbnail_still_valid(&old[i], &entries[ni]) {
                    cache.insert(ni, tex);
                }
            }
        }

        // Rebuild only the realized widgets whose entry changed; unchanged
        // ones survive in place (their thumbnails included).
        {
            let mut realized = self.inner.layout.realized.borrow_mut();
            let stale: Vec<usize> = realized
                .keys()
                .copied()
                .filter(|&i| old.get(i) != entries.get(i))
                .collect();
            for i in stale {
                if let Some(it) = realized.remove(&i) {
                    self.inner.layout.fixed.remove(it.widget());
                }
            }
        }

        self.relayout();
        self.emit_selection_changed();
        self.request_missing_thumbs();
    }

    /// Queue thumbnail jobs for previewable entries not already covered by
    /// the thumbnail cache, limited to `wanted_indices()` (plus its one-page
    /// padding) so a huge directory doesn't queue thousands of decodes for
    /// entries that are nowhere near the viewport. `sync_realized` submits
    /// the rest as items scroll into range. Keeps the current generation: an
    /// in-flight result for a stale path is still dropped by
    /// `apply_thumb_result`'s per-entry path check.
    pub(super) fn request_missing_thumbs(&self) {
        let Some(pool) = self.inner.thumb_pool.as_ref() else {
            return;
        };
        let generation = pool.generation();
        let size = crate::types::ThumbSize::for_icon(self.inner.layout.icon_size.get());
        let cache = self.inner.layout.thumb_cache.borrow();
        let entries = self.inner.layout.entries.borrow();
        let wanted = self.wanted_indices(entries.len());
        for index in wanted {
            if cache.contains_key(&index) {
                continue;
            }
            let Some(entry) = entries.get(index) else {
                continue;
            };
            let Some(kind) = crate::thumbs::wanted(entry.ext.as_deref()) else {
                continue;
            };
            pool.submit(crate::thumbs::ThumbRequest {
                index,
                path: entry.path.clone(),
                kind,
                size,
                generation,
            });
        }
    }

    /// Queue thumbnail jobs for the wanted entries under a fresh generation,
    /// discarding results still in flight for the previous entry list.
    fn request_thumbs(&self) {
        let Some(pool) = self.inner.thumb_pool.as_ref() else {
            return;
        };
        pool.bump_generation();
        self.request_missing_thumbs();
    }

    /// Turn a finished thumbnail into a texture and show it, unless the entry
    /// list moved on (generation bumped) or the row now holds a different path.
    fn apply_thumb_result(&self, res: crate::thumbs::ThumbResult) {
        let Some(pool) = self.inner.thumb_pool.as_ref() else {
            return;
        };
        if res.generation != pool.generation() {
            return;
        }
        {
            let entries = self.inner.layout.entries.borrow();
            let Some(entry) = entries.get(res.index) else {
                return;
            };
            if entry.path != res.path {
                return;
            }
        }
        let bytes = gtk::glib::Bytes::from_owned(res.png);
        let Ok(texture) = gtk::gdk::Texture::from_bytes(&bytes) else {
            return;
        };
        if let Some(item) = self.inner.layout.realized.borrow().get(&res.index) {
            item.set_thumbnail(&texture);
        }
        // Cached regardless of realization, so scrolling this index back
        // into view doesn't need a fresh thumbnail job.
        self.inner.layout.thumb_cache.borrow_mut().insert(res.index, texture);
    }

    /// Switch arrangement strategy.
    pub fn set_mode(&self, mode: LayoutMode) {
        self.inner.layout.mode.set(mode);
        self.rebuild_arranger();
        self.relayout();
    }

    /// Set the desktop auto-slot cell size in pixels. Takes effect in
    /// `Free` mode; harmless in `Grid` mode.
    pub fn set_grid_spacing(&self, px: i32) {
        if self.inner.layout.grid_spacing.get() == px {
            return;
        }
        self.inner.layout.grid_spacing.set(px);
        self.rebuild_arranger();
        self.relayout();
    }

    /// Set the dead space between desktop auto-slot cells in pixels. Takes
    /// effect in `Free` mode; harmless in `Grid` mode.
    pub fn set_grid_gap(&self, px: i32) {
        if self.inner.layout.grid_gap.get() == px {
            return;
        }
        self.inner.layout.grid_gap.set(px);
        self.rebuild_arranger();
        self.relayout();
    }

    /// Resize every item's icon box.
    pub fn set_icon_size(&self, px: i32) {
        let old_px = self.inner.layout.icon_size.get();
        self.inner.layout.icon_size.set(px);
        for it in self.inner.layout.realized.borrow().values() {
            it.set_icon_size(px);
        }
        self.relayout();
        let before = crate::types::ThumbSize::for_icon(old_px);
        let after = crate::types::ThumbSize::for_icon(px);
        if before != after {
            for it in self.inner.layout.realized.borrow().values() {
                it.clear_thumbnail();
            }
            self.inner.layout.thumb_cache.borrow_mut().clear();
            self.request_thumbs();
        }
    }

    /// Switch the icon library used for entries without a content thumbnail.
    /// Only the fallback icon rendering changes, so entries, selection,
    /// cursor, free positions and the thumbnail cache are all preserved;
    /// the realized item widgets are rebuilt to pick up the new set.
    pub fn set_icon_set(&self, set: opusdm_ipc::IconSetId) {
        if *self.inner.layout.icon_set.borrow() == set {
            return;
        }
        *self.inner.layout.icon_set.borrow_mut() = set;
        for (_, it) in self.inner.layout.realized.borrow_mut().drain() {
            self.inner.layout.fixed.remove(it.widget());
        }
        // A glyph and an image can measure differently; drop the cached
        // worst-case box so the relayout re-probes with the new set.
        *self.inner.layout.size_cache.borrow_mut() = None;
        self.relayout();
    }

    /// Number of entries currently shown.
    pub fn entry_count(&self) -> usize {
        self.inner.layout.entries.borrow().len()
    }

    /// Indices of selected items, ascending.
    pub fn selected_indices(&self) -> Vec<usize> {
        self.inner.selection.selected.borrow().iter().copied().collect()
    }

    /// Absolute paths of selected items.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let entries = self.inner.layout.entries.borrow();
        self.inner
            .selection
            .selected
            .borrow()
            .iter()
            .filter_map(|&i| entries.get(i).map(|e| e.path.clone()))
            .collect()
    }

    /// Register a selection-changed listener.
    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) {
        self.inner.events.on_selection_changed.borrow_mut().push(Rc::new(f));
    }

    /// Register an activate (open) listener.
    pub fn connect_activate(&self, f: impl Fn(usize) + 'static) {
        self.inner.events.on_activate.borrow_mut().push(Rc::new(f));
    }

    /// Register a listener for a double-click on empty space (no item under
    /// the pointer) - e.g. to open a new window rooted at this field's
    /// directory.
    pub fn connect_background_activate(&self, f: impl Fn() + 'static) {
        self.inner.events.on_background_activate.borrow_mut().push(Rc::new(f));
    }

    /// Register a drop listener: (dropped paths, target index or `-1`, shift held).
    pub fn connect_drop_uris(&self, f: impl Fn(Vec<PathBuf>, i32, bool) + 'static) {
        self.inner.events.on_drop_uris.borrow_mut().push(Rc::new(f));
    }

    /// Register a context-menu listener: (index or `-1`, x, y).
    pub fn connect_context_menu(&self, f: impl Fn(i32, f64, f64) + 'static) {
        self.inner.events.on_context_menu.borrow_mut().push(Rc::new(f));
    }

    /// Register an inline-rename listener: (index, new name).
    pub fn connect_rename_requested(&self, f: impl Fn(usize, String) + 'static) {
        self.inner.events.on_rename_requested.borrow_mut().push(Rc::new(f));
    }

    /// Register a zoom listener: signed step from Ctrl+scroll.
    pub fn connect_zoom_requested(&self, f: impl Fn(i32) + 'static) {
        self.inner.events.on_zoom_requested.borrow_mut().push(Rc::new(f));
    }

    /// Register a listener fired after icons are repositioned in Free mode.
    pub fn connect_move_icons(&self, f: impl Fn() + 'static) {
        self.inner.events.on_move_icons.borrow_mut().push(Rc::new(f));
    }

    /// Replace the pinned positions (by path). They apply to the entries
    /// shown by the next `set_entries` / `refresh_entries`.
    pub fn set_pinned_positions(&self, positions: HashMap<PathBuf, (f64, f64)>) {
        *self.inner.layout.pinned.borrow_mut() = positions;
    }

    /// Register a listener fired at the end of a reposition drag with the new
    /// position of every moved entry whose `left_out` or `group` flag is set. Not fired
    /// when no such entry moved.
    pub fn connect_pinned_moved(&self, f: impl Fn(PinnedMoves) + 'static) {
        self.inner.events.on_pinned_moved.borrow_mut().push(Rc::new(f));
    }

    /// Copy the pinned positions onto the entries that currently have one.
    fn apply_pinned(&self) {
        let pinned = self.inner.layout.pinned.borrow();
        if pinned.is_empty() {
            return;
        }
        let entries = self.inner.layout.entries.borrow();
        let mut fp = self.inner.layout.free_positions.borrow_mut();
        for (i, entry) in entries.iter().enumerate() {
            if let Some(&pos) = pinned.get(&entry.path) {
                fp.insert(i, pos);
            }
        }
    }

    /// The entry behind index `i`, cloned.
    pub fn entry(&self, i: usize) -> Option<IconEntry> {
        self.inner.layout.entries.borrow().get(i).cloned()
    }

    /// Translate a Fixed-space point - as reported to a `connect_context_menu`
    /// or `connect_drop_uris` listener - into `ancestor`'s coordinate space.
    /// Accounts for the field's placement and any scroll offset, so a caller
    /// that lays its own menu on some enclosing widget (rather than a
    /// `Popover` parented to this field) can still position it at the click.
    /// `None` if `ancestor` is not actually an ancestor of this field.
    pub fn translate_from_canvas(&self, ancestor: &impl IsA<gtk::Widget>, x: f64, y: f64) -> Option<(f64, f64)> {
        self.inner.layout.fixed.translate_coordinates(ancestor, x, y)
    }

    /// Parse a `text/uri-list` payload and fan it out to the drop listeners.
    pub(super) fn handle_drop_text(&self, text: &str, target: i32, shift: bool) {
        let paths = crate::dnd::uris_to_paths(text);
        if paths.is_empty() {
            return;
        }
        for cb in listeners(&self.inner.events.on_drop_uris) {
            cb(paths.clone(), target, shift);
        }
    }

    /// Whether a primary-button drag should start an inter-window file drag.
    /// In Free mode a plain drag is an internal reposition, so only a drag
    /// started with Ctrl held (`ctrl`) exports the files there.
    pub(super) fn inter_window_drag_enabled(&self, ctrl: bool) -> bool {
        ctrl || !matches!(self.inner.layout.mode.get(), LayoutMode::Free)
    }

    /// Begin a reposition: only in Free mode and only when the press lands on
    /// an item. Selects that item if it was not already, and snapshots the
    /// start positions of every item that will move.
    pub(super) fn begin_reposition(&self, x: f64, y: f64) {
        self.inner.layout.reposition_active.set(false);
        self.inner.layout.drag_bases.borrow_mut().clear();
        if !matches!(self.inner.layout.mode.get(), LayoutMode::Free) {
            return;
        }
        let Some(hit) = self.item_at(x, y) else {
            return;
        };
        if !self.inner.selection.selected.borrow().contains(&hit) {
            self.select_exclusive(hit);
        }
        let rects = self.inner.layout.item_rects.borrow();
        let bases: Vec<(usize, (f64, f64))> = self
            .selected_indices()
            .into_iter()
            .filter_map(|i| rects.get(i).map(|&(rx, ry, _, _)| (i, (rx, ry))))
            .collect();
        drop(rects);
        if bases.is_empty() {
            return;
        }
        *self.inner.layout.drag_bases.borrow_mut() = bases;
        self.inner.layout.reposition_active.set(true);
    }

    /// Live feedback: move the dragged items to base + offset, no commit.
    pub(super) fn update_reposition(&self, ox: f64, oy: f64) {
        if !self.inner.layout.reposition_active.get() {
            return;
        }
        let realized = self.inner.layout.realized.borrow();
        for &(i, (bx, by)) in self.inner.layout.drag_bases.borrow().iter() {
            if let Some(it) = realized.get(&i) {
                self.inner.layout.fixed.move_(it.widget(), (bx + ox).max(0.0), (by + oy).max(0.0));
            }
        }
    }

    /// Commit the reposition: write the final positions, re-lay-out, notify.
    pub(super) fn end_reposition(&self, ox: f64, oy: f64) {
        if !self.inner.layout.reposition_active.get() {
            return;
        }
        self.inner.layout.reposition_active.set(false);
        let bases = std::mem::take(&mut *self.inner.layout.drag_bases.borrow_mut());
        if ox.abs() < 4.0 && oy.abs() < 4.0 {
            self.relayout(); // undo any sub-threshold nudge from update
            return;
        }
        {
            let mut fp = self.inner.layout.free_positions.borrow_mut();
            for &(i, (bx, by)) in &bases {
                fp.insert(i, ((bx + ox).max(0.0), (by + oy).max(0.0)));
            }
        }
        // Left-out entries also update their pin, so a reload keeps the spot.
        let moved: Vec<(PathBuf, (f64, f64))> = {
            let fp = self.inner.layout.free_positions.borrow();
            let entries = self.inner.layout.entries.borrow();
            bases
                .iter()
                .filter_map(|&(i, _)| {
                    let entry = entries.get(i).filter(|e| e.left_out || e.group)?;
                    Some((entry.path.clone(), *fp.get(&i)?))
                })
                .collect()
        };
        {
            let mut pinned = self.inner.layout.pinned.borrow_mut();
            for (path, pos) in &moved {
                pinned.insert(path.clone(), *pos);
            }
        }
        self.relayout();
        for cb in listeners(&self.inner.events.on_move_icons) {
            cb();
        }
        if !moved.is_empty() {
            for cb in listeners(&self.inner.events.on_pinned_moved) {
                cb(moved.clone());
            }
        }
    }

    /// Snapshot of the current free-mode positions.
    pub fn free_positions(&self) -> HashMap<usize, (f64, f64)> {
        self.inner.layout.free_positions.borrow().clone()
    }

    pub(super) fn emit_selection_changed(&self) {
        self.sync_selection_classes();
        for cb in listeners(&self.inner.events.on_selection_changed) {
            cb();
        }
    }

    pub(super) fn emit_activate(&self, i: usize) {
        for cb in listeners(&self.inner.events.on_activate) {
            cb(i);
        }
    }

    pub(super) fn emit_background_activate(&self) {
        for cb in listeners(&self.inner.events.on_background_activate) {
            cb();
        }
    }

    pub(super) fn sync_selection_classes(&self) {
        let sel = self.inner.selection.selected.borrow();
        let cur = self.inner.selection.cursor.get();
        for (&i, it) in self.inner.layout.realized.borrow().iter() {
            it.set_selected(sel.contains(&i));
            it.set_cursor(cur == Some(i));
        }
    }

    pub(super) fn select_exclusive(&self, i: usize) {
        {
            let mut sel = self.inner.selection.selected.borrow_mut();
            sel.clear();
            sel.insert(i);
        }
        self.inner.selection.anchor.set(Some(i));
        self.inner.selection.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(super) fn toggle(&self, i: usize) {
        {
            let mut sel = self.inner.selection.selected.borrow_mut();
            if !sel.remove(&i) {
                sel.insert(i);
            }
        }
        self.inner.selection.anchor.set(Some(i));
        self.inner.selection.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(super) fn select_range_to(&self, i: usize) {
        let a = self.inner.selection.anchor.get().unwrap_or(i);
        let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
        {
            let mut sel = self.inner.selection.selected.borrow_mut();
            sel.clear();
            for k in lo..=hi {
                sel.insert(k);
            }
        }
        self.inner.selection.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(super) fn select_all(&self) {
        let n = self.inner.layout.entries.borrow().len();
        {
            let mut sel = self.inner.selection.selected.borrow_mut();
            sel.clear();
            for k in 0..n {
                sel.insert(k);
            }
        }
        self.emit_selection_changed();
    }

    pub(super) fn clear_selection(&self) {
        self.inner.selection.selected.borrow_mut().clear();
        self.emit_selection_changed();
    }

    pub(super) fn set_cursor(&self, i: usize) {
        self.inner.selection.cursor.set(Some(i));
        self.sync_selection_classes();
    }

    pub(super) fn cursor(&self) -> Option<usize> {
        self.inner.selection.cursor.get()
    }

    /// Select every item whose bounds intersect `rect` (x, y, w, h in the
    /// `Fixed`'s coordinate space). When `additive`, keep the prior selection.
    pub(super) fn rubber_band_to(&self, rect: (f64, f64, f64, f64), additive: bool) {
        let (rx, ry, rw, rh) = rect;
        let mut sel = if additive {
            self.inner.selection.selected.borrow().clone()
        } else {
            BTreeSet::new()
        };
        for (i, &(ix, iy, iw, ih)) in self.inner.layout.item_rects.borrow().iter().enumerate() {
            let hit = ix < rx + rw && ix + iw > rx && iy < ry + rh && iy + ih > ry;
            if hit {
                sel.insert(i);
            }
        }
        *self.inner.selection.selected.borrow_mut() = sel;
        self.emit_selection_changed();
    }

    /// The live widget for entry `i`, if it is currently realized.
    pub(super) fn item_at_index(&self, i: usize) -> Option<IconItem> {
        self.inner.layout.realized.borrow().get(&i).cloned()
    }
}

impl Default for IconField {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a thumbnail made for `old` still shows `new`: same file, not
/// modified since.
fn thumbnail_still_valid(old: &IconEntry, new: &IconEntry) -> bool {
    old.path == new.path && old.modified == new.modified
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_edited_file_loses_its_thumbnail_but_an_untouched_one_keeps_it() {
        let entry = |modified| IconEntry {
            name: "a.png".into(),
            path: "/d/a.png".into(),
            kind: IconKind::File,
            mode: 0o100_644,
            ext: Some("png".into()),
            modified,
            left_out: false,
            group: false,
        };
        assert!(thumbnail_still_valid(&entry(10), &entry(10)));
        assert!(!thumbnail_still_valid(&entry(10), &entry(11)));
    }

    use super::*;
    use crate::item::IconItem;
    use crate::types::IconKind;
    use std::path::PathBuf;

    /// A minimal valid 1x1 PNG, needed wherever a test decodes the bytes
    /// through `gtk::gdk::Texture::from_bytes` (unlike `icon_set.rs`'s own
    /// tests, which only compare raw bytes and never decode them).
    const TEST_PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn entries(n: usize) -> Vec<IconEntry> {
        (0..n)
            .map(|i| IconEntry {
                name: format!("file{i}.txt"),
                path: PathBuf::from(format!("/tmp/file{i}.txt")),
                kind: IconKind::File,
                mode: 0o100_644,
                ext: Some("txt".into()),
                modified: 0,
                left_out: false,
                group: false,
            })
            .collect()
    }

    /// An icon set naming a theme that is not installed: no icons resolve.
    fn no_icons() -> opusdm_ipc::IconSetId {
        opusdm_ipc::IconSetId::Custom("no-such-theme".to_string())
    }

    /// All widget behaviour in one test: GTK panics if `gtk::init` runs on two
    /// threads, and libtest gives every `#[test]` its own thread, so the crate
    /// keeps exactly one GTK test.
    /// Every GTK scenario below, run one after the other on this thread:
    /// GTK may only be used from the thread that initialized it, so they
    /// cannot be separate tests. A failing scenario does not hide the others;
    /// each failure prints its own panic message.
    #[test]
    fn gtk_widget_behavior() {
        if gtk::init().is_err() {
            return;
        }
        // Theme lookups read the config folder: keep them in a scratch one.
        let _env_guard = crate::test_env_lock();
        let config = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", config.path());
        let scenarios: &[(&str, fn())] = &[
            ("icon_item_basics", icon_item_basics),
            ("icon_item_tall_thumbnail", icon_item_tall_thumbnail),
            ("icon_item_left_out_badge", icon_item_left_out_badge),
            ("field_entries_and_modes", field_entries_and_modes),
            ("field_icon_set_switch", field_icon_set_switch),
            ("field_selection", field_selection),
            ("field_rubber_band_and_cursor", field_rubber_band_and_cursor),
            ("field_drop", field_drop),
            ("field_reposition_selection", field_reposition_selection),
            ("field_refresh_keeps_view_state", field_refresh_keeps_view_state),
            ("field_thumbnail_wiring", field_thumbnail_wiring),
            ("field_inter_window_drag_and_reposition", field_inter_window_drag_and_reposition),
            ("field_grid_spacing", field_grid_spacing),
            ("field_pinned_positions", field_pinned_positions),
            ("field_reload_cancels_drag", field_reload_cancels_drag),
            ("field_group_entry", field_group_entry),
            ("field_virtualization", field_virtualization),
            ("field_closures_hold_no_strong_refs", field_closures_hold_no_strong_refs),
            ("field_listeners_may_register_listeners", field_listeners_may_register_listeners),
        ];
        let failed: Vec<&str> =
            scenarios.iter().filter(|(_, run)| std::panic::catch_unwind(*run).is_err()).map(|(name, _)| *name).collect();
        assert!(failed.is_empty(), "failed GTK scenarios: {failed:?}");
    }

    /// IconItem selection, edit commit, long names and the thumbnail slot.
    fn icon_item_basics() {
        // --- IconItem: selected class toggles ---
        let it = IconItem::new(&entries(1)[0], 64, &no_icons());
        assert!(!it.widget().has_css_class("selected"));
        it.set_selected(true);
        assert!(it.widget().has_css_class("selected"));
        it.set_selected(false);
        assert!(!it.widget().has_css_class("selected"));

        // --- IconItem: edit commit fires only on a real change ---
        let hits = Rc::new(RefCell::new(Vec::<String>::new()));
        let h = hits.clone();
        it.connect_edit_committed(move |s| h.borrow_mut().push(s));
        it.start_edit();
        it.finish_edit(Some("file0.txt".to_string())); // unchanged
        assert!(hits.borrow().is_empty());
        it.start_edit();
        it.finish_edit(Some("renamed.txt".to_string()));
        assert_eq!(&*hits.borrow(), &["renamed.txt".to_string()]);
        it.start_edit();
        it.finish_edit(None); // Esc
        assert_eq!(hits.borrow().len(), 1);

        // --- IconItem: a long unbreakable name does not widen the item ---
        // `name_label` wraps at character boundaries, so a spaceless, hyphenless
        // filename stays within `max_width_chars` instead of reporting its full
        // single-line width and stretching every column of the auto-slot grid.
        let long_text = "726432604_122131481643172103_8689873794089316539_n.jpg";
        let long_name = IconEntry {
            name: long_text.to_string(),
            path: PathBuf::from(format!("/tmp/{long_text}")),
            kind: IconKind::File,
            mode: 0o100_644,
            ext: Some("jpg".into()),
            modified: 0,
            left_out: false,
            group: false,
        };
        let long = IconItem::new(&long_name, 64, &no_icons());
        // Width of the same text on one unwrapped line, for reference.
        let single_line = gtk::Label::new(Some(long_text));
        let (_, single_w, _, _) = single_line.measure(gtk::Orientation::Horizontal, -1);
        // The item stays near the 14-char wrap cap, nowhere near the full
        // single-line width that would stretch every column of the grid.
        assert!(long.measure().0 < single_w as f64 / 2.0);

        // --- IconItem: thumbnail slot swaps the glyph out and back ---
        let tex = gtk::gdk::MemoryTexture::new(
            1,
            1,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &gtk::glib::Bytes::from_static(&[0, 0, 0, 255]),
            4,
        );
        it.set_thumbnail(tex.upcast_ref::<gtk::gdk::Texture>());
        it.clear_thumbnail();
    }

    /// A tall thumbnail keeps the item size.
    fn icon_item_tall_thumbnail() {
        // --- IconItem: a tall thumbnail does not change the item's size ---
        // The thumbnail is a `gtk::Image` at `pixel_size`, so a portrait
        // texture measures the same as the glyph. Otherwise a thumbnail
        // loading in asynchronously would grow the item and, via
        // `FreeArrange`'s max-of-all pitch, re-space the whole desktop grid.
        let plain = IconItem::new(&entries(1)[0], 64, &no_icons());
        let before = plain.measure();
        let tall = gtk::gdk::MemoryTexture::new(
            2,
            200,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &gtk::glib::Bytes::from_owned(vec![0u8; 2 * 200 * 4]),
            2 * 4,
        );
        plain.set_thumbnail(tall.upcast_ref::<gtk::gdk::Texture>());
        assert_eq!(plain.measure(), before);
    }

    /// The left-out badge keeps the item size.
    fn icon_item_left_out_badge() {
        // --- IconItem: a left-out entry shows the badge without changing size ---
        let mut left = entries(1).remove(0);
        left.left_out = true;
        let badged = IconItem::new(&left, 64, &no_icons());
        let unbadged = IconItem::new(&entries(1)[0], 64, &no_icons());
        assert!(badged.shows_badge());
        assert!(!unbadged.shows_badge());
        assert_eq!(badged.measure(), unbadged.measure());
    }

    /// Background activation, entry count, mode and size changes.
    fn field_entries_and_modes() {
        // --- IconField: background double-click fires connect_background_activate ---
        let f = IconField::new();
        let bg_hits = Rc::new(Cell::new(0));
        let h = bg_hits.clone();
        f.connect_background_activate(move || h.set(h.get() + 1));
        f.emit_background_activate();
        assert_eq!(bg_hits.get(), 1);

        // --- IconField: set_entries builds one item per entry ---
        f.set_entries(&entries(5));
        assert_eq!(f.entry_count(), 5);
        f.set_entries(&entries(2));
        assert_eq!(f.entry_count(), 2);

        // --- IconField: mode and size changes are safe ---
        f.set_entries(&entries(3));
        f.set_mode(LayoutMode::Free);
        f.set_icon_size(96);
        f.set_mode(LayoutMode::Grid);
        f.set_icon_size(64);
    }

    /// Switching icon sets keeps the view state; a theme icon shows as an image.
    fn field_icon_set_switch() {
        let f = IconField::new();
        f.set_entries(&entries(3));
        // --- IconField: set_icon_set rebuilds with real icons ---
        let config = PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").expect("set by the runner"));
        let icons_dir = config.join("opusdm").join("themes").join("test-theme").join("icons");
        std::fs::create_dir_all(&icons_dir).unwrap();
        std::fs::write(icons_dir.join("text.png"), TEST_PNG_1X1).unwrap();
        let custom = opusdm_ipc::IconSetId::Custom("test-theme".to_string());

        f.set_icon_set(custom.clone());
        assert_eq!(f.entry_count(), 3);
        // View state survives an icon-set switch: selection, cursor and free
        // positions are about content, not about how icons render.
        f.select_exclusive(1);
        f.inner.layout.free_positions.borrow_mut().insert(1, (12.0, 34.0));
        f.set_icon_set(no_icons());
        assert_eq!(f.entry_count(), 3);
        assert_eq!(f.selected_indices(), vec![1]);
        assert_eq!(f.cursor(), Some(1));
        assert_eq!(f.free_positions().get(&1), Some(&(12.0, 34.0)));

        // --- IconItem: a theme with the icon shows the image, one without stays blank ---
        let themed = IconItem::new(&entries(1)[0], 64, &custom);
        assert!(themed.shows_image());
        let blank = IconItem::new(&entries(1)[0], 64, &no_icons());
        assert!(!blank.shows_image());
    }

    /// Selection helpers, the changed signal and selected paths.
    fn field_selection() {
        let f = IconField::new();
        // --- IconField: selection helpers and the changed signal ---
        f.set_entries(&entries(5));
        let count = Rc::new(Cell::new(0));
        let c = count.clone();
        f.connect_selection_changed(move || c.set(c.get() + 1));
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
        assert!(count.get() >= 6);

        // --- IconField: selected_paths follow the entries ---
        f.set_entries(&entries(3));
        f.select_exclusive(2);
        assert_eq!(f.selected_paths(), vec![PathBuf::from("/tmp/file2.txt")]);
    }

    /// Rubber-band selection and cursor steps.
    fn field_rubber_band_and_cursor() {
        let f = IconField::new();
        // --- IconField: rubber-band uses the cached rectangles ---
        f.set_entries(&entries(6));
        f.inner.layout.scroller.set_size_request(400, 400);
        f.relayout();
        f.rubber_band_to((0.0, 0.0, 10_000.0, 10_000.0), false);
        assert_eq!(f.selected_indices().len(), 6);
        f.rubber_band_to((99_999.0, 99_999.0, 1.0, 1.0), false);
        assert!(f.selected_indices().is_empty());

        // --- IconField: cursor steps by one in grid mode ---
        f.set_cursor(0);
        f.move_cursor(1, 0, false, true);
        assert_eq!(f.cursor(), Some(1));
    }

    /// A dropped uri-list is parsed and forwarded.
    fn field_drop() {
        let f = IconField::new();
        // --- IconField: drop parses the uri-list and forwards it ---
        f.set_entries(&entries(2));
        let got = Rc::new(RefCell::new(None::<(Vec<PathBuf>, i32, bool)>));
        let g = got.clone();
        f.connect_drop_uris(move |paths, target, shift| *g.borrow_mut() = Some((paths, target, shift)));
        f.handle_drop_text("file:///tmp/a.txt\r\nfile:///tmp/b%20c.txt\r\n", -1, true);
        let (paths, target, shift) = got.borrow().clone().unwrap();
        assert_eq!(paths, vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b c.txt")]);
        assert_eq!(target, -1);
        assert!(shift);
    }

    /// Dragging moves the whole selection and fires move-icons.
    fn field_reposition_selection() {
        let f = IconField::new();
        // --- IconField: reposition moves the selection and fires move-icons ---
        f.set_mode(LayoutMode::Free);
        f.set_entries(&entries(3));
        f.inner.layout.scroller.set_size_request(600, 400);
        f.relayout();
        let fired = Rc::new(Cell::new(0));
        let fr = fired.clone();
        f.connect_move_icons(move || fr.set(fr.get() + 1));
        f.select_exclusive(0);
        f.toggle(2); // selection is {0, 2}
        let r0 = f.inner.layout.item_rects.borrow()[0];
        f.begin_reposition(r0.0 + 2.0, r0.1 + 2.0);
        f.update_reposition(25.0, 40.0);
        f.end_reposition(25.0, 40.0);
        let pos = f.free_positions();
        assert!(pos.contains_key(&0));
        assert!(pos.contains_key(&2));
        assert!(!pos.contains_key(&1));
        assert_eq!(fired.get(), 1);
    }

    /// refresh_entries keeps selection, cursor and positions by path.
    fn field_refresh_keeps_view_state() {
        let f = IconField::new();
        // --- IconField: refresh_entries keeps view state, keyed by path ---
        f.set_mode(LayoutMode::Free);
        f.set_entries(&entries(4));
        f.select_exclusive(2); // /tmp/file2.txt
        f.inner.layout.free_positions.borrow_mut().insert(2, (5.0, 6.0));
        let mut reordered = entries(4);
        reordered.swap(0, 2); // same entries, new order
        f.refresh_entries(&reordered);
        // The selection, cursor and free position followed file2 to index 0.
        assert_eq!(f.entry_count(), 4);
        assert_eq!(f.selected_indices(), vec![0]);
        assert_eq!(f.cursor(), Some(0));
        assert_eq!(f.free_positions().get(&0), Some(&(5.0, 6.0)));
        // A vanished entry drops out of the selection cleanly.
        let mut shrunk = reordered.clone();
        shrunk.remove(0); // removes file2.txt, the selected one
        f.refresh_entries(&shrunk);
        assert_eq!(f.entry_count(), 3);
        assert!(f.selected_indices().is_empty());
        assert!(f.cursor().is_none());
        assert!(f.free_positions().is_empty());
    }

    /// Thumbnail wiring with the pool on and off.
    fn field_thumbnail_wiring() {
        // --- IconField: thumbnail wiring is safe with the pool on and off ---
        std::env::set_var("OPUSDM_NO_THUMBS", "1");
        let f_off = IconField::new();
        f_off.set_entries(&entries(2));
        f_off.set_icon_size(160); // bucket change path, pool disabled
        std::env::remove_var("OPUSDM_NO_THUMBS");

        let f_on = IconField::new();
        f_on.set_entries(&[IconEntry {
            name: "note.rs".into(),
            path: PathBuf::from("/tmp/note.rs"),
            kind: IconKind::File,
            mode: 0o100_644,
            ext: Some("rs".into()),
            modified: 0,
            left_out: false,
            group: false,
        }]);
        // "rs" is not previewable, so no job is queued and nothing panics.
        f_on.set_icon_size(200);
    }

    /// Inter-window drag rules and moving one icon.
    fn field_inter_window_drag_and_reposition() {
        // --- IconField: inter-window drag is off in Free mode unless Ctrl ---
        let fd = IconField::new();
        assert!(fd.inter_window_drag_enabled(false)); // Grid default
        fd.set_mode(LayoutMode::Free);
        assert!(!fd.inter_window_drag_enabled(false));
        assert!(fd.inter_window_drag_enabled(true));
        fd.set_mode(LayoutMode::Grid);
        assert!(fd.inter_window_drag_enabled(false));

        // --- IconField: begin/commit reposition moves a selected icon ---
        fd.set_mode(LayoutMode::Free);
        fd.set_entries(&entries(3));
        fd.inner.layout.scroller.set_size_request(600, 400);
        fd.relayout();
        let start = fd.inner.layout.item_rects.borrow()[0];
        fd.begin_reposition(start.0 + 2.0, start.1 + 2.0); // press on item 0
        fd.update_reposition(50.0, 30.0);
        fd.end_reposition(50.0, 30.0);
        let moved = fd.free_positions();
        assert_eq!(moved.get(&0).copied(), Some((start.0 + 50.0, start.1 + 30.0)));
    }

    /// Grid spacing changes the Free auto-slot pitch.
    fn field_grid_spacing() {
        // --- IconField: set_grid_spacing changes the Free auto-slot pitch ---
        // The headless scroller reports no width, so the auto-slot grid is a
        // single column: a larger `grid_spacing` shows up as a taller row
        // pitch, i.e. item 1 sits further down.
        let fs = IconField::new();
        fs.set_mode(LayoutMode::Free);
        fs.set_icon_size(48);
        fs.set_entries(&entries(4));
        fs.set_grid_spacing(280);
        fs.relayout();
        let wide_y = fs.inner.layout.item_rects.borrow()[1].1;
        fs.set_grid_spacing(96);
        fs.relayout();
        let narrow_y = fs.inner.layout.item_rects.borrow()[1].1;
        assert!(wide_y > narrow_y, "row pitch should track grid spacing ({wide_y} vs {narrow_y})");
    }

    /// Pinned positions follow left-out entries by path.
    fn field_pinned_positions() {
        // --- IconField: pinned positions follow left-out entries by path ---
        let mk_left = |n: usize| {
            let mut e = entries(n);
            e[1].left_out = true;
            e
        };
        let target = PathBuf::from("/tmp/file1.txt");
        let fp = IconField::new();
        fp.set_mode(LayoutMode::Free);
        fp.inner.layout.scroller.set_size_request(600, 400);
        fp.set_pinned_positions(HashMap::from([(target.clone(), (200.0, 150.0))]));
        fp.set_entries(&mk_left(3));
        assert_eq!(fp.free_positions().get(&1), Some(&(200.0, 150.0)));
        assert!(!fp.free_positions().contains_key(&0), "unpinned entries stay auto-slotted");
        // A refresh that reorders the entries keeps the pin on the same path.
        let mut reordered = mk_left(3);
        reordered.swap(0, 1);
        fp.refresh_entries(&reordered);
        assert_eq!(fp.free_positions().get(&0), Some(&(200.0, 150.0)));
        // A full reload pins it again.
        fp.set_entries(&mk_left(3));
        assert_eq!(fp.free_positions().get(&1), Some(&(200.0, 150.0)));

        // Dragging reports only left-out entries, and remembers the new spot.
        let moved_log: Rc<RefCell<PinnedMoves>> = Rc::new(RefCell::new(Vec::new()));
        let log = moved_log.clone();
        fp.connect_pinned_moved(move |m| log.borrow_mut().extend(m));
        fp.select_all();
        let r1 = fp.inner.layout.item_rects.borrow()[1];
        fp.begin_reposition(r1.0 + 2.0, r1.1 + 2.0);
        fp.update_reposition(50.0, 30.0);
        fp.end_reposition(50.0, 30.0);
        assert_eq!(*moved_log.borrow(), vec![(target.clone(), (250.0, 180.0))]);
        fp.set_entries(&mk_left(3));
        assert_eq!(fp.free_positions().get(&1), Some(&(250.0, 180.0)));

        // A sub-threshold nudge reports nothing.
        moved_log.borrow_mut().clear();
        let r1 = fp.inner.layout.item_rects.borrow()[1];
        fp.begin_reposition(r1.0 + 2.0, r1.1 + 2.0);
        fp.end_reposition(1.0, 1.0);
        assert!(moved_log.borrow().is_empty());
    }

    /// A reload during a drag cancels it.
    fn field_reload_cancels_drag() {
        // --- IconField: a reload during a drag cancels it (no wrong pin is reported) ---
        let all_left = || {
            let mut e = entries(3);
            for x in &mut e {
                x.left_out = true;
            }
            e
        };
        let fr = IconField::new();
        fr.set_mode(LayoutMode::Free);
        fr.inner.layout.scroller.set_size_request(600, 400);
        fr.set_entries(&all_left());
        let stray: Rc<RefCell<PinnedMoves>> = Rc::new(RefCell::new(Vec::new()));
        let log = stray.clone();
        fr.connect_pinned_moved(move |m| log.borrow_mut().extend(m));
        fr.select_all();
        let r1 = fr.inner.layout.item_rects.borrow()[1];
        fr.begin_reposition(r1.0 + 2.0, r1.1 + 2.0); // press on file1
        let mut swapped = all_left();
        swapped.swap(0, 1); // a reload puts file0 at index 1
        fr.set_entries(&swapped);
        fr.end_reposition(50.0, 30.0);
        assert!(stray.borrow().is_empty(), "the drag must be cancelled by set_entries: {:?}", stray.borrow());
        // Same for refresh_entries.
        fr.select_all();
        let r1 = fr.inner.layout.item_rects.borrow()[1];
        fr.begin_reposition(r1.0 + 2.0, r1.1 + 2.0);
        fr.refresh_entries(&all_left());
        fr.end_reposition(50.0, 30.0);
        assert!(stray.borrow().is_empty(), "the drag must be cancelled by refresh_entries: {:?}", stray.borrow());
    }

    /// A group entry shows a badge and reports its pinned move.
    fn field_group_entry() {
        // --- IconField: a group entry shows a badge and reports its pinned move ---
        let mut grp = entries(1).remove(0);
        grp.group = true;
        grp.path = PathBuf::from("group:tools");
        let badged = IconItem::new(&grp, 64, &no_icons());
        assert!(badged.shows_badge());
        let unbadged = IconItem::new(&entries(1)[0], 64, &no_icons());
        assert_eq!(badged.measure(), unbadged.measure());
        let fg = IconField::new();
        fg.set_mode(LayoutMode::Free);
        fg.inner.layout.scroller.set_size_request(600, 400);
        let mut with_group = entries(2);
        with_group.push(grp.clone());
        fg.set_pinned_positions(HashMap::from([(PathBuf::from("group:tools"), (300.0, 200.0))]));
        fg.set_entries(&with_group);
        assert_eq!(fg.free_positions().get(&2), Some(&(300.0, 200.0)));
        let group_log: Rc<RefCell<PinnedMoves>> = Rc::new(RefCell::new(Vec::new()));
        let log = group_log.clone();
        fg.connect_pinned_moved(move |m| log.borrow_mut().extend(m));
        fg.select_all();
        let rg = fg.inner.layout.item_rects.borrow()[2];
        fg.begin_reposition(rg.0 + 2.0, rg.1 + 2.0);
        fg.update_reposition(40.0, 20.0);
        fg.end_reposition(40.0, 20.0);
        assert_eq!(*group_log.borrow(), vec![(PathBuf::from("group:tools"), (340.0, 220.0))]);
    }

    /// The signal handlers and the thumbnail drain loop hold the field and
    /// its items weakly, so dropping the last handle frees them.
    fn field_closures_hold_no_strong_refs() {
        let item = IconItem::new(&entries(1)[0], 64, &no_icons());
        assert_eq!(item.strong_count(), 1, "an item's own handlers must not keep it alive");
        let f = IconField::new();
        f.set_entries(&entries(3));
        f.inner.layout.scroller.set_size_request(400, 400);
        f.relayout();
        assert!(!f.inner.layout.realized.borrow().is_empty());
        assert_eq!(Rc::strong_count(&f.inner), 1, "the field's handlers must not keep it alive");
        let weak = Rc::downgrade(&f.inner);
        drop(f);
        assert!(weak.upgrade().is_none());
    }

    /// A listener may register more listeners while it runs: the list is
    /// not borrowed during the call.
    fn field_listeners_may_register_listeners() {
        let f = IconField::new();
        f.set_entries(&entries(2));
        let hits = Rc::new(Cell::new(0));
        let (inner_f, h) = (f.clone(), hits.clone());
        f.connect_selection_changed(move || {
            let h = h.clone();
            inner_f.connect_selection_changed(move || h.set(h.get() + 1));
        });
        f.select_exclusive(0);
        f.select_exclusive(1);
        assert_eq!(hits.get(), 1, "the listener added by the first call runs on the second");

        let item = IconItem::new(&entries(1)[0], 64, &no_icons());
        let (again, h) = (item.clone(), hits.clone());
        item.connect_edit_committed(move |_| {
            let h = h.clone();
            again.connect_edit_committed(move |_| h.set(h.get() + 10));
        });
        item.start_edit();
        item.finish_edit(Some("one.txt".to_string()));
        item.start_edit();
        item.finish_edit(Some("two.txt".to_string()));
        assert_eq!(hits.get(), 11);
    }

    /// Only the visible items of a big field have widgets.
    fn field_virtualization() {
        // --- IconField: virtualization above VIRTUALIZE_THRESHOLD ---
        let big = entries(VIRTUALIZE_THRESHOLD + 100);
        let v = IconField::new();
        v.inner.layout.scroller.set_size_request(400, 400);
        v.set_entries(&big);
        assert_eq!(v.entry_count(), big.len());
        // Positions are known for every entry even though most have no widget.
        assert_eq!(v.inner.layout.item_rects.borrow().len(), big.len());
        let realized_count = v.inner.layout.realized.borrow().len();
        assert!(
            realized_count < big.len(),
            "expected fewer live widgets than entries, got {realized_count} of {}",
            big.len()
        );
        assert!(realized_count > 0, "expected the initially visible items to be realized");
        // Selection and rubber-band are index/rect based, not widget based,
        // so they still cover every entry regardless of realization.
        v.select_all();
        assert_eq!(v.selected_indices().len(), big.len());
        v.clear_selection();
        v.rubber_band_to((0.0, 0.0, 100_000.0, 100_000.0), false);
        assert_eq!(v.selected_indices().len(), big.len());
        // Scrolling changes which indices are realized. The headless
        // scroller never runs a real layout pass, so its adjustment is
        // configured directly rather than relying on actual allocation.
        let before: BTreeSet<usize> = v.inner.layout.realized.borrow().keys().copied().collect();
        let total_h = v.inner.layout.item_rects.borrow().last().map(|&(_, y, _, h)| y + h).unwrap_or(0.0);
        let vadj = v.inner.layout.scroller.vadjustment();
        vadj.set_upper(total_h);
        vadj.set_page_size(400.0);
        vadj.set_value(total_h - 400.0);
        v.sync_realized();
        let after: BTreeSet<usize> = v.inner.layout.realized.borrow().keys().copied().collect();
        assert_ne!(before, after, "scrolling to the end should realize different indices");
        assert!(after.contains(&(big.len() - 1)), "the last entry should be realized once scrolled to");
    }
}
