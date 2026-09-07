//! Drag-and-drop wiring for the Lister views. The `text/uri-list` codec lives
//! in `opusdm_iconview::dnd`; this module wires it to `ToHub::RunJob`.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_iconview::dnd::{paths_to_uri_list, uris_to_paths};
use opusdm_iconview::IconField;
use opusdm_ipc::{Op, ToHub};
use std::rc::Rc;

/// Attach a drag source and a drop target to the column view.
pub fn attach(ui: &Rc<Ui>, view: &gtk::ColumnView) {
    let src = gtk::DragSource::new();
    src.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    {
        let ui = ui.clone();
        src.connect_prepare(move |_s, _x, _y| {
            let paths = ui.selected_paths();
            if paths.is_empty() {
                return None;
            }
            let payload = paths_to_uri_list(&paths);
            Some(gdk::ContentProvider::for_bytes(
                "text/uri-list",
                &glib::Bytes::from(payload.as_bytes()),
            ))
        });
    }
    view.add_controller(src);

    let target =
        gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::COPY | gdk::DragAction::MOVE);
    {
        let ui = ui.clone();
        target.connect_drop(move |t, value, _x, _y| {
            let Ok(text) = value.get::<String>() else {
                return false;
            };
            let srcs = uris_to_paths(&text);
            if srcs.is_empty() {
                return false;
            }
            let shift = t
                .current_event()
                .map(|e| e.modifier_state().contains(gdk::ModifierType::SHIFT_MASK))
                .unwrap_or(false);
            let op = if shift { Op::Move } else { Op::Copy };
            let dest_dir = Some(ui.cwd_path());
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
    field.connect_drop_uris(move |srcs, _target, shift| {
        if srcs.is_empty() {
            return;
        }
        let op = if shift { Op::Move } else { Op::Copy };
        let dest_dir = Some(ui.cwd_path());
        let _ = ui.client.send(&ToHub::RunJob { op, srcs, dest_dir });
    });
}
