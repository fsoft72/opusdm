//! The script events editor (F-05): one row per binding, an event
//! drop-down and a function drop-down, with live validation.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, BindingError, ScriptBinding, ScriptEvent};
use opusdm_ipc::function::{Function, FunctionId};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// Everything the editor's callbacks share.
struct State {
    scripts: RefCell<Vec<ScriptBinding>>,
    functions: Vec<Function>,
    events: Vec<ScriptEvent>,
    rows_box: gtk::Box,
    save_btn: gtk::Button,
}

/// Open the "Edit Script Events" window, working on a copy of the Lister's
/// cached script bindings. Save sends only that section back to the hub.
pub fn open_scripts_editor(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Edit Script Events").size(600, 380).scrolled();
    let root = shell.content();

    let rows_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.append(&rows_box);
    let add_btn = gtk::Button::with_label("Add Binding");
    add_btn.set_halign(gtk::Align::Start);
    root.append(&add_btn);

    let save_btn = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Close");

    let state = Rc::new(State {
        scripts: RefCell::new(ui.hub.bindings.borrow().scripts.clone()),
        functions: ui.hub.functions.borrow().clone(),
        events: ScriptEvent::emitted(),
        rows_box,
        save_btn: save_btn.clone(),
    });

    {
        let state = state.clone();
        add_btn.connect_clicked(move |_| {
            let Some(first) = state.functions.first() else { return };
            state.scripts.borrow_mut().push(ScriptBinding { event: state.events[0], function: first.id.clone() });
            rebuild(&state);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        save_btn.connect_clicked(move |_| {
            let mut updated = ui.hub.bindings.borrow().clone();
            updated.scripts = state.scripts.borrow().clone();
            let _ = ui.hub.client.send(&opusdm_ipc::ToHub::SetBindings(updated));
            crate::actions::flash(&ui, "script events saved");
        });
    }

    rebuild(&state);
    shell.present();
}

/// The set of saved function ids.
fn known_ids(functions: &[Function]) -> HashSet<FunctionId> {
    functions.iter().map(|f| f.id.clone()).collect()
}

/// Repaint every row and re-run the validation that gates Save.
fn rebuild(state: &Rc<State>) {
    while let Some(child) = state.rows_box.first_child() {
        state.rows_box.remove(&child);
    }
    let scripts = state.scripts.borrow().clone();
    let errors = bindings::validate_scripts(&scripts, &known_ids(&state.functions));
    for (index, binding) in scripts.iter().enumerate() {
        state.rows_box.append(&build_row(state, index, binding, &errors));
    }
    state.save_btn.set_sensitive(errors.is_empty());
}

/// One row: event drop-down, function drop-down, remove button, errors below.
fn build_row(state: &Rc<State>, index: usize, binding: &ScriptBinding, errors: &[BindingError]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let labels: Vec<&str> = state.events.iter().map(|e| e.label()).collect();
    let event_dropdown = gtk::DropDown::from_strings(&labels);
    event_dropdown.set_selected(
        state.events.iter().position(|e| *e == binding.event).map(|p| p as u32).unwrap_or(gtk::INVALID_LIST_POSITION),
    );
    {
        let state = state.clone();
        event_dropdown.connect_selected_notify(move |d| {
            let Some(event) = state.events.get(d.selected() as usize).copied() else { return };
            if let Some(b) = state.scripts.borrow_mut().get_mut(index) {
                if b.event == event {
                    return;
                }
                b.event = event;
            }
            rebuild(&state);
        });
    }
    line.append(&event_dropdown);

    let function_dropdown = opusdm_ui::function_dropdown(&state.functions, &binding.function);
    function_dropdown.set_hexpand(true);
    {
        let state = state.clone();
        function_dropdown.connect_selected_notify(move |d| {
            let Some(id) = opusdm_ui::selected_function(d, &state.functions) else { return };
            if let Some(b) = state.scripts.borrow_mut().get_mut(index) {
                if b.function == id {
                    return;
                }
                b.function = id;
            }
            rebuild(&state);
        });
    }
    line.append(&function_dropdown);

    let remove = gtk::Button::from_icon_name("list-remove-symbolic");
    remove.add_css_class("flat");
    {
        let state = state.clone();
        remove.connect_clicked(move |_| {
            let mut scripts = state.scripts.borrow_mut();
            if index < scripts.len() {
                scripts.remove(index);
            }
            drop(scripts);
            rebuild(&state);
        });
    }
    line.append(&remove);
    row.append(&line);

    for error in errors.iter().filter(|e| e.script_index() == Some(index)) {
        let note = gtk::Label::new(Some(&error.to_string()));
        note.set_xalign(0.0);
        note.set_margin_start(6);
        note.add_css_class("error");
        row.append(&note);
    }
    row
}
