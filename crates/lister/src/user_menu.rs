//! The lister's user menu (F-04): a "Menu" button whose popup is built from
//! the cached `Bindings` every time it opens, so an edit made in any Lister
//! (or a function deleted meanwhile) shows up with no explicit refresh.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, MenuNode};
use opusdm_ipc::function::{self, FunctionId};
use std::collections::HashSet;
use std::rc::Rc;

/// Window action that runs a menu entry; its parameter is the function id.
const ACTION_RUN: &str = "usermenu-run";

/// Window action that opens the menu editor.
const ACTION_EDIT: &str = "usermenu-edit";

/// The sections of `nodes`: a separator closes the current section, entries
/// and submenus go into it, empty sections are dropped.
fn build_sections(nodes: &[MenuNode]) -> Vec<gio::Menu> {
    let mut sections = Vec::new();
    let mut section = gio::Menu::new();
    for node in nodes {
        match node {
            MenuNode::Entry { label, function } => {
                let item = gio::MenuItem::new(Some(label), None);
                item.set_action_and_target_value(Some(&format!("win.{ACTION_RUN}")), Some(&function.to_variant()));
                section.append_item(&item);
            }
            MenuNode::Submenu { label, children } => {
                section.append_submenu(Some(label), &build_menu(children));
            }
            MenuNode::Separator => {
                if section.n_items() > 0 {
                    sections.push(std::mem::replace(&mut section, gio::Menu::new()));
                }
            }
        }
    }
    if section.n_items() > 0 {
        sections.push(section);
    }
    sections
}

/// A `gio::Menu` of `nodes`, one section per separator-delimited group.
pub fn build_menu(nodes: &[MenuNode]) -> gio::Menu {
    let menu = gio::Menu::new();
    for section in build_sections(nodes) {
        menu.append_section(None, &section);
    }
    menu
}

/// The full popup model: the user's menu followed by a fixed section holding
/// "Edit User Menu...", so an empty menu still offers a way to fill it.
pub fn build_root_menu(nodes: &[MenuNode]) -> gio::Menu {
    let root = build_menu(nodes);
    let edit = gio::Menu::new();
    edit.append(Some("Edit User Menu..."), Some(&format!("win.{ACTION_EDIT}")));
    root.append_section(None, &edit);
    root
}

/// The set of saved function ids.
fn known_ids() -> HashSet<FunctionId> {
    function::load_functions().into_iter().map(|f| f.id).collect()
}

/// Register the two window actions and make the "Menu" button build its
/// popup lazily from the cached bindings. Holds only a weak reference to
/// `ui`, so the button and the window do not keep each other alive.
pub fn install(ui: &Rc<Ui>) {
    let run = gio::SimpleAction::new(ACTION_RUN, Some(glib::VariantTy::STRING));
    {
        let ui = Rc::downgrade(ui);
        run.connect_activate(move |_, parameter| {
            let (Some(ui), Some(id)) = (ui.upgrade(), parameter.and_then(|p| p.get::<String>())) else { return };
            ui.start_run(opusdm_ipc::ToHub::RunFunctionById { id });
        });
    }
    ui.window.add_action(&run);

    let edit = gio::SimpleAction::new(ACTION_EDIT, None);
    {
        let ui = Rc::downgrade(ui);
        edit.connect_activate(move |_, _| {
            let Some(ui) = ui.upgrade() else { return };
            crate::menu_editor::open_menu_editor(&ui);
        });
    }
    ui.window.add_action(&edit);

    let weak = Rc::downgrade(ui);
    ui.user_menu_btn.set_create_popup_func(move |button| {
        let Some(ui) = weak.upgrade() else { return };
        let tree = bindings::prune(&ui.hub.bindings.borrow().menu, &known_ids());
        button.set_menu_model(Some(&build_root_menu(&tree)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::gio::prelude::*;

    fn entry(label: &str, function: &str) -> MenuNode {
        MenuNode::Entry { label: label.into(), function: function.into() }
    }

    fn section_len(menu: &gio::Menu, index: i32) -> i32 {
        menu.item_link(index, "section").expect("a section").n_items()
    }

    #[test]
    fn separators_split_the_menu_into_sections() {
        let tree = vec![
            entry("a", "f1"),
            MenuNode::Separator,
            entry("b", "f2"),
            MenuNode::Submenu { label: "c".into(), children: vec![entry("d", "f3")] },
        ];
        let menu = build_menu(&tree);
        assert_eq!(menu.n_items(), 2);
        assert_eq!(section_len(&menu, 0), 1);
        assert_eq!(section_len(&menu, 1), 2);
    }

    #[test]
    fn empty_sections_and_leading_separators_leave_no_gap() {
        let tree = vec![MenuNode::Separator, MenuNode::Separator, entry("a", "f1"), MenuNode::Separator];
        let menu = build_menu(&tree);
        assert_eq!(menu.n_items(), 1);
        assert_eq!(section_len(&menu, 0), 1);
    }

    #[test]
    fn the_root_menu_always_ends_with_the_edit_section() {
        let empty = build_root_menu(&[]);
        assert_eq!(empty.n_items(), 1);
        assert_eq!(section_len(&empty, 0), 1);
        let filled = build_root_menu(&[entry("a", "f1")]);
        assert_eq!(filled.n_items(), 2);
    }

    #[test]
    fn an_entry_carries_the_run_action_and_its_function_id() {
        let menu = build_menu(&[entry("a", "fn-42")]);
        let section = menu.item_link(0, "section").unwrap();
        let action = section.item_attribute_value(0, "action", None).and_then(|v| v.get::<String>());
        let target = section.item_attribute_value(0, "target", None).and_then(|v| v.get::<String>());
        assert_eq!(action.as_deref(), Some("win.usermenu-run"));
        assert_eq!(target.as_deref(), Some("fn-42"));
    }
}
