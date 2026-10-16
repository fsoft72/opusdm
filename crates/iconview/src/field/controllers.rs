//! Event controllers for `IconField`: selection clicks, rubber-band drag,
//! keyboard navigation, DnD source/target, context menu, scroll zoom, and
//! free-mode repositioning.

use super::IconField;
use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Attach all event controllers to `field.inner.layout.fixed`.
pub(super) fn install_controllers(field: &IconField) {
    install_click_controller(field);
    install_rubber_band_controller(field);
    install_key_controller(field);
    install_dnd_controllers(field);
    install_context_menu_controller(field);
    install_zoom_controller(field);
    install_reposition_controller(field);
}

/// Left-click selection controller.
fn install_click_controller(field: &IconField) {
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_PRIMARY);
    let weak = field.downgrade();
    click.connect_pressed(move |g, n_press, x, y| {
        let Some(this) = weak.upgrade() else { return };
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
}

/// Rubber-band selection: a drag that begins on empty space.
fn install_rubber_band_controller(field: &IconField) {
    let drag = gtk::GestureDrag::new();
    drag.set_button(gtk::gdk::BUTTON_PRIMARY);
    let start: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
    let weak = field.downgrade();
    let start_b = start.clone();
    drag.connect_drag_begin(move |_, x, y| {
        let Some(this) = weak.upgrade() else { return };
        if this.item_at(x, y).is_some() {
            start_b.set(None);
            return;
        }
        start_b.set(Some((x, y)));
    });
    let weak = field.downgrade();
    let start_u = start.clone();
    drag.connect_drag_update(move |g, ox, oy| {
        let Some(this) = weak.upgrade() else { return };
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
}

/// Keyboard navigation controller.
fn install_key_controller(field: &IconField) {
    field.inner.layout.fixed.set_focusable(true);
    let key = gtk::EventControllerKey::new();
    let weak = field.downgrade();
    key.connect_key_pressed(move |_, keyval, _, state| {
        let Some(this) = weak.upgrade() else { return gtk::glib::Propagation::Proceed };
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
                    if let Some(it) = this.item_at_index(i) {
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
}

/// Drag source and Drop target controllers.
fn install_dnd_controllers(field: &IconField) {
    // Drag source: export the selection as a uri-list for other windows.
    let src = gtk::DragSource::new();
    src.set_actions(gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
    let weak = field.downgrade();
    src.connect_prepare(move |src, x, y| {
        let this = weak.upgrade()?;
        crate::dbg_log!("icon field drag prepare at ({x}, {y})");
        // In Free mode a primary-button drag is an internal reposition, not
        // an inter-window file drag; leaving this on would let the DnD
        // machinery steal the gesture and snap the icon back. Ctrl held
        // turns it into a file drag (onto a Lister, a terminal, ...).
        let ctrl = src.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if !this.inter_window_drag_enabled(ctrl) {
            crate::dbg_log!("icon field drag prepare: inter-window drag disabled");
            return None;
        }
        // The Ctrl+press that starts the drag toggled the icon under the
        // pointer, which unselects an icon that was selected: the dragged
        // icon always travels with the rest of the selection.
        if let Some(hit) = this.item_at(x, y).filter(|i| ctrl && !this.selected_indices().contains(i)) {
            this.toggle(hit);
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
    let weak = field.downgrade();
    target.connect_drop(move |t, value, x, y| {
        let Some(this) = weak.upgrade() else { return false };
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
}

/// Right-click context-menu controller.
fn install_context_menu_controller(field: &IconField) {
    let rclick = gtk::GestureClick::new();
    rclick.set_button(gtk::gdk::BUTTON_SECONDARY);
    let weak = field.downgrade();
    rclick.connect_pressed(move |_, _, x, y| {
        let Some(this) = weak.upgrade() else { return };
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
}

/// Ctrl+scroll zoom controller.
fn install_zoom_controller(field: &IconField) {
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let weak = field.downgrade();
    scroll.connect_scroll(move |c, _dx, dy| {
        let Some(this) = weak.upgrade() else { return gtk::glib::Propagation::Proceed };
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
}

/// Internal reposition drag (Free mode only), starting on any item.
fn install_reposition_controller(field: &IconField) {
    let rdrag = gtk::GestureDrag::new();
    rdrag.set_button(gtk::gdk::BUTTON_PRIMARY);
    let weak = field.downgrade();
    rdrag.connect_drag_begin(move |g, x, y| {
        let Some(this) = weak.upgrade() else { return };
        // Ctrl+drag is a file drag to other windows, never a reposition.
        if g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            this.inner.layout.reposition_active.set(false);
            return;
        }
        this.begin_reposition(x, y);
    });
    let weak = field.downgrade();
    rdrag.connect_drag_update(move |_, ox, oy| {
        let Some(this) = weak.upgrade() else { return };
        this.update_reposition(ox, oy);
    });
    let weak = field.downgrade();
    rdrag.connect_drag_end(move |_, ox, oy| {
        let Some(this) = weak.upgrade() else { return };
        this.end_reposition(ox, oy);
    });
    field.inner.layout.fixed.add_controller(rdrag);
}
