//! Bookmarks in the Lister (F-48): the manager dialog, "Add Bookmark for This
//! Folder" and the path bar lookup.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::bookmarks::{self, Bookmark, Bookmarks};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// Where the path bar text `text` leads when it is a bookmark reference.
/// The bookmarks come from the in-memory cache, not from disk.
pub fn resolve_text(text: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    crate::config_cache::bookmarks().resolve(text, &home)
}

/// Save the bookmarks and drop the cached copy, so the path bar sees the
/// change at once.
fn store_bookmarks(all: &Bookmarks) -> std::io::Result<()> {
    let saved = bookmarks::save_bookmarks(all);
    crate::config_cache::invalidate_bookmarks();
    saved
}

/// A bookmark name suggested for `folder`: its last component made usable.
pub fn suggest_name(folder: &std::path::Path) -> String {
    let base = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    base.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// Ask for a name and bookmark the current folder.
pub fn add_current_folder(ui: &Rc<Ui>) {
    let cwd = ui.cwd_path();
    if crate::remote::url_of(&cwd).is_some() || crate::archive::parse_archive_path(&cwd).is_some() {
        crate::actions::flash(ui, "only local folders can be bookmarked");
        return;
    }
    let ui2 = ui.clone();
    opusdm_ui::requester_entry(&ui.window, "Bookmark name", &suggest_name(&cwd), &[("Add", true), ("Cancel", false)], move |ok, name| {
        if !ok {
            return;
        }
        let name = name.trim().to_string();
        let mut all = bookmarks::load_bookmarks();
        all.set(&name, &ui2.cwd_path().to_string_lossy());
        match all.problems().first() {
            Some(problem) => crate::actions::flash(&ui2, problem),
            None => match store_bookmarks(&all) {
                Ok(()) => crate::actions::flash(&ui2, &format!("bookmark {name}: added")),
                Err(e) => crate::actions::flash(&ui2, &format!("could not save the bookmarks: {e}")),
            },
        }
    });
}

/// The manager: list with Go / New / Edit / Delete.
pub fn open_manager(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Bookmarks").size(520, 380);
    let hint = gtk::Label::new(Some("Type name: or name:sub/folder in the path bar to go there."));
    hint.set_xalign(0.0);
    shell.content().append(&hint);
    let list = gtk::ListBox::new();
    let scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
    shell.content().append(&scroll);

    let go_btn = shell.button("Go");
    let new_btn = shell.button("New...");
    let edit_btn = shell.button("Edit...");
    let delete_btn = shell.button("Delete");
    shell.button_spacer();
    shell.close_button("Close");

    let state = Rc::new(RefCell::new(bookmarks::load_bookmarks()));
    let refresh: Rc<dyn Fn()> = {
        let (list, state) = (list.clone(), state.clone());
        Rc::new(move || {
            while let Some(row) = list.first_child() {
                list.remove(&row);
            }
            for b in &state.borrow().items {
                let label = gtk::Label::new(Some(&format!("{}:   {}", b.name, b.path)));
                label.set_xalign(0.0);
                label.set_margin_start(6);
                label.set_margin_top(4);
                label.set_margin_bottom(4);
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&label));
                list.append(&row);
            }
        })
    };
    refresh();
    let window = shell.present();

    let selected = {
        let (list, state) = (list.clone(), state.clone());
        Rc::new(move || list.selected_row().and_then(|r| state.borrow().items.get(r.index() as usize).cloned().map(|b| (r.index() as usize, b))))
    };
    let save = {
        let (ui, state) = (ui.clone(), state.clone());
        Rc::new(move || match state.borrow().problems().first() {
            Some(problem) => {
                crate::actions::flash(&ui, problem);
                false
            }
            None => match store_bookmarks(&state.borrow()) {
                Ok(()) => true,
                Err(e) => {
                    crate::actions::flash(&ui, &format!("could not save the bookmarks: {e}"));
                    false
                }
            },
        })
    };

    {
        let (ui, selected, window) = (ui.clone(), selected.clone(), window.clone());
        go_btn.connect_clicked(move |_| {
            let Some((_, b)) = selected() else { return };
            let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
            if let Some(path) = (Bookmarks { items: vec![b] }).resolve(&format!("{}:", selected().map(|s| s.1.name).unwrap_or_default()), &home) {
                window.close();
                ui.navigate_to(&path);
            }
        });
    }
    {
        let (ui, state, refresh, save) = (ui.clone(), state.clone(), refresh.clone(), save.clone());
        new_btn.connect_clicked(move |_| edit_entry(&ui, &state, None, refresh.clone(), save.clone()));
    }
    {
        let (ui, state, refresh, save, selected) = (ui.clone(), state.clone(), refresh.clone(), save.clone(), selected.clone());
        edit_btn.connect_clicked(move |_| {
            if let Some((index, _)) = selected() {
                edit_entry(&ui, &state, Some(index), refresh.clone(), save.clone());
            }
        });
    }
    delete_btn.connect_clicked(move |_| {
        let Some((index, _)) = selected() else { return };
        state.borrow_mut().items.remove(index);
        if save() {
            refresh();
        }
    });
}

/// The add / edit form of one bookmark.
fn edit_entry(ui: &Rc<Ui>, state: &Rc<RefCell<Bookmarks>>, index: Option<usize>, refresh: Rc<dyn Fn()>, save: Rc<dyn Fn() -> bool>) {
    let current = index.and_then(|i| state.borrow().items.get(i).cloned()).unwrap_or(Bookmark { name: String::new(), path: ui.cwd_path().to_string_lossy().into_owned() });
    let shell = opusdm_ui::DialogShell::new(&ui.window, if index.is_some() { "Edit Bookmark" } else { "New Bookmark" }).size(420, 200);
    let name = gtk::Entry::new();
    name.set_text(&current.name);
    let path = gtk::Entry::new();
    path.set_text(&current.path);
    for (caption, entry) in [("Name", &name), ("Folder", &path)] {
        shell.content().append(&gtk::Label::builder().label(caption).xalign(0.0).build());
        shell.content().append(entry);
    }
    let ok_btn = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Cancel");
    let window = shell.present();
    let state = state.clone();
    ok_btn.connect_clicked(move |_| {
        let edited = Bookmark { name: name.text().trim().to_string(), path: path.text().trim().to_string() };
        let before = state.borrow().clone();
        {
            let mut all = state.borrow_mut();
            match index {
                Some(i) if i < all.items.len() => all.items[i] = edited,
                _ => all.items.push(edited),
            }
        }
        if save() {
            refresh();
            window.close();
        } else {
            *state.borrow_mut() = before;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_names_are_usable() {
        assert_eq!(suggest_name(std::path::Path::new("/home/me/My Docs")), "My_Docs");
        assert!(bookmarks::valid_name(&suggest_name(std::path::Path::new("/a/b-c"))));
        assert_eq!(suggest_name(std::path::Path::new("/")), "");
    }
}
