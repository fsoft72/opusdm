//! Drag-and-drop wiring for the Lister views. The `text/uri-list` codec lives
//! in `opusdm_iconview::dnd`; this module wires it to `ToHub::RunJob`.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use opusdm_iconview::IconField;
use opusdm_ipc::Op;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

thread_local! {
    /// The files being dragged out of a column view in this process, and that
    /// view. A drop on the same view can target one of its folders.
    static LIST_DRAG: RefCell<Option<(gtk::glib::WeakRef<gtk::ColumnView>, Vec<PathBuf>)>> = const { RefCell::new(None) };
    /// The row widget lit as the drop target of that drag.
    static LIST_DROP_ROW: RefCell<Option<gtk::glib::WeakRef<gtk::Widget>>> = const { RefCell::new(None) };
}

/// A drag of `paths` starts in `view`.
pub fn begin_list_drag(view: &gtk::ColumnView, paths: Vec<PathBuf>) {
    LIST_DRAG.with(|d| *d.borrow_mut() = Some((view.downgrade(), paths)));
}

/// The drag that started in a column view is over: forget it and unlight the row.
pub fn end_list_drag() {
    LIST_DRAG.with(|d| *d.borrow_mut() = None);
    _light_row(None);
}

/// The files being dragged out of `view`, if a drag from it is running.
fn _list_drag_paths(view: &gtk::ColumnView) -> Option<Vec<PathBuf>> {
    LIST_DRAG.with(|d| {
        let d = d.borrow();
        let (weak, paths) = d.as_ref()?;
        (weak.upgrade().as_ref() == Some(view)).then(|| paths.clone())
    })
}

/// Light `row` as the drop target and unlight the previous one (`None` clears).
fn _light_row(row: Option<&gtk::Widget>) {
    LIST_DROP_ROW.with(|cur| {
        let mut cur = cur.borrow_mut();
        let old = cur.as_ref().and_then(|w| w.upgrade());
        if old.as_ref() == row {
            return;
        }
        if let Some(old) = old {
            crate::columns::set_row_drop_target(&old, false);
        }
        if let Some(row) = row {
            crate::columns::set_row_drop_target(row, true);
        }
        *cur = row.map(|r| r.downgrade());
    });
}

/// The folder row under `y` that `dragged` may be dropped on, with its path
/// and widget: a directory that is not itself dragged.
fn _folder_at(ui: &Ui, view: &gtk::ColumnView, y: f64, dragged: &[PathBuf]) -> Option<(PathBuf, gtk::Widget)> {
    let (pos, widget) = crate::columns::row_at_y(view, y)?;
    let row = ui.row_at(pos)?;
    let path = ui.cwd_path().join(&row.name);
    (row.is_dir && !dragged.contains(&path)).then_some((path, widget))
}

/// The action a drag over the column view proposes at `y`. A drag that
/// started in this view is Move (Copy with Ctrl) over a folder and refused
/// elsewhere; any other drag is Copy, Move with Shift.
fn _list_hover_action(weak: &Weak<Ui>, view: &gtk::ColumnView, y: f64) -> gdk::DragAction {
    let Some(dragged) = _list_drag_paths(view) else {
        let shift = opusdm_iconview::dnd::shift_held(view);
        return if shift { gdk::DragAction::MOVE } else { gdk::DragAction::COPY };
    };
    let Some(ui) = weak.upgrade() else {
        opusdm_ipc::err_log!("opusdm-lister", "column view drop hover: the Lister window is gone");
        return gdk::DragAction::empty();
    };
    let folder = _folder_at(&ui, view, y, &dragged);
    _light_row(folder.as_ref().map(|(_, w)| w));
    match folder {
        None => gdk::DragAction::empty(),
        Some(_) if opusdm_iconview::dnd::ctrl_held(view) => gdk::DragAction::COPY,
        Some(_) => gdk::DragAction::MOVE,
    }
}

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
        let weak = Rc::downgrade(ui);
        target.connect_enter(move |_, x, y| {
            crate::dlog!("column view drop: enter at ({x}, {y})");
            _list_hover_action(&weak, &view, y)
        });
    }
    {
        let view = view.clone();
        let weak = Rc::downgrade(ui);
        target.connect_motion(move |_, _x, y| _list_hover_action(&weak, &view, y));
    }
    target.connect_leave(|_| {
        crate::dlog!("column view drop: leave");
        _light_row(None);
    });
    {
        let weak = Rc::downgrade(ui);
        let view = view.clone();
        target.connect_drop(move |t, value, x, y| {
            let Some(ui) = weak.upgrade() else {
                opusdm_ipc::err_log!("opusdm-lister", "column view drop: the Lister window is gone");
                return false;
            };
            _light_row(None);
            if let Some(dragged) = _list_drag_paths(&view) {
                // A drag from this view: only a folder row takes it.
                let Some((dest, _)) = _folder_at(&ui, &view, y, &dragged) else {
                    crate::dlog!("column view drop: own drag dropped away from a folder, ignored");
                    return false;
                };
                drop_on_folder(&ui, dragged, dest, opusdm_iconview::dnd::ctrl_held(&view));
                return true;
            }
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
    let weak = Rc::downgrade(ui);
    field.connect_drop_uris(move |srcs, target, shift| {
        let Some(ui) = weak.upgrade() else { return };
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

/// Wire the drop of files dragged out of the icon view onto one of its
/// folders: Move, or Copy with Ctrl held. Dragging a file out of an archive
/// view is handled by `submit_dropped` as for any other drop.
pub fn attach_field_folders(ui: &Rc<Ui>, field: &IconField) {
    let weak = Rc::downgrade(ui);
    let field_weak = field.downgrade();
    field.connect_drop_on_folder(move |srcs, folder, copy| {
        let Some(ui) = weak.upgrade() else {
            opusdm_ipc::err_log!("opusdm-lister", "icon field folder drop: the Lister window is gone");
            return;
        };
        let Some(entry) = field_weak.upgrade().and_then(|f| f.entry(folder)) else {
            opusdm_ipc::err_log!("opusdm-lister", "icon field folder drop: folder {folder} is no longer listed");
            return;
        };
        drop_on_folder(&ui, srcs, entry.path, copy);
    });
}

/// Drop `srcs` on the folder `dest`: Move, or Copy when `copy` is set.
pub fn drop_on_folder(ui: &Rc<Ui>, srcs: Vec<PathBuf>, dest: PathBuf, copy: bool) {
    if crate::archive::parse_archive_path(&dest).is_some() {
        crate::actions::flash(ui, "cannot drop into an archive");
        return;
    }
    let srcs = _without_dest(srcs, &dest);
    crate::dlog!("folder drop: copy={copy} srcs={srcs:?} dest={dest:?}");
    if srcs.is_empty() {
        crate::actions::flash(ui, "nothing to drop there");
        return;
    }
    let op = if copy { Op::Copy } else { Op::Move };
    submit_dropped(ui, op, srcs, Some(dest));
}

/// `srcs` without the folder `dest` itself and the folders that contain it:
/// a folder cannot be dropped into itself.
fn _without_dest(srcs: Vec<PathBuf>, dest: &std::path::Path) -> Vec<PathBuf> {
    srcs.into_iter().filter(|s| !dest.starts_with(s)).collect()
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
            let tag = crate::archive::keep_scratch(scratch, 1);
            ready.extend(extracted);
            crate::job_submit::submit_tagged_job(&ui, op, ready, dest_dir, opusdm_ipc::JobOptions::default(), tag);
        }
        Err(e) => crate::actions::flash(&ui, &format!("extraction failed: {e}")),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_never_dropped_into_itself_or_below_itself() {
        let srcs = vec![PathBuf::from("/a/x"), PathBuf::from("/a/y"), PathBuf::from("/a")];
        assert_eq!(_without_dest(srcs, std::path::Path::new("/a/x")), vec![PathBuf::from("/a/y")]);
    }

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
