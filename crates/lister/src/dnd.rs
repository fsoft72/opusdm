//! Drag-and-drop wiring for the Lister views. The `text/uri-list` codec lives
//! in `opusdm_iconview::dnd`; this module wires it to `ToHub::RunJob`.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use opusdm_iconview::IconField;
use opusdm_ipc::{Op, ToHub};
use std::rc::Rc;

/// Attach a drop target to the column view. The drag source lives per-cell
/// in `columns.rs`, since it needs the row actually under the pointer rather
/// than the (possibly stale) selection model - see the comment there.
pub fn attach(ui: &Rc<Ui>, view: &gtk::ColumnView) {
    // GDK's builtin mime table deserializes `text/uri-list` into
    // `GdkFileList`, not a plain string - requesting `glib::Type::STRING`
    // left no format in common with a source that only advertises
    // `text/uri-list`, so the drop silently failed with `NoTarget` before
    // ever reaching `connect_drop`.
    let target = gtk::DropTarget::new(
        gdk::FileList::static_type(),
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    // `connect_enter`/`connect_motion` must each return exactly one action -
    // an OR'd bitmask trips a GtkDropTarget-CRITICAL ("did not return a
    // unique preferred action"). Recompute live so the proposed action (and
    // its cursor) tracks Shift as it's pressed/released while hovering.
    {
        let view = view.clone();
        target.connect_enter(move |_, x, y| {
            let shift = opusdm_iconview::dnd::shift_held(&view);
            crate::dlog!("column view drop: enter at ({x}, {y}) shift={shift}");
            if shift { gdk::DragAction::MOVE } else { gdk::DragAction::COPY }
        });
    }
    {
        let view = view.clone();
        target.connect_motion(move |_, _x, _y| {
            let shift = opusdm_iconview::dnd::shift_held(&view);
            if shift { gdk::DragAction::MOVE } else { gdk::DragAction::COPY }
        });
    }
    target.connect_leave(|_| {
        crate::dlog!("column view drop: leave");
    });
    {
        let ui = ui.clone();
        let view = view.clone();
        target.connect_drop(move |t, value, x, y| {
            crate::dlog!("column view drop: fired at ({x}, {y}), value type={:?}", value.type_());
            let Ok(file_list) = value.get::<gdk::FileList>() else {
                crate::dlog!("column view drop: value is not a FileList");
                return false;
            };
            let srcs: Vec<_> = file_list.files().into_iter().filter_map(|f| f.path()).collect();
            crate::dlog!("column view drop: srcs={srcs:?}");
            if srcs.is_empty() {
                crate::dlog!("column view drop: no paths parsed from uri-list");
                return false;
            }
            let event_shift = t
                .current_event()
                .map(|e| e.modifier_state().contains(gdk::ModifierType::SHIFT_MASK))
                .unwrap_or(false);
            let shift = opusdm_iconview::dnd::shift_held(&view);
            crate::dlog!("column view drop: event_shift={event_shift} live_shift={shift}");
            let op = if shift { Op::Move } else { Op::Copy };
            let dest_dir = Some(ui.cwd_path());
            crate::dlog!("column view drop: op={op:?} srcs={srcs:?} dest={dest_dir:?}");
            let _ = ui.client.send(&ToHub::RunJob { op, srcs, dest_dir });
            true
        });
    }
    view.add_controller(target);
}

/// Wire an `IconField`'s drop callback to `ToHub::RunJob`, mirroring `attach`
/// for the column view. Shift held at drop time means move, otherwise copy.
pub fn attach_field(ui: &Rc<Ui>, field: &IconField) {
    let ui = ui.clone();
    field.connect_drop_uris(move |srcs, target, shift| {
        crate::dlog!("icon field drop: target={target} shift={shift} srcs={srcs:?}");
        if srcs.is_empty() {
            crate::dlog!("icon field drop: no paths, aborting");
            return;
        }
        let op = if shift { Op::Move } else { Op::Copy };
        let dest_dir = Some(ui.cwd_path());
        crate::dlog!("icon field drop: op={op:?} srcs={srcs:?} dest={dest_dir:?}");
        let _ = ui.client.send(&ToHub::RunJob { op, srcs, dest_dir });
    });
}
