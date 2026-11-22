//! The file keys a Lister and the Desktop share, and the actions on the
//! selection they start. The Lister resolves its keys in its own table
//! (`opusdm_lister::keys`); a test there checks that it maps every key of
//! [`resolve`] to the same command.

use crate::host::FileActionHost;
use gtk4::gdk;
use opusdm_ipc::Op;

/// A file key and what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKey {
    /// F3: show the selection in the viewer.
    OpenViewer,
    /// F5: copy the selection to the DEST Lister.
    Copy,
    /// F6: move the selection to the DEST Lister.
    Move,
    /// Delete: move the selection to the trash.
    Trash,
    /// Shift+Delete: delete the selection for good.
    DeletePermanent,
    /// F2: rename the selected entry.
    Rename,
    /// Ctrl+C: copy the selection to the clipboard.
    ClipCopy,
    /// Ctrl+X: cut the selection to the clipboard.
    ClipCut,
    /// Ctrl+V: paste the clipboard here.
    ClipPaste,
    /// Ctrl+N: make a folder here.
    MakeDir,
    /// Ctrl+Shift+N: make an empty file here.
    NewFile,
}

/// Every file key, with the key, Ctrl and Shift that start it (the first
/// keyval of each; the keypad Delete is also accepted by [`resolve`]).
pub const FILE_KEYS: [(gdk::Key, bool, bool, FileKey); 11] = [
    (gdk::Key::F3, false, false, FileKey::OpenViewer),
    (gdk::Key::F5, false, false, FileKey::Copy),
    (gdk::Key::F6, false, false, FileKey::Move),
    (gdk::Key::Delete, false, false, FileKey::Trash),
    (gdk::Key::Delete, false, true, FileKey::DeletePermanent),
    (gdk::Key::F2, false, false, FileKey::Rename),
    (gdk::Key::c, true, false, FileKey::ClipCopy),
    (gdk::Key::x, true, false, FileKey::ClipCut),
    (gdk::Key::v, true, false, FileKey::ClipPaste),
    (gdk::Key::n, true, false, FileKey::MakeDir),
    (gdk::Key::N, true, true, FileKey::NewFile),
];

/// The file key for `key` with these modifiers, or `None`. Alt never makes
/// a file key; Shift matters only for Delete, F3 and Ctrl+N.
pub fn resolve(key: gdk::Key, ctrl: bool, shift: bool, alt: bool) -> Option<FileKey> {
    if alt { return None; }
    if ctrl {
        return match key {
            gdk::Key::c | gdk::Key::C => Some(FileKey::ClipCopy),
            gdk::Key::x | gdk::Key::X => Some(FileKey::ClipCut),
            gdk::Key::v | gdk::Key::V => Some(FileKey::ClipPaste),
            gdk::Key::n | gdk::Key::N if shift => Some(FileKey::NewFile),
            gdk::Key::n | gdk::Key::N => Some(FileKey::MakeDir),
            _ => None,
        };
    }
    match key {
        // Shift+F3 is the Lister's Sidecar, not a file key.
        gdk::Key::F3 if !shift => Some(FileKey::OpenViewer),
        gdk::Key::F5 => Some(FileKey::Copy),
        gdk::Key::F6 => Some(FileKey::Move),
        gdk::Key::Delete | gdk::Key::KP_Delete if shift => Some(FileKey::DeletePermanent),
        gdk::Key::Delete | gdk::Key::KP_Delete => Some(FileKey::Trash),
        gdk::Key::F2 => Some(FileKey::Rename),
        _ => None,
    }
}

/// F5 / F6: copy or move the selection to the DEST Lister.
pub fn transfer_selection(host: &dyn FileActionHost, op: Op) {
    if let Err(msg) = host.dest_check() {
        host.notify(&msg);
        return;
    }
    let srcs = host.selected_paths();
    if srcs.is_empty() {
        host.notify("nothing selected");
        return;
    }
    host.submit_job(op, srcs, None);
}

/// Delete / Shift+Delete: ask, then trash the selection (or delete it for
/// good when `permanent`).
pub fn delete_selection(host: &dyn FileActionHost, permanent: bool) {
    let paths = host.selected_paths();
    if paths.is_empty() {
        host.notify("nothing selected");
        return;
    }
    host.confirm_delete(&delete_message(paths.len(), permanent), paths, permanent);
}

/// The question asked before deleting `count` entries.
pub fn delete_message(count: usize, permanent: bool) -> String {
    if permanent {
        format!("Delete {count} item(s) permanently?")
    } else {
        format!("Move {count} item(s) to the trash?")
    }
}

/// F2: rename the one selected entry.
pub fn rename_selection(host: &dyn FileActionHost) {
    let selected = host.selected_paths();
    let [one] = selected.as_slice() else {
        host.notify(if selected.is_empty() { "nothing selected to rename" } else { "select one entry to rename" });
        return;
    };
    let name = one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    host.rename(one, &name);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_key_resolves_to_its_file_key() {
        for (key, ctrl, shift, want) in FILE_KEYS {
            assert_eq!(resolve(key, ctrl, shift, false), Some(want), "{key:?} ctrl={ctrl} shift={shift}");
        }
    }

    #[test]
    fn alt_shift_f3_and_plain_letters_are_not_file_keys() {
        assert_eq!(resolve(gdk::Key::F5, false, false, true), None);
        assert_eq!(resolve(gdk::Key::F3, false, true, false), None);
        assert_eq!(resolve(gdk::Key::c, false, false, false), None);
        assert_eq!(resolve(gdk::Key::a, true, false, false), None, "Ctrl+A is the icon field's own");
        assert_eq!(resolve(gdk::Key::KP_Delete, false, false, false), Some(FileKey::Trash));
    }

    #[test]
    fn the_delete_question_says_trash_or_permanently() {
        assert_eq!(delete_message(2, false), "Move 2 item(s) to the trash?");
        assert_eq!(delete_message(1, true), "Delete 1 item(s) permanently?");
    }
}
