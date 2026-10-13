//! The "Show Trash" dialog: what is in the freedesktop.org trash (the home
//! trash and the trash of every mounted disk), with Restore, Delete
//! (permanently) and Empty Trash. The work runs on a thread; the list is read
//! again when it is done.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_fsops::trash_items::{self, TrashItem};
use std::cell::RefCell;
use std::rc::Rc;

/// Width of the Name column, in characters.
const NAME_CHARS: i32 = 28;
/// Width of the Deleted column, in characters (`YYYY-MM-DD hh:mm:ss`).
const DATE_CHARS: i32 = 19;

/// The open dialog: its list and the items the rows stand for.
struct TrashDialog {
    window: gtk::Window,
    list: gtk::ListBox,
    count: gtk::Label,
    items: RefCell<Vec<TrashItem>>,
}

/// Open the Trash dialog over the Lister.
pub fn open(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Trash").size(820, 480);
    shell.content().append(&_row_box("Name", "Original location", "Deleted", true));
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Multiple);
    let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
    shell.content().append(&scroller);
    let count = gtk::Label::new(None);
    count.set_xalign(0.0);
    shell.content().append(&count);

    let restore_btn = shell.button("Restore");
    let delete_btn = shell.button("Delete");
    restore_btn.set_tooltip_text(Some("Put the selected items back where they were"));
    delete_btn.set_tooltip_text(Some("Delete the selected items permanently (Del)"));
    shell.button_spacer();
    let empty_btn = shell.button("Empty Trash...");
    shell.close_button("Close");

    let dialog = Rc::new(TrashDialog { window: shell.window().clone(), list, count, items: RefCell::new(Vec::new()) });
    _reload(&dialog);

    {
        let dialog = dialog.clone();
        restore_btn.connect_clicked(move |_| _restore_selected(&dialog));
    }
    {
        let dialog = dialog.clone();
        delete_btn.connect_clicked(move |_| _delete_selected(&dialog));
    }
    {
        let dialog = dialog.clone();
        empty_btn.connect_clicked(move |_| _empty_all(&dialog));
    }
    let keys = gtk::EventControllerKey::new();
    {
        let dialog = dialog.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if !matches!(key, gtk::gdk::Key::Delete | gtk::gdk::Key::KP_Delete) {
                return glib::Propagation::Proceed;
            }
            _delete_selected(&dialog);
            glib::Propagation::Stop
        });
    }
    dialog.window.add_controller(keys);
    shell.present();
}

/// One line of the dialog: name, original folder and deletion date in fixed
/// columns. `header` makes it the bold column titles.
fn _row_box(name: &str, origin: &str, date: &str, header: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_start(6);
    row.set_margin_end(6);
    row.set_margin_top(3);
    row.set_margin_bottom(3);
    let name_label = _cell(name, header, gtk::pango::EllipsizeMode::Middle);
    name_label.set_width_chars(NAME_CHARS);
    name_label.set_max_width_chars(NAME_CHARS);
    let origin_label = _cell(origin, header, gtk::pango::EllipsizeMode::Middle);
    origin_label.set_hexpand(true);
    let date_label = _cell(date, header, gtk::pango::EllipsizeMode::None);
    date_label.set_width_chars(DATE_CHARS);
    for label in [&name_label, &origin_label, &date_label] {
        row.append(label);
    }
    row
}

/// A left aligned label of `_row_box`.
fn _cell(text: &str, header: bool, ellipsize: gtk::pango::EllipsizeMode) -> gtk::Label {
    let label = gtk::Label::new(None);
    if header {
        label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(text)));
    } else {
        label.set_text(text);
    }
    label.set_xalign(0.0);
    label.set_ellipsize(ellipsize);
    label
}

/// The texts of the row of `item`: name (`/` after a folder), original
/// folder and the deletion date with a space instead of the `T`.
pub fn row_texts(item: &TrashItem) -> (String, String, String) {
    let name = if item.is_dir { format!("{}/", _original_name(item)) } else { _original_name(item) };
    let folder = item.original.parent().map(|p| p.display().to_string()).unwrap_or_default();
    (name, folder, item.deleted.replacen('T', " ", 1))
}

/// The name the item had before it was trashed.
fn _original_name(item: &TrashItem) -> String {
    item.original.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| item.name.clone())
}

/// Read the trash again and rebuild the rows.
fn _reload(dialog: &TrashDialog) {
    let items = trash_items::list_all();
    while let Some(row) = dialog.list.first_child() {
        dialog.list.remove(&row);
    }
    for item in &items {
        let (name, folder, date) = row_texts(item);
        let row = _row_box(&name, &folder, &date, false);
        row.set_tooltip_text(Some(&item.original.display().to_string()));
        dialog.list.append(&row);
    }
    dialog.count.set_text(&match items.len() {
        0 => "The trash is empty.".to_string(),
        n => format!("{n} item(s) in the trash"),
    });
    *dialog.items.borrow_mut() = items;
}

/// The items of the selected rows.
fn _selected(dialog: &TrashDialog) -> Vec<TrashItem> {
    let items = dialog.items.borrow();
    dialog.list.selected_rows().iter().filter_map(|r| items.get(r.index() as usize).cloned()).collect()
}

/// Put the selected items back where they were.
fn _restore_selected(dialog: &Rc<TrashDialog>) {
    let items = _selected(dialog);
    if items.is_empty() { return; }
    _run(dialog, move || items.iter().filter_map(|i| trash_items::restore(i).err()).collect());
}

/// Ask, then delete the selected items permanently.
fn _delete_selected(dialog: &Rc<TrashDialog>) {
    let items = _selected(dialog);
    if items.is_empty() { return; }
    let message = format!("Delete {} item(s) permanently?", items.len());
    let this = dialog.clone();
    opusdm_ui::requester(&dialog.window, "Request", &message, None, &[("Delete", true), ("Cancel", false)], move |yes| {
        if !yes { return; }
        let items = items.clone();
        _run(&this, move || items.iter().filter_map(|i| trash_items::erase(i).err()).collect());
    });
}

/// Ask, then empty every trash folder.
fn _empty_all(dialog: &Rc<TrashDialog>) {
    let count = dialog.items.borrow().len();
    if count == 0 { return; }
    let message = format!("Empty the trash ({count} item(s))? This cannot be undone.");
    let this = dialog.clone();
    opusdm_ui::requester(&dialog.window, "Request", &message, None, &[("Empty Trash", true), ("Cancel", false)], move |yes| {
        if !yes { return; }
        _run(&this, || {
            trash_items::trash_dirs()
                .iter()
                .flat_map(|t| trash_items::empty(t))
                .map(|(path, msg)| format!("{}: {msg}", path.display()))
                .collect()
        });
    });
}

/// Run `work` on a thread with the dialog locked, then read the trash again
/// and show the errors it returned, if any.
fn _run(dialog: &Rc<TrashDialog>, work: impl FnOnce() -> Vec<String> + Send + 'static) {
    dialog.window.set_sensitive(false);
    let (tx, rx) = async_channel::bounded::<Vec<String>>(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    let dialog = dialog.clone();
    glib::spawn_future_local(async move {
        let errors = rx.recv().await.unwrap_or_else(|e| vec![format!("the trash worker stopped: {e}")]);
        dialog.window.set_sensitive(true);
        _reload(&dialog);
        if errors.is_empty() { return; }
        let message = format!("{} item(s) failed", errors.len());
        opusdm_ui::requester(&dialog.window, "Trash", &message, Some(&errors.join("\n")), &[("OK", ())], |_| {});
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn item(original: &str, is_dir: bool) -> TrashItem {
        TrashItem {
            trash: PathBuf::from("/t"),
            name: "x.2.txt".into(),
            original: PathBuf::from(original),
            deleted: "2026-10-14T10:20:30".into(),
            is_dir,
        }
    }

    #[test]
    fn a_row_shows_the_original_name_folder_and_date() {
        let texts = row_texts(&item("/home/me/x.txt", false));
        assert_eq!(texts, ("x.txt".into(), "/home/me".into(), "2026-10-14 10:20:30".into()));
    }

    #[test]
    fn a_folder_row_ends_with_a_slash() {
        assert_eq!(row_texts(&item("/home/me/docs", true)).0, "docs/");
    }
}
