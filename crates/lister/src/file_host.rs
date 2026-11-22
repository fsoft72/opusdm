//! The Lister as the host of the shared file actions
//! (`opusdm_fileactions`): the context menu and its dialogs call back into
//! this window through [`ListerHost`].
//!
//! Ownership: a `ListerHost` holds a strong `Rc<Ui>`, exactly what the menu
//! items and the dialogs held before they moved to the shared crate. It is
//! made when a menu or a dialog opens and lives as long as that menu or
//! dialog; the window itself is owned elsewhere (see `window.rs`).

use crate::window::Ui;
use opusdm_fileactions::{ExtraPlace, FileActionHost, MenuSink, MenuTarget};
use opusdm_ipc::filetypes::FileType;
use opusdm_ipc::function::{Function, FunctionId};
use opusdm_ipc::{JobOptions, Op};
use gtk4 as gtk;
use gtk::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The file actions host of one Lister window.
pub struct ListerHost {
    ui: Rc<Ui>,
}

impl ListerHost {
    /// The host of `ui`, ready to hand to `opusdm_fileactions`.
    pub fn rc(ui: &Rc<Ui>) -> Rc<dyn FileActionHost> {
        Rc::new(Self { ui: ui.clone() })
    }
}

impl FileActionHost for ListerHost {
    fn parent_window(&self) -> gtk::Window {
        self.ui.window.clone().upcast()
    }

    fn notify(&self, text: &str) {
        crate::actions::flash(&self.ui, text);
    }

    fn folder(&self) -> PathBuf {
        self.ui.cwd_path()
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.ui.selected_paths()
    }

    fn dest_check(&self) -> Result<(), String> {
        crate::actions::resolve_copy_target(&self.ui.hub.roles.borrow(), self.ui.hub.id)
    }

    fn submit_job(&self, op: Op, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf>) {
        crate::job_submit::submit_job(&self.ui, op, srcs, dest_dir, JobOptions::default());
    }

    fn rename(&self, path: &Path, old_name: &str) {
        let (ui, old) = (self.ui.clone(), path.to_path_buf());
        crate::actions::prompt(&self.ui.window.clone(), "Rename to", old_name, move |new_name| {
            crate::actions::rename_and_reload(&ui, &old, &new_name)
        });
    }

    fn confirm_delete(&self, message: &str, srcs: Vec<PathBuf>, permanent: bool) {
        crate::actions::confirm_delete(&self.ui, message, srcs, permanent);
    }

    fn functions(&self) -> Vec<Function> {
        self.ui.hub.functions.borrow().clone()
    }

    fn file_type(&self, target: &MenuTarget) -> Option<FileType> {
        opusdm_fileactions::file_type_in(&crate::config_cache::filetypes(), target)
    }

    fn run_on_target(&self, target: &MenuTarget, id: &FunctionId) {
        crate::filetypes_ui::run_on_row(&self.ui, &target.name, id);
    }

    fn open_in_viewer(&self, paths: Vec<PathBuf>) {
        crate::context_menu::open_in_viewer(&self.ui, paths);
    }

    fn set_background(&self, path: &Path) {
        crate::context_menu::set_background(&self.ui, path);
    }

    fn inside_archive(&self, path: &Path) -> bool {
        crate::archive::parse_archive_path(path).is_some()
    }

    fn extra_items(&self, place: ExtraPlace, menu: &dyn MenuSink, target: &MenuTarget) {
        if place != ExtraPlace::Bottom { return; }
        {
            let (ui, path) = (self.ui.clone(), target.path.clone());
            menu.item("Leave Out on Desktop", Box::new(move || crate::context_menu::leave_out(&ui, &path)));
        }
        if target.is_dir {
            let ui = self.ui.clone();
            menu.item("Checksums & Dimensioni cartelle", Box::new(move || crate::tools::calculate_dir_sizes_async(&ui, false)));
        }
    }
}
