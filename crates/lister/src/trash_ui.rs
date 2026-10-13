//! The "Show Trash" dialog: what is in the freedesktop.org trash (the home
//! trash and the trash of every mounted disk), with Restore, Restore To...,
//! Delete (permanently) and Empty Trash. The list is a `ColumnView` with a
//! `MultiSelection` like the Lister (click, Ctrl / Shift click, Ctrl+A,
//! sortable headers). The work runs on a thread; the list is read again when
//! it is done.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib::{self, BoxedAnyObject};
use gtk::{gio, prelude::*};
use opusdm_fsops::trash_items::{self, TrashItem};
use std::cmp::Ordering;
use std::path::PathBuf;
use std::rc::Rc;

/// The open dialog: the store behind the list and its selection.
struct TrashDialog {
    window: gtk::Window,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    count: gtk::Label,
    /// The folder Restore To... proposes: the Lister's current one.
    cwd: PathBuf,
}

/// One column of the list.
#[derive(Clone, Copy)]
enum Col {
    Name,
    Origin,
    Deleted,
}

impl Col {
    /// The text this column shows for `item`.
    fn text(self, item: &TrashItem) -> String {
        let (name, folder, date) = row_texts(item);
        match self {
            Col::Name => name,
            Col::Origin => folder,
            Col::Deleted => date,
        }
    }
}

/// Open the Trash dialog over the Lister.
pub fn open(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Trash").size(860, 500);
    let store = gio::ListStore::new::<BoxedAnyObject>();
    let view = gtk::ColumnView::new(None::<gtk::MultiSelection>);
    let sorted = gtk::SortListModel::new(Some(store.clone()), view.sorter());
    let selection = gtk::MultiSelection::new(Some(sorted));
    view.set_model(Some(&selection));
    view.set_show_column_separators(true);
    view.add_css_class("data-table");
    _add_column(&view, "Name", Col::Name, false);
    _add_column(&view, "Original location", Col::Origin, true);
    let deleted = _add_column(&view, "Deleted", Col::Deleted, false);
    view.sort_by_column(Some(&deleted), gtk::SortType::Descending);
    let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
    shell.content().append(&scroller);
    let count = gtk::Label::new(None);
    count.set_xalign(0.0);
    shell.content().append(&count);

    let restore_btn = shell.button("Restore");
    let restore_to_btn = shell.button("Restore To...");
    let delete_btn = shell.button("Delete");
    restore_btn.set_tooltip_text(Some("Put the selected items back where they were"));
    restore_to_btn.set_tooltip_text(Some("Put the selected items into another folder"));
    delete_btn.set_tooltip_text(Some("Delete the selected items permanently (Del)"));
    shell.button_spacer();
    let empty_btn = shell.button("Empty Trash...");
    shell.close_button("Close");

    let dialog = Rc::new(TrashDialog { window: shell.window().clone(), store, selection, count, cwd: ui.cwd_path() });
    _reload(&dialog);

    _on_click(&dialog, &restore_btn, _restore_selected);
    _on_click(&dialog, &restore_to_btn, _restore_selected_to);
    _on_click(&dialog, &delete_btn, _delete_selected);
    _on_click(&dialog, &empty_btn, _empty_all);
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
    view.grab_focus();
}

/// Run `action` on the dialog when `btn` is clicked.
fn _on_click(dialog: &Rc<TrashDialog>, btn: &gtk::Button, action: fn(&Rc<TrashDialog>)) {
    let dialog = dialog.clone();
    btn.connect_clicked(move |_| action(&dialog));
}

/// Add the column `col` titled `title` to `view`, sortable by its text
/// (the Deleted text sorts by time, as it is `YYYY-MM-DD hh:mm:ss`). Cells
/// carry the Lister's `row-dir` / `row-file` / `cell-selected` classes, so
/// the list has the Lister's colours.
fn _add_column(view: &gtk::ColumnView, title: &str, col: Col, expand: bool) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, list_item| {
        let Some(item) = list_item.downcast_ref::<gtk::ListItem>() else { return };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        item.set_child(Some(&label));
        // Selection can change without a rebind.
        item.connect_selected_notify(|li| {
            if let Some(cell) = li.child() {
                crate::columns::apply_selected_class(&cell, li.is_selected());
            }
        });
    });
    factory.connect_bind(move |_, list_item| {
        let Some(item) = list_item.downcast_ref::<gtk::ListItem>() else { return };
        let (Some(label), Some(trashed)) = (item.child().and_downcast::<gtk::Label>(), item.item().as_ref().and_then(_item_of)) else { return };
        label.remove_css_class("row-dir");
        label.remove_css_class("row-file");
        label.add_css_class(if trashed.is_dir { "row-dir" } else { "row-file" });
        crate::columns::apply_selected_class(&label, item.is_selected());
        label.set_text(&col.text(&trashed));
        label.set_tooltip_text(Some(&trashed.original.display().to_string()));
    });
    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_expand(expand);
    column.set_resizable(true);
    column.set_sorter(Some(&gtk::CustomSorter::new(move |a, b| {
        let (Some(a), Some(b)) = (_item_of(a), _item_of(b)) else { return gtk::Ordering::Equal };
        _text_cmp(&col.text(&a), &col.text(&b)).into()
    })));
    view.append_column(&column);
    column
}

/// Case-insensitive order of two cell texts, the exact text breaking ties.
fn _text_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
}

/// The trash item a list object stands for.
fn _item_of(obj: &glib::Object) -> Option<TrashItem> {
    obj.downcast_ref::<BoxedAnyObject>().map(|o| o.borrow::<TrashItem>().clone())
}

/// The texts of the row of `item`: name (`/` after a folder), original
/// folder and the deletion date with a space instead of the `T`.
pub fn row_texts(item: &TrashItem) -> (String, String, String) {
    let name = item.original_name();
    let name = if item.is_dir { format!("{name}/") } else { name };
    let folder = item.original.parent().map(|p| p.display().to_string()).unwrap_or_default();
    (name, folder, item.deleted.replacen('T', " ", 1))
}

/// Read the trash again and refill the list.
fn _reload(dialog: &TrashDialog) {
    let items = trash_items::list_all();
    let objects: Vec<BoxedAnyObject> = items.iter().cloned().map(BoxedAnyObject::new).collect();
    dialog.store.splice(0, dialog.store.n_items(), &objects);
    dialog.count.set_text(&match items.len() {
        0 => "The trash is empty.".to_string(),
        n => format!("{n} item(s) in the trash"),
    });
}

/// The items of the selected rows.
fn _selected(dialog: &TrashDialog) -> Vec<TrashItem> {
    let bits = dialog.selection.selection();
    (0..bits.size())
        .filter_map(|i| dialog.selection.item(bits.nth(i as u32)))
        .filter_map(|o| _item_of(&o))
        .collect()
}

/// Put the selected items back where they were.
fn _restore_selected(dialog: &Rc<TrashDialog>) {
    let items = _selected(dialog);
    if items.is_empty() { return; }
    _run(dialog, move || items.iter().filter_map(|i| trash_items::restore(i).err()).collect());
}

/// Ask for a folder (the Lister's current one proposed), then restore the
/// selected items into it under their original names.
fn _restore_selected_to(dialog: &Rc<TrashDialog>) {
    let items = _selected(dialog);
    if items.is_empty() { return; }
    let title = format!("Restore {} item(s) into the folder", items.len());
    let this = dialog.clone();
    let initial = dialog.cwd.to_string_lossy().into_owned();
    opusdm_ui::requester_entry(&dialog.window, &title, &initial, &[("Restore", true), ("Cancel", false)], move |ok, text| {
        if !ok || text.trim().is_empty() { return; }
        let dir = PathBuf::from(crate::pathcomplete::expand_tilde(text.trim()));
        let items = items.clone();
        _run(&this, move || items.iter().filter_map(|i| trash_items::restore_to(i, &dir).err()).collect());
    });
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
    let count = dialog.store.n_items();
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
