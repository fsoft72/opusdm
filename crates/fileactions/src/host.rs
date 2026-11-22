//! What a file action needs from the window it runs in (a Lister or the
//! Desktop), and where the items of a menu go.

use gtk4 as gtk;
use opusdm_ipc::filetypes::{FileFacts, FileType, FileTypes};
use opusdm_ipc::function::{Function, FunctionId};
use opusdm_ipc::Op;
use std::path::{Path, PathBuf};

/// The entry a context menu was opened on.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuTarget {
    /// Absolute path of the entry.
    pub path: PathBuf,
    /// File name, no directory part.
    pub name: String,
    /// Whether the entry is a directory (following symlinks).
    pub is_dir: bool,
    /// Size in bytes (0 when unknown or for a directory).
    pub size: u64,
    /// Modification time, whole seconds since the Unix epoch (0 = unknown).
    pub modified: i64,
    /// The icon stands for a file that lives somewhere else (a left-out
    /// Desktop icon): Rename, Move and Delete would act on that file, which
    /// is not what the user sees, so they are not offered.
    pub elsewhere: bool,
}

/// Where host-only items go in the shared menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtraPlace {
    /// Before the shared items.
    Top,
    /// After the shared items.
    Bottom,
}

/// Where the items of a menu go: the Lister popover or the Desktop box.
pub trait MenuSink {
    /// Add an item titled `title`: clicking it closes the menu, then runs
    /// `action`.
    fn item(&self, title: &str, action: Box<dyn Fn()>);
    /// Add a horizontal separator.
    fn separator(&self);
}

/// The window a file action runs in. A Lister and the Desktop implement it,
/// so both offer the same menu and keys.
pub trait FileActionHost {
    /// Window the dialogs are transient for.
    fn parent_window(&self) -> gtk::Window;
    /// Tell the user `text` (a refusal, a failure).
    fn notify(&self, text: &str);
    /// The folder the host shows.
    fn folder(&self) -> PathBuf;
    /// The selected entries, as absolute paths.
    fn selected_paths(&self) -> Vec<PathBuf>;
    /// The selected entries an action that removes files may get: the
    /// selection without the entries that stand for files elsewhere (the
    /// Desktop left-out icons). Every entry of a Lister is its own.
    fn own_selected_paths(&self) -> Vec<PathBuf> {
        self.selected_paths()
    }
    /// `Ok` when a button Copy / Move has somewhere to go (a DEST Lister),
    /// else the reason.
    fn dest_check(&self) -> Result<(), String>;
    /// Run `op` on `srcs`. A `None` destination means "every DEST Lister".
    fn submit_job(&self, op: Op, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf>);
    /// Ask for a new name of `path` (now called `old_name`) and rename it.
    fn rename(&self, path: &Path, old_name: &str);
    /// Ask with `message`, then trash `srcs` (or delete them for good when
    /// `permanent`).
    fn confirm_delete(&self, message: &str, srcs: Vec<PathBuf>, permanent: bool);
    /// The saved Functions.
    fn functions(&self) -> Vec<Function>;
    /// The user file type of `target`, if one matches.
    fn file_type(&self, target: &MenuTarget) -> Option<FileType>;
    /// Run the file type function `id` on `target` alone.
    fn run_on_target(&self, target: &MenuTarget, id: &FunctionId);
    /// Show `paths` in `opusdm-viewer`.
    fn open_in_viewer(&self, paths: Vec<PathBuf>);
    /// Make `path` the desktop background.
    fn set_background(&self, path: &Path);
    /// Whether `path` is an entry inside an archive (only a Lister can
    /// browse one).
    fn inside_archive(&self, _path: &Path) -> bool {
        false
    }
    /// Add the items only this host has at `place` of the menu of `target`.
    fn extra_items(&self, _place: ExtraPlace, _menu: &dyn MenuSink, _target: &MenuTarget) {}
}

/// The paths a context menu action on `target` applies to: the whole
/// selection when `target` is part of it, otherwise just `target`.
pub fn targets_for(selected: Vec<PathBuf>, target: &Path) -> Vec<PathBuf> {
    if selected.iter().any(|p| p == target) {
        selected
    } else {
        vec![target.to_path_buf()]
    }
}

/// The destination of a job that goes to the DEST Lister when there is one:
/// `None` (every DEST) when a DEST is set, else the host's own folder.
pub fn dest_or_folder(host: &dyn FileActionHost) -> Option<PathBuf> {
    match host.dest_check() {
        Ok(()) => None,
        Err(_) => Some(host.folder()),
    }
}

/// The file type of `target` in `types`, or `None` when none matches (or
/// `target` is a directory). Only the few leading bytes a magic rule needs
/// are read from the file.
pub fn file_type_in(types: &FileTypes, target: &MenuTarget) -> Option<FileType> {
    if target.is_dir { return None; }
    let head = opusdm_ipc::filetypes::read_head(&target.path, types.head_needed());
    let facts = FileFacts { name: &target.name, size: target.size, modified: target.modified, head: &head };
    types.find(&facts).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_targets_follow_the_selection_only_when_it_holds_the_clicked_file() {
        let (a, b, c) = (PathBuf::from("/d/a"), PathBuf::from("/d/b"), PathBuf::from("/d/c"));
        assert_eq!(targets_for(vec![a.clone(), b.clone()], &b), vec![a.clone(), b.clone()]);
        assert_eq!(targets_for(vec![a, b], &c), vec![c]);
    }
}
