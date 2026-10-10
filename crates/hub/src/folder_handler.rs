//! Keep OpusDM the default handler of folders: at hub start the
//! `opusdm-lister.desktop` entry becomes the default application for
//! `inode/directory`, so `xdg-open`, file choosers and "open folder" actions
//! of other programs open a Lister instead of another file manager.

use gtk4::gio;
use gtk4::gio::prelude::*;

/// MIME type of a folder.
pub const FOLDER_MIME: &str = "inode/directory";

/// Desktop entry installed by `install.sh` (runs `opusdm-open-folder`).
pub const LISTER_DESKTOP_ID: &str = "opusdm-lister.desktop";

/// Whether the default handler `current` (a desktop id, `None` when there
/// is none) has to be replaced by the Lister entry.
pub fn needs_update(current: Option<&str>) -> bool {
    current != Some(LISTER_DESKTOP_ID)
}

/// Make the Lister entry the default for folders unless it already is.
/// Writes `~/.config/mimeapps.list` only when something changes. A missing
/// entry (a development build that was never installed) or a failed write
/// is logged and leaves the setting alone.
pub fn ensure_default() {
    let current = gio::AppInfo::default_for_type(FOLDER_MIME, false).and_then(|a| a.id());
    if !needs_update(current.as_deref()) { return; }
    let Some(lister) = gio::DesktopAppInfo::new(LISTER_DESKTOP_ID) else {
        opusdm_ipc::err_log!("opusdm-hub", "{LISTER_DESKTOP_ID} is not installed: folders keep opening with {current:?}");
        return;
    };
    match lister.set_as_default_for_type(FOLDER_MIME) {
        Ok(()) => opusdm_ipc::err_log!("opusdm-hub", "folders now open in a Lister (was {current:?})"),
        Err(e) => opusdm_ipc::err_log!("opusdm-hub", "cannot make {LISTER_DESKTOP_ID} the default for {FOLDER_MIME}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_handler_or_none_is_replaced() {
        assert!(needs_update(Some("org.kde.dolphin.desktop")));
        assert!(needs_update(None));
    }

    #[test]
    fn the_lister_entry_is_left_alone() {
        assert!(!needs_update(Some(LISTER_DESKTOP_ID)));
    }
}
