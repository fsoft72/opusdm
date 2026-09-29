//! The "Cache List..." dialog: every Lister's cached directories.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::{CacheEntry, ToHub};
use std::rc::Rc;

/// The text of one dialog row.
pub fn entry_label(entry: &CacheEntry) -> String {
    format!("Lister {}: {}", entry.lister.0, entry.path.display())
}

/// Ask the hub for every Lister's cached paths; the reply opens the dialog.
pub fn open(ui: &Rc<Ui>) {
    if let Err(e) = ui.hub.client.send(&ToHub::QueryCaches) {
        crate::actions::flash(ui, &format!("cannot reach the hub: {e}"));
    }
}

/// Ask every Lister to drop its cache.
pub fn free_all(ui: &Rc<Ui>) {
    if let Err(e) = ui.hub.client.send(&ToHub::FreeAllCaches) {
        crate::actions::flash(ui, &format!("cannot reach the hub: {e}"));
    }
}

/// Show the dialog for a `CacheList` reply. Double click raises the owning
/// Lister and navigates it there; "Open here" navigates this Lister.
pub fn show(ui: &Rc<Ui>, entries: Vec<CacheEntry>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Cache List").size(560, 420);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    for entry in &entries {
        let label = gtk::Label::new(Some(&entry_label(entry)));
        label.set_xalign(0.0);
        list.append(&label);
    }
    if entries.is_empty() {
        shell.content().append(&gtk::Label::new(Some("No directory is cached.")));
    }
    let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
    shell.content().append(&scroller);

    let entries = Rc::new(entries);
    {
        let (ui, entries) = (ui.clone(), entries.clone());
        list.connect_row_activated(move |_, row| {
            let Some(entry) = entries.get(row.index() as usize) else { return };
            let msg = ToHub::JumpTo { lister: entry.lister, path: entry.path.clone() };
            if let Err(e) = ui.hub.client.send(&msg) {
                crate::actions::flash(&ui, &format!("cannot reach the hub: {e}"));
            }
        });
    }
    let here = shell.button("Open here");
    let free = shell.button("Free all caches");
    shell.close_button("Close");
    let window = shell.window().clone();
    {
        let (ui, entries, list, window) = (ui.clone(), entries.clone(), list.clone(), window.clone());
        here.connect_clicked(move |_| {
            let Some(row) = list.selected_row() else {
                crate::actions::flash(&ui, "select a directory first");
                return;
            };
            let Some(entry) = entries.get(row.index() as usize) else { return };
            ui.navigate_to(&entry.path);
            window.close();
        });
    }
    {
        let (ui, window) = (ui.clone(), window.clone());
        free.connect_clicked(move |_| {
            free_all(&ui);
            window.close();
        });
    }
    shell.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::ListerId;

    #[test]
    fn a_row_names_the_lister_and_the_path() {
        let e = CacheEntry { lister: ListerId(3), path: "/home/x".into() };
        assert_eq!(entry_label(&e), "Lister 3: /home/x");
    }
}
