//! The Desktop as the host of the shared file actions
//! (`opusdm_fileactions`), so a Desktop icon gets the same context menu and
//! keys as a file in a Lister.
//!
//! Ownership: a `DesktopHost` holds a strong `Rc<Widgets>`, as every other
//! Desktop menu item already does. It is made when the icon menu is filled
//! (or a key is pressed) and lives as long as the items of that menu and the
//! dialogs they open. `Widgets` itself is owned by the GTK side for the
//! whole run (see `toolbar.rs`).

use super::toolbar::Widgets;
use crate::server::HubCmd;
use gtk4 as gtk;
use gtk::prelude::*;
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
}

impl DesktopHost {
    /// The host of the Desktop folder `dir`, for the menu of `entry` (or a
    /// key press when `None`).
    pub(crate) fn rc(widgets: &Rc<Widgets>, dir: PathBuf, entry: Option<opusdm_iconview::IconEntry>) -> Rc<dyn FileActionHost> {
        Rc::new(Self { widgets: widgets.clone(), dir, entry })
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
        let Some(field) = self.widgets.desktop.field.borrow().clone() else { return Vec::new() };
        selectable_paths(field.selected_paths())
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
/// a virtual `group:<id>` path, not a file, so it is left out.
fn selectable_paths(selected: Vec<PathBuf>) -> Vec<PathBuf> {
    selected.into_iter().filter(|p| opusdm_ipc::groups::group_id(p).is_none()).collect()
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
    fn group_icons_are_not_files_to_act_on() {
        let group = opusdm_ipc::groups::group_path("work");
        let file = PathBuf::from("/home/me/Desktop/a.txt");
        assert_eq!(selectable_paths(vec![group, file.clone()]), vec![file]);
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
