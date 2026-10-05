//! The user menu editor (F-04): the menu tree as indented rows with move,
//! rename, remove and add controls. Modeled on
//! `crates/desktop-config/src/menu_ui.rs`, editing functions instead of apps.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, BindingError, MenuNode};
use opusdm_ipc::function::{Function, FunctionId};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// Everything the editor's callbacks share.
struct State {
    tree: RefCell<Vec<MenuNode>>,
    functions: Vec<Function>,
    body: gtk::Box,
    save_btn: gtk::Button,
    window: gtk::Window,
    ui: Rc<Ui>,
}

/// Open the "Edit User Menu" window, working on a copy of the Lister's cached
/// menu. Save sends only the menu section back to the hub.
pub fn open_menu_editor(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Edit User Menu").size(600, 480).scrolled();
    let root = shell.content();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.append(&body);

    let save_btn = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Close");

    let state = Rc::new(State {
        tree: RefCell::new(ui.hub.bindings.borrow().menu.clone()),
        functions: ui.hub.functions.borrow().clone(),
        body,
        save_btn: save_btn.clone(),
        window: shell.window().clone(),
        ui: ui.clone(),
    });

    root.prepend(&add_buttons(&state, Vec::new()));

    {
        let state = state.clone();
        save_btn.connect_clicked(move |_| {
            let mut updated = state.ui.hub.bindings.borrow().clone();
            updated.menu = state.tree.borrow().clone();
            let _ = state.ui.hub.client.send(&opusdm_ipc::ToHub::SetBindings(updated));
            crate::actions::flash(&state.ui, "user menu saved");
        });
    }

    rebuild(&state);
    shell.present();
}

/// The set of saved function ids.
fn known_ids(functions: &[Function]) -> HashSet<FunctionId> {
    functions.iter().map(|f| f.id.clone()).collect()
}

/// Repaint the whole tree and re-run the validation that gates Save.
fn rebuild(state: &Rc<State>) {
    while let Some(child) = state.body.first_child() {
        state.body.remove(&child);
    }
    let tree = state.tree.borrow().clone();
    let errors = bindings::validate_menu(&tree, &known_ids(&state.functions));
    build_level(state, &tree, &mut Vec::new(), &errors);
    state.save_btn.set_sensitive(errors.is_empty());
}

/// Append the rows of `nodes` (and, depth first, their children); `path` is
/// the position of `nodes` in the tree.
fn build_level(state: &Rc<State>, nodes: &[MenuNode], path: &mut Vec<usize>, errors: &[BindingError]) {
    for (index, node) in nodes.iter().enumerate() {
        path.push(index);
        state.body.append(&build_row(state, node, path, errors));
        if let MenuNode::Submenu { children, .. } = node {
            build_level(state, children, path, errors);
        }
        path.pop();
    }
}

/// One row: an indented description, move/rename/remove buttons, add buttons
/// for a submenu, and this node's errors underneath.
fn build_row(state: &Rc<State>, node: &MenuNode, path: &[usize], errors: &[BindingError]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    row.set_margin_start(18 * (path.len() as i32 - 1));
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let text = match node {
        MenuNode::Entry { label, function } => format!("{label}  ->  {}", opusdm_ipc::funcref::title(function, &state.functions)),
        MenuNode::Submenu { label, .. } => format!("{label}  (submenu)"),
        MenuNode::Separator => "--------".to_string(),
    };
    let label = gtk::Label::new(Some(&text));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    line.append(&label);

    if !matches!(node, MenuNode::Separator) {
        line.append(&rename_button(state, node, path));
    }
    line.append(&tree_button(state, "go-up-symbolic", path, |t, p| bindings::move_node(t, p, -1)));
    line.append(&tree_button(state, "go-down-symbolic", path, |t, p| bindings::move_node(t, p, 1)));
    line.append(&tree_button(state, "list-remove-symbolic", path, bindings::remove_node));
    if matches!(node, MenuNode::Submenu { .. }) {
        line.append(&add_buttons(state, path.to_vec()));
    }
    row.append(&line);

    for error in errors.iter().filter(|e| e.menu_path() == Some(path)) {
        let note = gtk::Label::new(Some(&error.to_string()));
        note.set_xalign(0.0);
        note.add_css_class("error");
        row.append(&note);
    }
    row
}

/// A flat icon button that applies `action` to the node at `path`, then
/// repaints when it changed anything.
fn tree_button(
    state: &Rc<State>,
    icon: &str,
    path: &[usize],
    action: impl Fn(&mut Vec<MenuNode>, &[usize]) -> bool + 'static,
) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("flat");
    let state = state.clone();
    let path = path.to_vec();
    button.connect_clicked(move |_| {
        let changed = action(&mut state.tree.borrow_mut(), &path);
        if changed {
            rebuild(&state);
        }
    });
    button
}

/// The rename button of an entry or submenu row.
fn rename_button(state: &Rc<State>, node: &MenuNode, path: &[usize]) -> gtk::Button {
    let current = match node {
        MenuNode::Entry { label, .. } | MenuNode::Submenu { label, .. } => label.clone(),
        MenuNode::Separator => String::new(),
    };
    let button = gtk::Button::from_icon_name("document-edit-symbolic");
    button.add_css_class("flat");
    let state = state.clone();
    let path = path.to_vec();
    button.connect_clicked(move |_| {
        let state = state.clone();
        let path = path.clone();
        opusdm_ui::requester_entry(&state.window.clone(), "Rename", &current, &[("Rename", true), ("Cancel", false)], move |ok, text| {
            if !ok || text.trim().is_empty() {
                return;
            }
            bindings::rename_node(&mut state.tree.borrow_mut(), &path, text.trim());
            rebuild(&state);
        });
    });
    button
}

/// "+ Entry", "+ Submenu" and "+ Separator" buttons that add into the list at
/// `parent` (an empty path is the root list).
fn add_buttons(state: &Rc<State>, parent: Vec<usize>) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let entry_btn = gtk::Button::with_label("+ Entry");
    let submenu_btn = gtk::Button::with_label("+ Submenu");
    let separator_btn = gtk::Button::with_label("+ Separator");
    for button in [&entry_btn, &submenu_btn, &separator_btn] {
        button.add_css_class("flat");
        bar.append(button);
    }
    {
        let state = state.clone();
        let parent = parent.clone();
        entry_btn.connect_clicked(move |_| prompt_add_entry(&state, parent.clone()));
    }
    {
        let state = state.clone();
        let parent = parent.clone();
        submenu_btn.connect_clicked(move |_| {
            let state = state.clone();
            let parent = parent.clone();
            opusdm_ui::requester_entry(&state.window.clone(), "New Submenu", "", &[("Create", true), ("Cancel", false)], move |ok, text| {
                if !ok || text.trim().is_empty() {
                    return;
                }
                let node = MenuNode::Submenu { label: text.trim().to_string(), children: Vec::new() };
                bindings::add_node(&mut state.tree.borrow_mut(), &parent, node);
                rebuild(&state);
            });
        });
    }
    {
        let state = state.clone();
        separator_btn.connect_clicked(move |_| {
            bindings::add_node(&mut state.tree.borrow_mut(), &parent, MenuNode::Separator);
            rebuild(&state);
        });
    }
    bar
}

/// Ask for what to run (a saved function, a bash command or a program) and a
/// label, then add an entry to the list at `parent`. The label follows the
/// chosen target until the user edits it.
fn prompt_add_entry(state: &Rc<State>, parent: Vec<usize>) {
    let shell = opusdm_ui::DialogShell::new(&state.window, "Add Menu Entry");
    let first = state.functions.first().map(|f| f.id.clone()).unwrap_or_default();
    let reference = Rc::new(RefCell::new(first.clone()));
    let label_entry = gtk::Entry::new();
    label_entry.set_text(&opusdm_ipc::funcref::title(&first, &state.functions));
    // The last text the label got from the target: while the label still
    // holds it (or is empty), a new target replaces it.
    let last_auto = Rc::new(RefCell::new(label_entry.text().to_string()));
    let picker = {
        let (state, reference, label_entry, last_auto) = (state.clone(), reference.clone(), label_entry.clone(), last_auto.clone());
        opusdm_ui::RefPicker::new(&state.functions.clone(), &first, &state.window.clone(), None, move |chosen| {
            let text = opusdm_ipc::funcref::title(&chosen, &state.functions);
            let current = label_entry.text().to_string();
            if current.is_empty() || current == *last_auto.borrow() {
                label_entry.set_text(&text);
                *last_auto.borrow_mut() = text;
            }
            *reference.borrow_mut() = chosen;
        })
    };
    shell.content().append(picker.widget());
    shell.content().append(&label_entry);
    let add_btn = shell.button("Add");
    shell.close_button("Cancel");
    let window = shell.window().clone();
    let state = state.clone();
    add_btn.connect_clicked(move |_| {
        let label = label_entry.text().trim().to_string();
        let function = reference.borrow().clone();
        if label.is_empty() || function.is_empty() {
            return;
        }
        bindings::add_node(&mut state.tree.borrow_mut(), &parent, MenuNode::Entry { label, function });
        rebuild(&state);
        window.destroy();
    });
    shell.present();
}
