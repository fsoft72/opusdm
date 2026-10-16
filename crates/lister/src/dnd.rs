//! Drag-and-drop wiring for the Lister views. The `text/uri-list` codec lives
//! in `opusdm_iconview::dnd`; this module wires it to `ToHub::RunJob`.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use opusdm_iconview::IconField;
use opusdm_ipc::Op;
use std::path::PathBuf;
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
            submit_dropped(&ui, op, srcs, dest_dir);
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
        submit_dropped(&ui, op, srcs, dest_dir);
    });
}

/// Sources of a drop split into real paths and entries of archives: the
/// plain paths, then one `(archive, inner paths)` group per archive.
fn _split_archive_srcs(srcs: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<(PathBuf, Vec<String>)>) {
    let mut plain = Vec::new();
    let mut groups: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for src in srcs {
        let Some((arch, inner)) = crate::archive::parse_archive_path(&src) else {
            plain.push(src);
            continue;
        };
        match groups.iter_mut().find(|(a, _)| *a == arch) {
            Some((_, items)) => items.push(inner),
            None => groups.push((arch, vec![inner])),
        }
    }
    (plain, groups)
}

/// Submit the job of a drop. A source dragged out of an archive is a virtual
/// path the job runner cannot read, so those entries are extracted first, off
/// the GTK thread, into a scratch folder that lives until the job is done. An
/// archive is read-only, so a Move runs as a Copy.
fn submit_dropped(ui: &Rc<Ui>, op: Op, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf>) {
    let (mut ready, groups) = _split_archive_srcs(srcs);
    if groups.is_empty() {
        crate::job_submit::submit_job(ui, op, ready, dest_dir, opusdm_ipc::JobOptions::default());
        return;
    }
    let scratch = match tempfile::Builder::new().prefix("opusdm-extract-").tempdir() {
        Ok(dir) => dir,
        Err(e) => {
            crate::actions::flash(ui, &format!("cannot create a temporary folder: {e}"));
            return;
        }
    };
    let op = match op {
        Op::Move => Op::Copy,
        other => other,
    };
    crate::actions::flash(ui, "extracting from the archive...");
    let root = scratch.path().to_path_buf();
    let extract = move || -> std::io::Result<Vec<PathBuf>> {
        let mut extracted = Vec::new();
        for (n, (arch, items)) in groups.iter().enumerate() {
            // One folder per archive, so equal entry names never collide.
            let dir = root.join(n.to_string());
            extracted.extend(crate::archive::extract_items_to_dir(arch, items, &dir)?);
        }
        Ok(extracted)
    };
    let ui = ui.clone();
    crate::background::run_blocking(extract, move |result| match result {
        Ok(extracted) => {
            // The drop makes this Lister the requester: one JobDone comes back.
            crate::archive::keep_scratch(scratch, 1);
            ready.extend(extracted);
            crate::job_submit::submit_job(&ui, op, ready, dest_dir, opusdm_ipc::JobOptions::default());
        }
        Err(e) => crate::actions::flash(&ui, &format!("extraction failed: {e}")),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_entries_are_grouped_per_archive() {
        let srcs = vec![
            PathBuf::from("/a/x.zip#dir/one"),
            PathBuf::from("/tmp/plain.txt"),
            PathBuf::from("/a/y.zip#two"),
            PathBuf::from("/a/x.zip#three"),
        ];
        let (plain, groups) = _split_archive_srcs(srcs);
        assert_eq!(plain, vec![PathBuf::from("/tmp/plain.txt")]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], (PathBuf::from("/a/x.zip"), vec!["dir/one".to_string(), "three".to_string()]));
        assert_eq!(groups[1], (PathBuf::from("/a/y.zip"), vec!["two".to_string()]));
    }
}
