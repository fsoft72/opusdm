//! Program groups and button banks on the desktop: adding icons to a group,
//! the group window actions, the new group dialog and the group menu entries.

use gtk4 as gtk;
use gtk::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;
use super::toolbar::Widgets;
use super::{desktop_menu::*, desktop_windows::*};

/// Persist group `id`, logging (not failing) on error.
pub(crate) fn save_group(widgets: &Widgets, id: &str) {
    if let Err(e) = widgets.desktop.groups.borrow().save_group(id) {
        opusdm_ipc::err_log!("hub", "cannot save group {id}: {e}");
    }
}

/// After any change to the groups: bring the open group windows and the
/// desktop icons in line with the store.
pub(crate) fn groups_changed(widgets: &Rc<Widgets>) {
    let snapshot = widgets.desktop.groups.borrow().clone();
    widgets.desktop.group_windows.refresh(&snapshot);
    reload_desktop_field(widgets);
}

/// Tell the user why a group operation was refused.
pub(crate) fn show_group_error(widgets: &Rc<Widgets>, message: &str) {
    opusdm_ui::requester(&widgets.primary_window(), "Request", message, None, &[("OK", true)], |_| {});
}

/// Add `paths` to group `id`, save and refresh. Nothing happens when every path
/// is already a member (or is not absolute).
pub(crate) fn add_to_group(widgets: &Rc<Widgets>, id: &str, paths: Vec<PathBuf>) {
    let added = widgets.desktop.groups.borrow_mut().add_items(id, &paths);
    if added == 0 {
        return;
    }
    save_group(widgets, id);
    groups_changed(widgets);
}

/// The callbacks a group window uses to talk back to the desktop.
pub(crate) fn group_actions(widgets: &Rc<Widgets>) -> crate::group_window::GroupActions {
    let for_launch = widgets.clone();
    let for_add = widgets.clone();
    let for_remove = widgets.clone();
    crate::group_window::GroupActions {
        launch: Rc::new(move |entry| launch_entry(&for_launch, entry)),
        add: Rc::new(move |id, paths| add_to_group(&for_add, id, paths)),
        remove: Rc::new(move |id, path| {
            for_remove.desktop.groups.borrow_mut().remove_item(id, &path);
            save_group(&for_remove, id);
            groups_changed(&for_remove);
        }),
    }
}

/// The callbacks the bank manager and editor use to reach the hub.
pub(crate) fn bank_hooks(widgets: &Rc<Widgets>) -> opusdm_buttonbank::editor::Hooks {
    use opusdm_buttonbank::editor::{BankChange, Hooks};
    let for_changed = widgets.clone();
    let for_open = widgets.clone();
    let for_close = widgets.clone();
    let for_functions = widgets.clone();
    Hooks {
        changed: Rc::new(move |change| match change {
            BankChange::Saved(id) | BankChange::Deleted(id) => for_changed.bank_windows.refresh(&id),
        }),
        open: Rc::new(move |id| {
            if let Some(app) = for_open.primary_window().application() {
                for_open.bank_windows.open_id(&app, id);
            }
        }),
        close: Rc::new(move |id| for_close.bank_windows.close_id(id)),
        functions: Rc::new(move || for_functions.functions.borrow().clone()),
    }
}

/// Open the button bank manager.
pub(crate) fn open_bank_manager(widgets: &Rc<Widgets>) {
    opusdm_buttonbank::editor::open_manager(&widgets.primary_window(), widgets.banks.clone(), bank_hooks(widgets));
}

/// Open (or raise) the window of group `id`.
pub(crate) fn open_group(widgets: &Rc<Widgets>, id: &str) {
    let Some(app) = widgets.primary_window().application() else { return };
    let Some(group) = widgets.desktop.groups.borrow().get(id).cloned() else { return };
    let icon_set = widgets.desktop.icon_set.borrow().clone();
    widgets.desktop.group_windows.show(&app, &group, &icon_set, &group_actions(widgets));
}

/// Ask for a name and create a new, empty group.
pub(crate) fn new_group_dialog(widgets: &Rc<Widgets>) {
    let widgets = widgets.clone();
    opusdm_ui::requester_entry(
        &widgets.primary_window(),
        "New group name",
        "",
        &[("OK", true), ("Cancel", false)],
        move |accepted, text| {
            if !accepted {
                return;
            }
            let created = widgets
                .desktop
                .groups
                .borrow_mut()
                .create_in_dir(&opusdm_ipc::groups::groups_dir(), &text);
            match created {
                Ok(id) => {
                    save_group(&widgets, &id);
                    groups_changed(&widgets);
                }
                Err(message) => show_group_error(&widgets, &message),
            }
        },
    );
}

/// The context menu of a group icon: Open, Rename Group..., Delete Group...
pub(crate) fn fill_group_menu(menu: &gtk::Box, widgets: &Rc<Widgets>, entry: &opusdm_iconview::IconEntry) {
    let Some(id) = opusdm_ipc::groups::group_id(&entry.path).map(str::to_string) else { return };
    {
        let id = id.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Open");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            open_group(&widgets, &id);
        });
        menu.append(&btn);
    }
    {
        let id = id.clone();
        let name = entry.name.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Rename Group...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let id = id.clone();
            let widgets_for_cb = widgets.clone();
            opusdm_ui::requester_entry(
                &widgets.primary_window(),
                "Rename group",
                &name,
                &[("OK", true), ("Cancel", false)],
                move |accepted, text| {
                    if !accepted {
                        return;
                    }
                    let renamed = widgets_for_cb.desktop.groups.borrow_mut().rename(&id, &text);
                    match renamed {
                        Ok(()) => {
                            save_group(&widgets_for_cb, &id);
                            groups_changed(&widgets_for_cb);
                        }
                        Err(message) => show_group_error(&widgets_for_cb, &message),
                    }
                },
            );
        });
        menu.append(&btn);
    }
    {
        let id = id.clone();
        let name = entry.name.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Delete Group...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let id = id.clone();
            let widgets_for_cb = widgets.clone();
            opusdm_ui::requester(
                &widgets.primary_window(),
                "Request",
                &format!("Delete the group \"{name}\"? Its members are not touched."),
                None,
                &[("Delete", true), ("Cancel", false)],
                move |confirmed| {
                    if !confirmed {
                        return;
                    }
                    widgets_for_cb.desktop.groups.borrow_mut().delete(&id);
                    if let Err(e) = opusdm_ipc::Groups::remove_group_file(&id) {
                        opusdm_ipc::err_log!("hub", "cannot delete group file {id}: {e}");
                    }
                    groups_changed(&widgets_for_cb);
                },
            );
        });
        menu.append(&btn);
    }
}
