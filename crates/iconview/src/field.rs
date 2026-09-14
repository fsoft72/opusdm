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

/// A list of listener closures with signature `F`.
type Listeners<F> = RefCell<Vec<Box<F>>>;

/// Widget tree, content and arrangement: what to draw, and where. Also owns
/// the in-progress reposition drag, since that only ever mutates layout.
struct LayoutState {
    scroller: gtk::ScrolledWindow,
    fixed: gtk::Fixed,
    items: RefCell<Vec<IconItem>>,
    entries: RefCell<Vec<IconEntry>>,
    mode: Cell<LayoutMode>,
    icon_size: Cell<i32>,
    icon_set: RefCell<opusdm_ipc::IconSetId>,
    /// Auto-slot cell size for `FreeArrange`, in pixels. Ignored in `Grid`
    /// mode. Default matches `FreeArrange::new`'s `cell`.
    grid_spacing: Cell<i32>,
    /// Dead space between auto-slot cells for `FreeArrange`, in pixels. Ignored
    /// in `Grid` mode. Default matches `FreeArrange::new`'s `gap`.
    grid_gap: Cell<i32>,
    arrange: RefCell<Box<dyn Arrange>>,
    free_positions: RefCell<HashMap<usize, (f64, f64)>>,
    /// Last laid-out rectangle per item: (x, y, w, h) in the `Fixed`'s space.
    /// Filled by `relayout`; hit-testing and nav read this, not GTK geometry,
    /// so they work before the widget is realized.
    item_rects: RefCell<Vec<(f64, f64, f64, f64)>>,
    reposition_active: Cell<bool>,
    /// While a reposition drag is live: (item index, its position at drag start).
    drag_bases: RefCell<Vec<(usize, (f64, f64))>>,
}

/// Selection and keyboard-cursor state, independent of how items are laid out.
struct SelectionState {
    selected: RefCell<BTreeSet<usize>>,
    anchor: Cell<Option<usize>>,
    cursor: Cell<Option<usize>>,
}

/// Registered listener closures, one list per event kind.
struct Events {
    on_selection_changed: Listeners<dyn Fn()>,
    on_activate: Listeners<dyn Fn(usize)>,
    on_background_activate: Listeners<dyn Fn()>,
    on_drop_uris: Listeners<dyn Fn(Vec<PathBuf>, i32, bool)>,
    on_context_menu: Listeners<dyn Fn(i32, f64, f64)>,
    on_rename_requested: Listeners<dyn Fn(usize, String)>,
    on_zoom_requested: Listeners<dyn Fn(i32)>,
    on_move_icons: Listeners<dyn Fn()>,
}

struct Inner {
    layout: LayoutState,
    selection: SelectionState,
    events: Events,
    thumb_pool: Option<crate::thumbs::ThumbPool>,
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
                items: RefCell::new(Vec::new()),
                entries: RefCell::new(Vec::new()),
                mode: Cell::new(LayoutMode::Grid),
                icon_size: Cell::new(64),
                icon_set: RefCell::new(opusdm_ipc::IconSetId::None),
                grid_spacing: Cell::new(96),
                grid_gap: Cell::new(4),
                arrange: RefCell::new(Box::new(GridArrange::new())),
                free_positions: RefCell::new(HashMap::new()),
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
            },
            thumb_pool,
        });
        let field = IconField { inner };
        crate::style::install();

        // Drain finished thumbnails on the main loop.
        if let Some(rx) = thumb_rx {
            let this = field.clone();
            gtk::glib::spawn_future_local(async move {
                while let Ok(res) = rx.recv().await {
                    this.apply_thumb_result(res);
                }
            });
        }

        // Re-flow when the visible width changes or the field is first shown.
        let this = field.clone();
        field.inner.layout.scroller.hadjustment().connect_page_size_notify(move |_| this.relayout());
        let this = field.clone();
        field.inner.layout.fixed.connect_map(move |_| this.relayout());

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
                None if n_press >= 2 => this.emit_background_activate(),
                None if !ctrl && !shift => this.clear_selection(),
                None => {}
            }
        });
        field.inner.layout.fixed.add_controller(click);

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
        field.inner.layout.fixed.add_controller(drag);

        // Keyboard navigation.
        field.inner.layout.fixed.set_focusable(true);
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
        field.inner.layout.fixed.add_controller(key);

        // Drag source: export the selection as a uri-list for other windows.
        let src = gtk::DragSource::new();
        src.set_actions(gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
        let this = field.clone();
        src.connect_prepare(move |_, x, y| {
            crate::dbg_log!("icon field drag prepare at ({x}, {y})");
            // In Free mode a primary-button drag is an internal reposition, not
            // an inter-window file drag; leaving this on would let the DnD
            // machinery steal the gesture and snap the icon back.
            if !this.inter_window_drag_enabled() {
                crate::dbg_log!("icon field drag prepare: inter-window drag disabled");
                return None;
            }
            let paths = this.selected_paths();
            crate::dbg_log!("icon field drag prepare: selected paths={paths:?}");
            if paths.is_empty() {
                crate::dbg_log!("icon field drag prepare: empty selection, aborting");
                return None;
            }
            let payload = crate::dnd::paths_to_uri_list(&paths);
            Some(gtk::gdk::ContentProvider::for_bytes(
                "text/uri-list",
                &gtk::glib::Bytes::from(payload.as_bytes()),
            ))
        });
        field.inner.layout.fixed.add_controller(src);

        // Drop target: hand parsed paths to the consumer. GDK's builtin mime
        // table deserializes `text/uri-list` into `GdkFileList`, not a plain
        // string - requesting `glib::Type::STRING` here left no format in
        // common with a source that only advertises `text/uri-list`, so the
        // drop silently failed with `NoTarget` before ever reaching this
        // callback.
        let target = gtk::DropTarget::new(
            gtk::gdk::FileList::static_type(),
            gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE,
        );
        let this = field.clone();
        target.connect_drop(move |t, value, x, y| {
            crate::dbg_log!("icon field drop fired at ({x}, {y}), value type={:?}", value.type_());
            let Ok(file_list) = value.get::<gtk::gdk::FileList>() else {
                crate::dbg_log!("icon field drop: value is not a FileList");
                return false;
            };
            let paths: Vec<_> = file_list.files().into_iter().filter_map(|f| f.path()).collect();
            crate::dbg_log!("icon field drop: paths={paths:?}");
            let tgt = this.item_at(x, y).map(|i| i as i32).unwrap_or(-1);
            let event_shift = t
                .current_event()
                .map(|e| e.modifier_state().contains(gtk::gdk::ModifierType::SHIFT_MASK))
                .unwrap_or(false);
            let shift = crate::dnd::shift_held(&this.inner.layout.fixed);
            crate::dbg_log!("icon field drop: event_shift={event_shift} live_shift={shift}");
            this.handle_drop_text(&crate::dnd::paths_to_uri_list(&paths), tgt, shift);
            true
        });
        field.inner.layout.fixed.add_controller(target);

        // Right-click: select under the pointer, then emit context-menu.
        let rclick = gtk::GestureClick::new();
        rclick.set_button(gtk::gdk::BUTTON_SECONDARY);
        let this = field.clone();
        rclick.connect_pressed(move |_, _, x, y| {
            let hit = this.item_at(x, y);
            if let Some(i) = hit {
                if !this.selected_indices().contains(&i) {
                    this.select_exclusive(i);
                }
            }
            let idx = hit.map(|i| i as i32).unwrap_or(-1);
            for cb in this.inner.events.on_context_menu.borrow().iter() {
                cb(idx, x, y);
            }
        });
        field.inner.layout.fixed.add_controller(rclick);

        // Ctrl+scroll: zoom request.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let this = field.clone();
        scroll.connect_scroll(move |c, _dx, dy| {
            if !c.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                return gtk::glib::Propagation::Proceed;
            }
            let step = if dy < 0.0 { 1 } else { -1 };
            for cb in this.inner.events.on_zoom_requested.borrow().iter() {
                cb(step);
            }
            gtk::glib::Propagation::Stop
        });
        field.inner.layout.fixed.add_controller(scroll);

        // Internal reposition drag (Free mode only), starting on any item.
        let rdrag = gtk::GestureDrag::new();
        rdrag.set_button(gtk::gdk::BUTTON_PRIMARY);
        let this = field.clone();
        rdrag.connect_drag_begin(move |_, x, y| {
            this.begin_reposition(x, y);
        });
        let this = field.clone();
        rdrag.connect_drag_update(move |_, ox, oy| {
            this.update_reposition(ox, oy);
        });
        let this = field.clone();
        rdrag.connect_drag_end(move |_, ox, oy| {
            this.end_reposition(ox, oy);
        });
        field.inner.layout.fixed.add_controller(rdrag);

        field
    }

    /// Outer widget for embedding.
    pub fn widget(&self) -> &gtk::Widget {
        self.inner.layout.scroller.upcast_ref()
    }

    /// Replace all entries. Rebuilds the item widgets and re-lays-out.
    pub fn set_entries(&self, entries: &[IconEntry]) {
        for it in self.inner.layout.items.borrow().iter() {
            self.inner.layout.fixed.remove(it.widget());
        }
        let size = self.inner.layout.icon_size.get();
        let mut items = Vec::with_capacity(entries.len());
        for (idx, e) in entries.iter().enumerate() {
            let it = IconItem::new(e, size, &self.inner.layout.icon_set.borrow());
            self.inner.layout.fixed.put(it.widget(), 0.0, 0.0);
            let this = self.clone();
            it.connect_edit_committed(move |new_name| {
                for cb in this.inner.events.on_rename_requested.borrow().iter() {
                    cb(idx, new_name.clone());
                }
            });
            items.push(it);
        }
        *self.inner.layout.items.borrow_mut() = items;
        *self.inner.layout.entries.borrow_mut() = entries.to_vec();
        self.inner.layout.free_positions.borrow_mut().clear();
        self.inner.selection.selected.borrow_mut().clear();
        self.inner.selection.anchor.set(None);
        self.inner.selection.cursor.set(None);
        self.relayout();
        self.emit_selection_changed();
        self.request_thumbs();
    }

    /// Queue a thumbnail job for every previewable entry, newest generation.
    fn request_thumbs(&self) {
        let Some(pool) = self.inner.thumb_pool.as_ref() else {
            return;
        };
        pool.bump_generation();
        let size = crate::types::ThumbSize::for_icon(self.inner.layout.icon_size.get());
        for (index, entry) in self.inner.layout.entries.borrow().iter().enumerate() {
            let Some(kind) = crate::thumbs::wanted(entry.ext.as_deref()) else {
                continue;
            };
            pool.submit(crate::thumbs::ThumbRequest {
                index,
                path: entry.path.clone(),
                kind,
                size,
            });
        }
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
        if let Some(item) = self.inner.layout.items.borrow().get(res.index) {
            item.set_thumbnail(&texture);
        }
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
        for it in self.inner.layout.items.borrow().iter() {
            it.set_icon_size(px);
        }
        self.relayout();
        let before = crate::types::ThumbSize::for_icon(old_px);
        let after = crate::types::ThumbSize::for_icon(px);
        if before != after {
            for it in self.inner.layout.items.borrow().iter() {
                it.clear_thumbnail();
            }
            self.request_thumbs();
        }
    }

    /// Switch the icon library used for entries without a content thumbnail.
    /// Rebuilds every item.
    pub fn set_icon_set(&self, set: opusdm_ipc::IconSetId) {
        if *self.inner.layout.icon_set.borrow() == set {
            return;
        }
        *self.inner.layout.icon_set.borrow_mut() = set;
        let entries = self.inner.layout.entries.borrow().clone();
        self.set_entries(&entries);
    }

    /// Number of entries currently shown.
    pub fn entry_count(&self) -> usize {
        self.inner.layout.items.borrow().len()
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
        self.inner.events.on_selection_changed.borrow_mut().push(Box::new(f));
    }

    /// Register an activate (open) listener.
    pub fn connect_activate(&self, f: impl Fn(usize) + 'static) {
        self.inner.events.on_activate.borrow_mut().push(Box::new(f));
    }

    /// Register a listener for a double-click on empty space (no item under
    /// the pointer) - e.g. to open a new window rooted at this field's
    /// directory.
    pub fn connect_background_activate(&self, f: impl Fn() + 'static) {
        self.inner.events.on_background_activate.borrow_mut().push(Box::new(f));
    }

    /// Register a drop listener: (dropped paths, target index or `-1`, shift held).
    pub fn connect_drop_uris(&self, f: impl Fn(Vec<PathBuf>, i32, bool) + 'static) {
        self.inner.events.on_drop_uris.borrow_mut().push(Box::new(f));
    }

    /// Register a context-menu listener: (index or `-1`, x, y).
    pub fn connect_context_menu(&self, f: impl Fn(i32, f64, f64) + 'static) {
        self.inner.events.on_context_menu.borrow_mut().push(Box::new(f));
    }

    /// Register an inline-rename listener: (index, new name).
    pub fn connect_rename_requested(&self, f: impl Fn(usize, String) + 'static) {
        self.inner.events.on_rename_requested.borrow_mut().push(Box::new(f));
    }

    /// Register a zoom listener: signed step from Ctrl+scroll.
    pub fn connect_zoom_requested(&self, f: impl Fn(i32) + 'static) {
        self.inner.events.on_zoom_requested.borrow_mut().push(Box::new(f));
    }

    /// Register a listener fired after icons are repositioned in Free mode.
    pub fn connect_move_icons(&self, f: impl Fn() + 'static) {
        self.inner.events.on_move_icons.borrow_mut().push(Box::new(f));
    }

    /// The entry behind index `i`, cloned.
    pub fn entry(&self, i: usize) -> Option<IconEntry> {
        self.inner.layout.entries.borrow().get(i).cloned()
    }

    /// Parse a `text/uri-list` payload and fan it out to the drop listeners.
    pub(crate) fn handle_drop_text(&self, text: &str, target: i32, shift: bool) {
        let paths = crate::dnd::uris_to_paths(text);
        if paths.is_empty() {
            return;
        }
        for cb in self.inner.events.on_drop_uris.borrow().iter() {
            cb(paths.clone(), target, shift);
        }
    }

    /// Whether a primary-button drag should start an inter-window file drag.
    /// False in Free mode, where that gesture is an internal reposition.
    pub(crate) fn inter_window_drag_enabled(&self) -> bool {
        !matches!(self.inner.layout.mode.get(), LayoutMode::Free)
    }

    /// Begin a reposition: only in Free mode and only when the press lands on
    /// an item. Selects that item if it was not already, and snapshots the
    /// start positions of every item that will move.
    fn begin_reposition(&self, x: f64, y: f64) {
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
    fn update_reposition(&self, ox: f64, oy: f64) {
        if !self.inner.layout.reposition_active.get() {
            return;
        }
        let items = self.inner.layout.items.borrow();
        for &(i, (bx, by)) in self.inner.layout.drag_bases.borrow().iter() {
            if let Some(it) = items.get(i) {
                self.inner.layout.fixed.move_(it.widget(), (bx + ox).max(0.0), (by + oy).max(0.0));
            }
        }
    }

    /// Commit the reposition: write the final positions, re-lay-out, notify.
    fn end_reposition(&self, ox: f64, oy: f64) {
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
            for (i, (bx, by)) in bases {
                fp.insert(i, ((bx + ox).max(0.0), (by + oy).max(0.0)));
            }
        }
        self.relayout();
        for cb in self.inner.events.on_move_icons.borrow().iter() {
            cb();
        }
    }

    /// Snapshot of the current free-mode positions.
    pub fn free_positions(&self) -> HashMap<usize, (f64, f64)> {
        self.inner.layout.free_positions.borrow().clone()
    }

    fn emit_selection_changed(&self) {
        self.sync_selection_classes();
        for cb in self.inner.events.on_selection_changed.borrow().iter() {
            cb();
        }
    }

    pub(crate) fn emit_activate(&self, i: usize) {
        for cb in self.inner.events.on_activate.borrow().iter() {
            cb(i);
        }
    }

    fn emit_background_activate(&self) {
        for cb in self.inner.events.on_background_activate.borrow().iter() {
            cb();
        }
    }

    fn sync_selection_classes(&self) {
        let sel = self.inner.selection.selected.borrow();
        let cur = self.inner.selection.cursor.get();
        for (i, it) in self.inner.layout.items.borrow().iter().enumerate() {
            it.set_selected(sel.contains(&i));
            it.set_cursor(cur == Some(i));
        }
    }

    pub(crate) fn select_exclusive(&self, i: usize) {
        {
            let mut sel = self.inner.selection.selected.borrow_mut();
            sel.clear();
            sel.insert(i);
        }
        self.inner.selection.anchor.set(Some(i));
        self.inner.selection.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(crate) fn toggle(&self, i: usize) {
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

    pub(crate) fn select_range_to(&self, i: usize) {
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

    pub(crate) fn select_all(&self) {
        let n = self.inner.layout.items.borrow().len();
        {
            let mut sel = self.inner.selection.selected.borrow_mut();
            sel.clear();
            for k in 0..n {
                sel.insert(k);
            }
        }
        self.emit_selection_changed();
    }

    pub(crate) fn clear_selection(&self) {
        self.inner.selection.selected.borrow_mut().clear();
        self.emit_selection_changed();
    }

    pub(crate) fn set_cursor(&self, i: usize) {
        self.inner.selection.cursor.set(Some(i));
        self.sync_selection_classes();
    }

    pub(crate) fn cursor(&self) -> Option<usize> {
        self.inner.selection.cursor.get()
    }

    /// Select every item whose bounds intersect `rect` (x, y, w, h in the
    /// `Fixed`'s coordinate space). When `additive`, keep the prior selection.
    pub(crate) fn rubber_band_to(&self, rect: (f64, f64, f64, f64), additive: bool) {
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

    /// Move the keyboard cursor by (`dx`, `dy`) in {-1, 0, 1}. `extend` grows
    /// the selection from the anchor; `move_only` leaves the selection alone.
    pub(crate) fn move_cursor(&self, dx: i32, dy: i32, extend: bool, move_only: bool) {
        let n = self.inner.layout.items.borrow().len();
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
        let rects = self.inner.layout.item_rects.borrow();
        let &(fx, fy, _, _) = rects.get(cur)?;
        let cands: Vec<(usize, (f64, f64))> =
            rects.iter().enumerate().map(|(i, &(x, y, _, _))| (i, (x, y))).collect();
        crate::arrange::nearest_in_direction((fx, fy), &cands, dx, dy)
    }

    fn current_columns(&self) -> usize {
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
    fn item_at(&self, x: f64, y: f64) -> Option<usize> {
        // Scan back to front: when rectangles overlap (dragged-together icons
        // in Free mode), the last-placed one is drawn on top and wins the hit.
        for (i, &(ix, iy, iw, ih)) in self.inner.layout.item_rects.borrow().iter().enumerate().rev() {
            if x >= ix && x < ix + iw && y >= iy && y < iy + ih {
                return Some(i);
            }
        }
        None
    }

    pub(crate) fn items(&self) -> Vec<IconItem> {
        self.inner.layout.items.borrow().clone()
    }

    fn rebuild_arranger(&self) {
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

    /// Recompute positions, move every item, and cache the item rectangles.
    pub(crate) fn relayout(&self) {
        let items = self.inner.layout.items.borrow();
        if items.is_empty() {
            self.inner.layout.item_rects.borrow_mut().clear();
            self.inner.layout.fixed.set_size_request(0, 0);
            return;
        }
        let fallback_w = (self.inner.layout.icon_size.get() + 12) as f64;
        let fallback_h = (self.inner.layout.icon_size.get() + 40) as f64;
        let boxes: Vec<ItemBox> = items
            .iter()
            .map(|it| {
                let (w, h) = it.measure();
                ItemBox {
                    w: if w > 1.0 { w } else { fallback_w },
                    h: if h > 1.0 { h } else { fallback_h },
                }
            })
            .collect();
        let page = self.inner.layout.scroller.hadjustment().page_size();
        let viewport_w = if page > 1.0 { page } else { self.inner.layout.scroller.width().max(1) as f64 };
        if matches!(self.inner.layout.mode.get(), LayoutMode::Free) {
            self.rebuild_arranger();
        }
        let placement = self.inner.layout.arrange.borrow().place(&boxes, viewport_w);
        let mut rects = Vec::with_capacity(items.len());
        for ((it, &(x, y)), b) in items.iter().zip(placement.positions.iter()).zip(boxes.iter()) {
            self.inner.layout.fixed.move_(it.widget(), x, y);
            rects.push((x, y, b.w, b.h));
        }
        *self.inner.layout.item_rects.borrow_mut() = rects;
        self.inner
            .layout
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
    use crate::item::IconItem;
    use crate::types::IconKind;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

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
            })
            .collect()
    }

    /// All widget behaviour in one test: GTK panics if `gtk::init` runs on two
    /// threads, and libtest gives every `#[test]` its own thread, so the crate
    /// keeps exactly one GTK test.
    #[test]
    fn gtk_widget_behavior() {
        if gtk::init().is_err() {
            return;
        }

        // --- font_probe: this machine has no Nerd Font installed (verified
        // via `fc-list` while investigating the hex-tofu glyph bug), so
        // detection must report false. A flip to true would mean detection
        // silently broke and icon-less views would show tofu again.
        assert!(!crate::font_probe::nerd_font_available());

        // --- IconItem: selected class toggles ---
        let it = IconItem::new(&entries(1)[0], 64, &opusdm_ipc::IconSetId::None);
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
        };
        let long = IconItem::new(&long_name, 64, &opusdm_ipc::IconSetId::None);
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

        // --- IconItem: a tall thumbnail does not change the item's size ---
        // The thumbnail is a `gtk::Image` at `pixel_size`, so a portrait
        // texture measures the same as the glyph. Otherwise a thumbnail
        // loading in asynchronously would grow the item and, via
        // `FreeArrange`'s max-of-all pitch, re-space the whole desktop grid.
        let plain = IconItem::new(&entries(1)[0], 64, &opusdm_ipc::IconSetId::None);
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

        // --- IconField: set_icon_set rebuilds with real icons ---
        let _env_guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let icons_dir = td.path().join("opusdm").join("themes").join("test-theme").join("icons");
        std::fs::create_dir_all(&icons_dir).unwrap();
        std::fs::write(icons_dir.join("text.png"), TEST_PNG_1X1).unwrap();
        let custom = opusdm_ipc::IconSetId::Custom("test-theme".to_string());

        f.set_icon_set(custom.clone());
        assert_eq!(f.entry_count(), 3);
        f.set_icon_set(opusdm_ipc::IconSetId::None);
        assert_eq!(f.entry_count(), 3);

        // --- IconItem: a real icon set shows the image, not the glyph ---
        let themed = IconItem::new(&entries(1)[0], 64, &custom);
        assert!(themed.shows_image());
        let plain_glyph = IconItem::new(&entries(1)[0], 64, &opusdm_ipc::IconSetId::None);
        assert!(!plain_glyph.shows_image());

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
        }]);
        // "rs" is not previewable, so no job is queued and nothing panics.
        f_on.set_icon_size(200);

        // --- IconField: inter-window drag is off in Free mode ---
        let fd = IconField::new();
        assert!(fd.inter_window_drag_enabled()); // Grid default
        fd.set_mode(LayoutMode::Free);
        assert!(!fd.inter_window_drag_enabled());
        fd.set_mode(LayoutMode::Grid);
        assert!(fd.inter_window_drag_enabled());

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
}
