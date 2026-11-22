//! The context menu of one entry, the same in a Lister and on the Desktop.

use crate::host::{targets_for, ExtraPlace, FileActionHost, MenuSink, MenuTarget};
use opusdm_ipc::filetypes::FileType;
use opusdm_ipc::function::{Function, FunctionId};
use opusdm_ipc::Op;
use std::path::PathBuf;
use std::rc::Rc;

/// The paths an action on `target` applies to: the selection when it holds
/// `target`, else `target` alone. Read when the item is clicked.
fn targets(host: &Rc<dyn FileActionHost>, target: &MenuTarget) -> Vec<PathBuf> {
    targets_for(host.selected_paths(), &target.path)
}

/// The question of the menu Delete: the name of `target` when it is the only
/// entry, else the count of the `count` entries.
fn delete_question(target: &MenuTarget, count: usize) -> String {
    if count == 1 { return format!("Delete \"{}\" permanently?", target.name); }
    crate::keys::delete_message(count, true)
}

/// Copy or move `target` to the DEST Lister, when there is one.
fn transfer(host: &dyn FileActionHost, op: Op, target: &MenuTarget) {
    if let Err(msg) = host.dest_check() {
        host.notify(&msg);
        return;
    }
    host.submit_job(op, vec![target.path.clone()], None);
}

/// Add an item whose action gets the host and the target.
fn add(menu: &dyn MenuSink, host: &Rc<dyn FileActionHost>, target: &Rc<MenuTarget>, title: &str, action: impl Fn(&Rc<dyn FileActionHost>, &MenuTarget) + 'static) {
    let (host, target) = (host.clone(), target.clone());
    menu.item(title, Box::new(move || action(&host, &target)));
}

/// Fill `menu` with the items of `target`: the host's own top items, Copy,
/// Rename, Move, Delete, Change Permissions, the "Open With" entries of the
/// file type, Open with, Compress, Extract, Encrypt / Decrypt, Open in
/// Viewer, Set as Desktop Background, then the host's own bottom items.
pub fn fill_entry_menu(menu: &dyn MenuSink, host: &Rc<dyn FileActionHost>, target: MenuTarget) {
    let target = Rc::new(target);
    host.extra_items(ExtraPlace::Top, menu, &target);

    // Quick actions: Copy, Rename, Move, Delete.
    add(menu, host, &target, "Copy (Source -> Dest)", |h, t| transfer(&**h, Op::Copy, t));
    if !target.elsewhere {
        add(menu, host, &target, "Rename...", |h, t| h.rename(&t.path, &t.name));
        add(menu, host, &target, "Move (Source -> Dest)", |h, t| transfer(&**h, Op::Move, t));
        add(menu, host, &target, "Delete...", |h, t| {
            let paths = targets_for(h.own_selected_paths(), &t.path);
            h.confirm_delete(&delete_question(t, paths.len()), paths, true);
        });
    }
    add(menu, host, &target, "Change Permissions...", |h, t| crate::permissions::open_permissions(h, targets(h, t)));

    // File type "Open With" entries: one per function of the type the file matches.
    if let Some(file_type) = host.file_type(&target) {
        let entries = menu_entries(&file_type, &host.functions());
        if !entries.is_empty() {
            menu.separator();
        }
        for (function, label) in entries {
            add(menu, host, &target, &format!("Open With: {label}"), move |h, t| h.run_on_target(t, &function));
        }
    }

    add(menu, host, &target, "Open with...", |h, t| crate::open_with::open_with(h, targets(h, t)));
    add(menu, host, &target, "Compress...", |h, t| crate::compress::open_compress(h, targets(h, t)));
    // Offered on a supported archive file (not on one already inside an archive).
    if !target.is_dir && !crate::extract::extractable(vec![target.path.clone()], |p| host.inside_archive(p)).is_empty() {
        add(menu, host, &target, "Extract archive", |h, t| crate::extract::open_extract(h, targets(h, t)));
        add(menu, host, &target, "Extract here", |h, t| crate::extract::extract_here(h, targets(h, t)));
    }
    add(menu, host, &target, "Encrypt...", |h, t| crate::crypt::open_crypt(h, true, targets(h, t)));
    if !target.is_dir && target.name.to_ascii_lowercase().ends_with(opusdm_fsops::crypt::ENC_SUFFIX) {
        add(menu, host, &target, "Decrypt...", |h, t| crate::crypt::open_crypt(h, false, targets(h, t)));
    }

    menu.separator();
    add(menu, host, &target, "Open in Viewer", |h, t| h.open_in_viewer(targets(h, t)));
    if !target.is_dir && opusdm_ipc::is_background_media(&target.path) {
        add(menu, host, &target, "Set as Desktop Background", |h, t| h.set_background(&t.path));
    }
    host.extra_items(ExtraPlace::Bottom, menu, &target);
}

/// The "Open With" entries of `file_type`: reference and label (the
/// function name, or the command for an inline bash or exec one, the latter
/// with no `exec:` prefix). Saved functions that no longer exist and inline
/// commands that cannot run are left out.
pub fn menu_entries(file_type: &FileType, functions: &[Function]) -> Vec<(FunctionId, String)> {
    file_type
        .actions
        .menu
        .iter()
        .filter(|id| match opusdm_ipc::funcref::parse(id) {
            opusdm_ipc::funcref::FuncRef::Saved(_) => functions.iter().any(|f| &f.id == *id),
            inline => opusdm_ipc::funcref::problem(&inline).is_none(),
        })
        .map(|id| (id.clone(), menu_label(id, functions)))
        .collect()
}

/// The label of an Open With entry. Like [`opusdm_ipc::funcref::title`], but an
/// exec command shows its command line with no `exec:` prefix.
fn menu_label(reference: &str, functions: &[Function]) -> String {
    match opusdm_ipc::funcref::parse(reference) {
        opusdm_ipc::funcref::FuncRef::Exec { line } => line,
        _ => opusdm_ipc::funcref::title(reference, functions),
    }
}

#[cfg(test)]
mod tests;
