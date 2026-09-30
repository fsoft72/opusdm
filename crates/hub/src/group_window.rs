//! Group windows: one top-level window per opened program group, showing its
//! members as an icon field. Double click starts a member, dropping files adds
//! them, the context menu removes one. The window only reports what the user
//! asked for through [`GroupActions`]; the desktop owns the group store. See
//! `docs/superpowers/specs/2026-09-30-program-groups-design.md`.

use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_iconview::{scan, IconEntry, IconField, LayoutMode};
use opusdm_ipc::{Group, Groups, IconSetId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

/// Default size of a new group window, in pixels.
const WINDOW_WIDTH: i32 = 480;
const WINDOW_HEIGHT: i32 = 360;

/// Starts a member (double click).
pub type LaunchFn = Rc<dyn Fn(&IconEntry)>;
/// Adds dropped paths to the group with the given id.
pub type AddFn = Rc<dyn Fn(&str, Vec<PathBuf>)>;
/// Removes one member from the group with the given id.
pub type RemoveFn = Rc<dyn Fn(&str, PathBuf)>;

/// What a group window asks of the desktop.
#[derive(Clone)]
pub struct GroupActions {
    pub launch: LaunchFn,
    pub add: AddFn,
    pub remove: RemoveFn,
}

/// A group window and its icon field.
struct Open {
    window: gtk::Window,
    field: IconField,
}

/// The group windows currently open, keyed by group id.
#[derive(Default)]
pub struct GroupWindows {
    open: Rc<RefCell<HashMap<String, Open>>>,
}

/// The icon entries of `group`'s members, in the order they were added. A
/// member that no longer exists is a broken entry.
fn member_entries(group: &Group) -> Vec<IconEntry> {
    group.items.iter().map(|p| scan::entry_for_path(p)).collect()
}

impl GroupWindows {
    /// Open the window of `group`, or refresh and raise it when it is already
    /// open. `icon_set` is the desktop's current icon set.
    pub fn show(&self, app: &gtk::Application, group: &Group, icon_set: &IconSetId, actions: &GroupActions) {
        if let Some(open) = self.open.borrow().get(&group.id) {
            open.window.set_title(Some(&group.name));
            open.field.set_entries(&member_entries(group));
            open.window.present();
            return;
        }

        let window = gtk::Window::builder()
            .application(app)
            .title(&group.name)
            .default_width(WINDOW_WIDTH)
            .default_height(WINDOW_HEIGHT)
            .build();
        let field = IconField::new();
        field.set_mode(LayoutMode::Grid);
        field.set_icon_set(icon_set.clone());
        field.widget().set_hexpand(true);
        field.widget().set_vexpand(true);
        field.set_entries(&member_entries(group));
        window.set_child(Some(field.widget()));

        {
            let field_for_activate = field.clone();
            let launch = actions.launch.clone();
            field.connect_activate(move |i| {
                if let Some(entry) = field_for_activate.entry(i) {
                    launch(&entry);
                }
            });
        }
        {
            let add = actions.add.clone();
            let id = group.id.clone();
            field.connect_drop_uris(move |paths, _target, _shift| add(&id, paths));
        }
        {
            let remove = actions.remove.clone();
            let id = group.id.clone();
            let field_for_menu = field.clone();
            field.connect_context_menu(move |idx, x, y| {
                if idx < 0 {
                    return;
                }
                let Some(entry) = field_for_menu.entry(idx as usize) else { return };
                let (x, y) = field_for_menu
                    .translate_from_canvas(field_for_menu.widget(), x, y)
                    .unwrap_or((x, y));
                popup_remove_menu(field_for_menu.widget(), x, y, &id, entry.path, remove.clone());
            });
        }
        {
            let open = self.open.clone();
            let id = group.id.clone();
            window.connect_close_request(move |_| {
                open.borrow_mut().remove(&id);
                glib::Propagation::Proceed
            });
        }

        window.present();
        self.open.borrow_mut().insert(group.id.clone(), Open { window, field });
    }

    /// Show or hide every open group window (used when the desktop is hidden
    /// or revealed).
    pub fn set_visible(&self, visible: bool) {
        let windows: Vec<gtk::Window> = self.open.borrow().values().map(|o| o.window.clone()).collect();
        for window in windows {
            window.set_visible(visible);
        }
    }

    /// Bring every open window in line with `groups`: new title and members,
    /// or closed when its group is gone.
    pub fn refresh(&self, groups: &Groups) {
        let ids: Vec<String> = self.open.borrow().keys().cloned().collect();
        for id in ids {
            let target = self.open.borrow().get(&id).map(|o| (o.window.clone(), o.field.clone()));
            let Some((window, field)) = target else { continue };
            match groups.get(&id) {
                Some(group) => {
                    window.set_title(Some(&group.name));
                    field.set_entries(&member_entries(group));
                }
                None => window.close(),
            }
        }
    }
}

/// Show a popover with "Remove from Group" at `(x, y)` inside `parent`.
fn popup_remove_menu(
    parent: &gtk::Widget,
    x: f64,
    y: f64,
    id: &str,
    member: PathBuf,
    remove: RemoveFn,
) {
    let popover = gtk::Popover::new();
    popover.set_parent(parent);
    popover.set_has_arrow(true);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    let btn = gtk::Button::with_label("Remove from Group");
    btn.add_css_class("flat");
    {
        let popover = popover.clone();
        let id = id.to_string();
        btn.connect_clicked(move |_| {
            popover.popdown();
            remove(&id, member.clone());
        });
    }
    popover.set_child(Some(&btn));
    popover.connect_closed(|p| p.unparent());
    popover.popup();
}
