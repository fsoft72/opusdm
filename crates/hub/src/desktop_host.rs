//! The Desktop as the host of the shared file actions
//! (`opusdm_fileactions`), so a Desktop icon gets the same context menu and
//! keys as a file in a Lister.
//!
//! Ownership: a `DesktopHost` holds a strong `Rc<Widgets>`, as every other
//! Desktop menu item already does. It is made when the icon menu is filled
//! (or a key is pressed) and lives as long as the items of that menu and the
//! dialogs they open. The file keys of a Lister are wired on the Desktop
//! window by [`attach_keys`]. `Widgets` itself is owned by the GTK side for the
//! whole run (see `toolbar.rs`).

use super::toolbar::Widgets;
use crate::server::HubCmd;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_fileactions::keys::FileKey;
use opusdm_fileactions::{ExtraPlace, FileActionHost, MenuSink, MenuTarget};
use opusdm_ipc::filetypes::FileType;
use opusdm_ipc::function::{Function, FunctionId};
use opusdm_ipc::Op;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Log tag of the Desktop file actions.
const TAG: &str = "hub";

/// The stand-alone image / media viewer, started by "Open in Viewer".
const VIEWER_BIN: &str = "opusdm-viewer";

/// What the Desktop tells the user when a button Copy / Move has nowhere to go.
pub(crate) const NO_DEST: &str = "no DEST Lister set";

/// What the Desktop tells the user when every DEST Lister shows a remote folder.
pub(crate) const REMOTE_DEST: &str = "the DEST Lister shows a remote folder: copy from a Lister there";

/// The file actions host of the Desktop icon field.
pub(crate) struct DesktopHost {
    widgets: Rc<Widgets>,
    /// The Desktop folder.
    dir: PathBuf,
    /// The icon the menu was opened on (`None` for a key press).
    entry: Option<opusdm_iconview::IconEntry>,
    /// Leave the left-out icons out of the selection: a key that changes or
    /// removes files (Move, Delete, Rename, Cut) must not reach the real
    /// file elsewhere, as the menu does not offer those items on them.
    own_files_only: bool,
}

impl DesktopHost {
    /// The host of the Desktop folder `dir`, for the menu of `entry` (or a
    /// key press when `None`).
    pub(crate) fn rc(widgets: &Rc<Widgets>, dir: PathBuf, entry: Option<opusdm_iconview::IconEntry>) -> Rc<dyn FileActionHost> {
        Rc::new(Self { widgets: widgets.clone(), dir, entry, own_files_only: false })
    }

    /// The host of a key press; `own_files_only` leaves the left-out icons
    /// out of the selection.
    fn for_key(widgets: &Rc<Widgets>, dir: PathBuf, own_files_only: bool) -> Self {
        Self { widgets: widgets.clone(), dir, entry: None, own_files_only }
    }

    /// The selected entries of the icon field.
    fn selected_entries(&self) -> Vec<opusdm_iconview::IconEntry> {
        let Some(field) = self.widgets.desktop.field.borrow().clone() else { return Vec::new() };
        field.selected_indices().into_iter().filter_map(|i| field.entry(i)).collect()
    }

    /// Send `cmd` to the hub server.
    fn send(&self, cmd: HubCmd) {
        opusdm_hub::channels::send_or_log(&self.widgets.cmd_tx, cmd, "hub command");
    }

    /// The top items of an icon: Open, Put Away (left-out icons), Cut and
    /// Copy to the clipboard (Cut not on a left-out icon), as before the menu
    /// was shared.
    fn top_items(&self, menu: &dyn MenuSink, entry: &opusdm_iconview::IconEntry) {
        {
            let path = entry.path.clone();
            let opens_in_lister = opusdm_hub::leftout::opens_in_lister(entry);
            let widgets = self.widgets.clone();
            menu.item(
                "Open",
                Box::new(move || {
                    if opens_in_lister {
                        opusdm_hub::channels::send_or_log(&widgets.cmd_tx, HubCmd::SpawnRequested { start_path: Some(path.clone()) }, "hub command");
                    } else {
                        crate::desktop_windows::open_file(&widgets.primary_window(), &widgets.cmd_tx, &path);
                    }
                }),
            );
        }
        // Put Away (left-out icons only): forget the icon, never touch the file.
        if entry.left_out {
            let path = entry.path.clone();
            let widgets = self.widgets.clone();
            menu.item(
                "Put Away",
                Box::new(move || {
                    widgets.desktop.leftout.borrow_mut().remove(std::slice::from_ref(&path));
                    crate::desktop_windows::save_leftout(&widgets);
                    crate::desktop_windows::reload_desktop_field(&widgets);
                }),
            );
        }
        menu.separator();
        if !entry.left_out {
            let path = entry.path.clone();
            menu.item("Cut", Box::new(move || crate::desktop_ops::copy_or_cut(std::slice::from_ref(&path), true)));
        }
        let path = entry.path.clone();
        menu.item("Copy", Box::new(move || crate::desktop_ops::copy_or_cut(std::slice::from_ref(&path), false)));
        menu.separator();
    }
}

impl FileActionHost for DesktopHost {
    fn parent_window(&self) -> gtk::Window {
        self.widgets.primary_window().upcast()
    }

    fn notify(&self, text: &str) {
        opusdm_ipc::err_log!(TAG, "desktop: {text}");
        opusdm_ui::requester(&self.widgets.primary_window(), "Desktop", text, None, &[("OK", true)], |_| {});
    }

    fn folder(&self) -> PathBuf {
        self.dir.clone()
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        selectable_paths(&self.selected_entries(), self.own_files_only)
    }

    fn own_selected_paths(&self) -> Vec<PathBuf> {
        selectable_paths(&self.selected_entries(), true)
    }

    fn dest_check(&self) -> Result<(), String> {
        dest_check_of(&self.widgets.dest_dirs.borrow())
    }

    fn submit_job(&self, op: Op, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf>) {
        match dest_dir {
            Some(dest_dir) => self.send(HubCmd::RunJob { op, srcs, dest_dir }),
            None => self.send(HubCmd::RunJobToDests { op, srcs }),
        }
    }

    fn rename(&self, path: &Path, old_name: &str) {
        crate::desktop_ops::rename(&self.widgets.primary_window(), path.to_path_buf(), old_name);
    }

    fn confirm_delete(&self, message: &str, srcs: Vec<PathBuf>, permanent: bool) {
        crate::desktop_ops::confirm_delete(&self.widgets.primary_window(), &self.widgets.cmd_tx, message, srcs, self.dir.clone(), permanent);
    }

    fn functions(&self) -> Vec<Function> {
        self.widgets.functions.borrow().clone()
    }

    fn file_type(&self, target: &MenuTarget) -> Option<FileType> {
        opusdm_fileactions::file_type_in(&opusdm_hub::config_cache::filetypes(), target)
    }

    fn run_on_target(&self, target: &MenuTarget, id: &FunctionId) {
        self.send(HubCmd::RunFunctionOnPaths { id: id.clone(), paths: vec![target.path.clone()] });
    }

    fn open_in_viewer(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() { return self.notify("nothing selected"); }
        let exe = std::env::current_exe().ok();
        let bin = opusdm_ipc::instance::sibling_bin_in(exe.as_deref().and_then(Path::parent), VIEWER_BIN);
        // The hub's SIGCHLD reaper collects the viewer when it exits.
        if let Err(e) = std::process::Command::new(&bin).args(&paths).spawn() {
            self.notify(&format!("cannot start {}: {e}", bin.display()));
        }
    }

    fn set_background(&self, path: &Path) {
        match std::fs::canonicalize(path) {
            Ok(path) => self.send(HubCmd::SetBackground(path)),
            Err(e) => self.notify(&format!("cannot set the background {}: {e}", path.display())),
        }
    }

    fn extra_items(&self, place: ExtraPlace, menu: &dyn MenuSink, _target: &MenuTarget) {
        if place != ExtraPlace::Top { return; }
        let Some(entry) = &self.entry else { return };
        self.top_items(menu, entry);
    }
}

/// The selected Desktop paths an action may get: a program group icon has
/// a virtual `group:<id>` path, not a file, so it is left out; so are the
/// left-out icons when `own_files_only`.
fn selectable_paths(selected: &[opusdm_iconview::IconEntry], own_files_only: bool) -> Vec<PathBuf> {
    selected
        .iter()
        .filter(|e| !e.group && opusdm_ipc::groups::group_id(&e.path).is_none())
        .filter(|e| !(own_files_only && e.left_out))
        .map(|e| e.path.clone())
        .collect()
}

/// What the Desktop says when a key that changes files finds only left-out
/// icons selected.
const ONLY_LEFT_OUT: &str = "left-out icons stand for files elsewhere: use Put Away, or open their folder";

/// Whether `key` changes or removes the selected files themselves, so it
/// must not act on the real file behind a left-out icon.
fn changes_files(key: FileKey) -> bool {
    matches!(key, FileKey::Move | FileKey::Trash | FileKey::DeletePermanent | FileKey::Rename | FileKey::ClipCut)
}

/// Wire the file keys of a Lister (F2, F3, F5, F6, Delete, Shift+Delete,
/// Ctrl+C / X / V, Ctrl+N, Ctrl+Shift+N) on the Desktop `window`. Runs in
/// the capture phase, so F2 opens the Rename requester like in a Lister
/// instead of the icon field's inline edit; keys typed in a text field go
/// through untouched. The handler holds a strong `Rc<Widgets>`, like the
/// other Desktop handlers (`Widgets` lives for the whole run).
pub(crate) fn attach_keys(window: &gtk::ApplicationWindow, widgets: &Rc<Widgets>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let widgets = widgets.clone();
    let win = window.clone();
    keys.connect_key_pressed(move |_, keyval, _, state| {
        let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
        let alt = state.contains(gtk::gdk::ModifierType::ALT_MASK);
        let Some(key) = opusdm_fileactions::keys::resolve(keyval, ctrl, shift, alt) else { return gtk::glib::Propagation::Proceed };
        if gtk::prelude::GtkWindowExt::focus(&win).is_some_and(|w| w.is::<gtk::Editable>()) {
            return gtk::glib::Propagation::Proceed;
        }
        run_key(&widgets, key);
        gtk::glib::Propagation::Stop
    });
    window.add_controller(keys);
}

/// Run the file key `key` on the Desktop selection.
fn run_key(widgets: &Rc<Widgets>, key: FileKey) {
    let Some(dir) = widgets.desktop.dir.borrow().clone() else {
        opusdm_ipc::err_log!(TAG, "desktop key {key:?} ignored: no desktop folder");
        return;
    };
    let host = DesktopHost::for_key(widgets, dir.clone(), changes_files(key));
    if changes_files(key) && host.selected_paths().is_empty() && !host.selected_entries().is_empty() {
        return host.notify(ONLY_LEFT_OUT);
    }
    match key {
        FileKey::OpenViewer => host.open_in_viewer(host.selected_paths()),
        FileKey::Copy => opusdm_fileactions::keys::transfer_selection(&host, Op::Copy),
        FileKey::Move => opusdm_fileactions::keys::transfer_selection(&host, Op::Move),
        FileKey::Trash => opusdm_fileactions::keys::delete_selection(&host, false),
        FileKey::DeletePermanent => opusdm_fileactions::keys::delete_selection(&host, true),
        FileKey::Rename => opusdm_fileactions::keys::rename_selection(&host),
        FileKey::ClipCopy | FileKey::ClipCut => {
            let paths = host.selected_paths();
            if paths.is_empty() { return host.notify("nothing selected"); }
            crate::desktop_ops::copy_or_cut(&paths, key == FileKey::ClipCut);
        }
        FileKey::ClipPaste => crate::desktop_ops::paste(&widgets.cmd_tx, dir),
        FileKey::MakeDir => crate::desktop_ops::prompt_new(&widgets.primary_window(), "New folder name", &widgets.cmd_tx, dir, |name| Op::MakeDir { name }),
        FileKey::NewFile => crate::desktop_ops::prompt_new(&widgets.primary_window(), "New file name", &widgets.cmd_tx, dir, |name| Op::NewFile { name }),
    }
}

/// `Ok` when at least one DEST Lister shows a local folder, else what to
/// tell the user.
fn dest_check_of(dest_dirs: &[PathBuf]) -> Result<(), String> {
    if dest_dirs.is_empty() {
        return Err(NO_DEST.to_string());
    }
    if dest_dirs.iter().all(|d| opusdm_hub::jobs::is_remote_path(d)) {
        return Err(REMOTE_DEST.to_string());
    }
    Ok(())
}

/// The menu target of the Desktop icon `entry`. A left-out icon stands for
/// a file elsewhere, so the shared menu leaves out Rename, Move and Delete.
pub(crate) fn target_of(entry: &opusdm_iconview::IconEntry) -> MenuTarget {
    let meta = std::fs::metadata(&entry.path).ok();
    MenuTarget {
        path: entry.path.clone(),
        name: entry.name.clone(),
        is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
        size: meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()).unwrap_or(0),
        modified: entry.modified,
        elsewhere: entry.left_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_icons_are_not_files_to_act_on_and_left_out_ones_only_when_asked() {
        let mut group = opusdm_iconview::scan::entry_for_path(Path::new("/nowhere"));
        group.path = opusdm_ipc::groups::group_path("work");
        group.group = true;
        let file = opusdm_iconview::scan::entry_for_path(Path::new("/home/me/Desktop/a.txt"));
        let mut left = opusdm_iconview::scan::entry_for_path(Path::new("/elsewhere/b.txt"));
        left.left_out = true;
        let selected = [group, file.clone(), left.clone()];
        assert_eq!(selectable_paths(&selected, false), vec![file.path.clone(), left.path]);
        assert_eq!(selectable_paths(&selected, true), vec![file.path]);
    }

    #[test]
    fn only_the_keys_that_change_files_skip_left_out_icons() {
        let changing: Vec<FileKey> = opusdm_fileactions::keys::FILE_KEYS.iter().map(|k| k.3).filter(|k| changes_files(*k)).collect();
        assert_eq!(changing, vec![FileKey::Move, FileKey::Trash, FileKey::DeletePermanent, FileKey::Rename, FileKey::ClipCut]);
    }

    #[test]
    fn copy_to_dest_needs_a_local_dest_folder() {
        assert_eq!(dest_check_of(&[]), Err(NO_DEST.to_string()));
        assert_eq!(dest_check_of(&[PathBuf::from("sftp://me@host/in")]), Err(REMOTE_DEST.to_string()));
        assert_eq!(dest_check_of(&[PathBuf::from("sftp://me@host/in"), PathBuf::from("/tmp")]), Ok(()));
    }

    #[test]
    fn a_left_out_icon_is_a_target_that_lives_elsewhere() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("a.txt");
        std::fs::write(&file, b"abc").unwrap();
        let mut entry = opusdm_iconview::scan::entry_for_path(&file);
        entry.left_out = true;
        let target = target_of(&entry);
        assert!(target.elsewhere && !target.is_dir);
        assert_eq!((target.size, target.name.as_str()), (3, "a.txt"));
        let dir = target_of(&opusdm_iconview::scan::entry_for_path(td.path()));
        assert!(dir.is_dir && !dir.elsewhere && dir.size == 0);
    }
}
