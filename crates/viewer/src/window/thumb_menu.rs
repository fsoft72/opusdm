//! The menu of a thumbnail (right click in the strip): copy the path, open the
//! folder in a Lister, trash or delete the file. It acts on the file of the
//! thumbnail, which may not be the one on screen.

use super::{State, _name};
use crate::strip::ThumbMenuAt;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::sockpath::{hub_sock_path, send_oneshot};
use opusdm_ipc::ToHub;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Margin of the menu around its items, in pixels.
const MENU_MARGIN: i32 = 4;

/// `path` as a full path: the gallery keeps the names of the command line, and
/// neither the clipboard nor the hub share the folder the viewer started in.
fn _full_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|e| {
        opusdm_ipc::err_log!("viewer", "cannot make {} a full path: {e}", path.display());
        path.to_path_buf()
    })
}

/// Ask the hub at `sock` to open a Lister on the folder that holds `path`.
fn _ask_hub_for_folder(sock: &Path, path: &Path) -> io::Result<()> {
    let full = _full_path(path);
    let folder = full.parent().ok_or_else(|| io::Error::other(format!("{} has no folder", full.display())))?;
    send_oneshot(sock, &[ToHub::SpawnLister { start_path: Some(folder.to_path_buf()) }])
}

impl State {
    /// Show the menu of the thumbnail of `at.path`. The popover hangs on the
    /// strip, not on the thumbnail: a Sidecar update can drop the thumbnail while
    /// the menu is open. It holds the state only until it closes, when it is
    /// unparented and dropped, so it does not keep a closed window alive.
    pub(super) fn _show_thumb_menu(self: &Rc<Self>, at: ThumbMenuAt) {
        let popover = gtk::Popover::new();
        popover.set_parent(&self.strip.root);
        popover.set_position(gtk::PositionType::Top);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(at.x as i32, at.y as i32, 1, 1)));
        // Unparent on idle: `closed` comes before the action of the clicked item.
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        let items = gtk::Box::new(gtk::Orientation::Vertical, 2);
        items.set_margin_start(MENU_MARGIN);
        items.set_margin_end(MENU_MARGIN);
        items.set_margin_top(MENU_MARGIN);
        items.set_margin_bottom(MENU_MARGIN);
        items.append(&self._menu_item(&popover, "Copy path", &at.path, |s, path| s._copy_path(path)));
        items.append(&self._menu_item(&popover, "Open folder in Lister", &at.path, |s, path| s._open_folder(path)));
        items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        items.append(&self._menu_item(&popover, "Trash", &at.path, |s, path| s._ask_trash(path)));
        items.append(&self._menu_item(&popover, "Delete permanently", &at.path, |s, path| s._ask_remove(path)));
        popover.set_child(Some(&items));
        popover.popup();
    }

    /// A flat button that closes the menu and runs `action` on `path`.
    fn _menu_item(self: &Rc<Self>, popover: &gtk::Popover, title: &str, path: &Path, action: impl Fn(&Rc<State>, &Path) + 'static) -> gtk::Button {
        let button = gtk::Button::with_label(title);
        button.add_css_class("flat");
        if let Some(label) = button.child().and_downcast::<gtk::Label>() { label.set_xalign(0.0); }
        let (this, popover, target) = (self.clone(), popover.clone(), path.to_path_buf());
        button.connect_clicked(move |_| {
            popover.popdown();
            action(&this, &target);
        });
        button
    }

    /// Put the full path of `path` on the clipboard.
    fn _copy_path(&self, path: &Path) {
        self.window.clipboard().set_text(&_full_path(path).to_string_lossy());
    }

    /// Open the folder of `path` in a Lister; a hub that cannot be reached is told.
    fn _open_folder(&self, path: &Path) {
        let sock = hub_sock_path();
        let Err(e) = _ask_hub_for_folder(&sock, path) else { return };
        opusdm_ipc::err_log!("viewer", "cannot open the folder of {} through the hub at {}: {e}", path.display(), sock.display());
        opusdm_ui::requester(&self.window, "Open folder", "Cannot open the folder in a Lister. Is the hub running?", Some(&e.to_string()), &[("Close", ())], |_| {});
    }

    /// Ask before deleting `path` for good.
    fn _ask_remove(self: &Rc<Self>, path: &Path) {
        let (this, target) = (self.clone(), path.to_path_buf());
        opusdm_ui::requester(&self.window, "Delete", &format!("Delete \"{}\" permanently? This cannot be undone.", _name(path)), None, &[("Delete", true), ("Cancel", false)], move |yes| {
            if yes { this._remove_for_good(&target); }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::frame::recv;
    use std::os::unix::net::UnixListener;

    #[test]
    fn a_relative_name_becomes_a_full_path() {
        let full = _full_path(Path::new("pics/a.png"));
        assert_eq!(full, std::env::current_dir().unwrap().join("pics/a.png"));
    }

    #[test]
    fn a_full_path_stays_as_it_is() {
        assert_eq!(_full_path(Path::new("/d/a.png")), PathBuf::from("/d/a.png"));
    }

    #[test]
    fn the_hub_is_asked_for_a_lister_on_the_folder_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("hub.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        _ask_hub_for_folder(&sock, Path::new("/d/pics/a.png")).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let message: ToHub = recv(&mut stream).unwrap().expect("a message");
        assert!(matches!(message, ToHub::SpawnLister { start_path: Some(p) } if p == Path::new("/d/pics")));
    }

    #[test]
    fn a_hub_that_is_not_there_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(_ask_hub_for_folder(&dir.path().join("none.sock"), Path::new("/d/a.png")).is_err());
    }
}
