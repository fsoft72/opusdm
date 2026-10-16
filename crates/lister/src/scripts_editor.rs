//! The script events editor (F-05): one row per binding, an event
//! drop-down and a function picker (saved function, bash command or
//! program), with live validation.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, ScriptBinding, ScriptEvent};
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
    /// One box per row for its error notes, repainted in place so typing in a
    /// command field keeps the focus.
    notes: RefCell<Vec<gtk::Box>>,
    save_btn: gtk::Button,
    window: gtk::Window,
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
        notes: RefCell::new(Vec::new()),
        save_btn: save_btn.clone(),
        window: shell.window().clone(),
    });

    {
        let state = state.clone();
        add_btn.connect_clicked(move |_| {
            let first = state.functions.first().map(|f| f.id.clone()).unwrap_or_default();
            state.scripts.borrow_mut().push(ScriptBinding { event: state.events[0], function: first });
            rebuild(&state);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        save_btn.connect_clicked(move |_| {
            let mut updated = ui.hub.bindings.borrow().clone();
            updated.scripts = state.scripts.borrow().clone();
            ui.send_hub(&opusdm_ipc::ToHub::SetBindings(updated));
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
    state.notes.borrow_mut().clear();
    let scripts = state.scripts.borrow().clone();
    for (index, binding) in scripts.iter().enumerate() {
        state.rows_box.append(&build_row(state, index, binding));
    }
    revalidate(state);
}

/// Re-run the validation and repaint every row's notes in place, without
/// touching the widgets that have the keyboard focus. Save is enabled only
/// when nothing is wrong.
fn revalidate(state: &Rc<State>) {
    let scripts = state.scripts.borrow().clone();
    let errors = bindings::validate_scripts(&scripts, &known_ids(&state.functions));
    for (index, notes) in state.notes.borrow().iter().enumerate() {
        while let Some(child) = notes.first_child() {
            notes.remove(&child);
        }
        for error in errors.iter().filter(|e| e.script_index() == Some(index)) {
            let note = gtk::Label::new(Some(&error.to_string()));
            note.set_xalign(0.0);
            note.set_margin_start(6);
            note.add_css_class("error");
            notes.append(&note);
        }
    }
    state.save_btn.set_sensitive(errors.is_empty());
}

/// One row: event drop-down, function picker, remove button, errors below.
fn build_row(state: &Rc<State>, index: usize, binding: &ScriptBinding) -> gtk::Box {
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

    let picker = {
        let on_change_state = state.clone();
        opusdm_ui::RefPicker::new(&state.functions, &binding.function, &state.window, None, move |reference| {
            if let Some(b) = on_change_state.scripts.borrow_mut().get_mut(index) {
                if b.function == reference {
                    return;
                }
                b.function = reference;
            }
            revalidate(&on_change_state);
        })
    };
    picker.widget().set_hexpand(true);
    line.append(picker.widget());

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

    let notes = gtk::Box::new(gtk::Orientation::Vertical, 2);
    row.append(&notes);
    state.notes.borrow_mut().push(notes);
    row
}
