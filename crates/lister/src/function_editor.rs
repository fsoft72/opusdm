//! The Function Editor: a manager window listing saved functions (New/
//! Edit/Delete), and a per-function editor window (step list, parameter
//! forms, Test, Save). See
//! `docs/superpowers/specs/2026-09-28-function-editor-design.md`.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::function::{self, schema, Function, FnStep, InternalArgs, InternalCmd};
use std::cell::RefCell;
use std::rc::Rc;

/// Open the "Manage Functions" window: every saved function, with New/Edit/
/// Delete buttons.
pub fn open_functions_manager(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Manage Functions").size(420, 360);
    let root = shell.content();

    let list_box = gtk::ListBox::new();
    let functions: Rc<RefCell<Vec<Function>>> = Rc::new(RefCell::new(function::load_functions()));

    fn rebuild(list_box: &gtk::ListBox, functions: &[Function]) {
        while let Some(row) = list_box.first_child() {
            list_box.remove(&row);
        }
        for f in functions {
            let row = gtk::ListBoxRow::new();
            let label = gtk::Label::new(Some(&f.name));
            label.set_xalign(0.0);
            label.set_margin_start(6);
            label.set_margin_end(6);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            row.set_child(Some(&label));
            list_box.append(&row);
        }
    }
    rebuild(&list_box, &functions.borrow());

    let scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).build();
    root.append(&scroller);

    let new_btn = shell.button("New Function...");
    let edit_btn = shell.button("Edit...");
    let delete_btn = shell.button("Delete");
    shell.button_spacer();
    shell.close_button("Close");

    // Called after anything mutates `functions` (New/Edit/Delete), instead
    // of a timed poll: an earlier version rebuilt the list every 300ms
    // whether or not anything changed, which also destroyed and recreated
    // every row on that tick - dropping whatever the user had selected out
    // from under them well before a click on Edit/Delete could reach it,
    // and the timer itself was never cancelled when this window closed, so
    // each time the manager was reopened another one kept running. This
    // closure rebuilds only on an actual change and the window owns it, so
    // it simply stops existing (and is only ever called) while the window
    // is open.
    let refresh: Rc<dyn Fn()> = {
        let list_box = list_box.clone();
        let functions = functions.clone();
        Rc::new(move || rebuild(&list_box, &functions.borrow()))
    };

    {
        let ui = ui.clone();
        let functions = functions.clone();
        let window = shell.window().clone();
        let refresh = refresh.clone();
        new_btn.connect_clicked(move |_| {
            let ui = ui.clone();
            let functions = functions.clone();
            let window = window.clone();
            let refresh = refresh.clone();
            opusdm_ui::requester_entry(&window, "New Function", "", &[("Create", true), ("Cancel", false)], move |ok, name| {
                if !ok || name.trim().is_empty() {
                    return;
                }
                let f = Function::new(name.trim());
                open_function_editor(&ui, f, functions.clone(), refresh.clone());
            });
        });
    }
    {
        let ui = ui.clone();
        let functions = functions.clone();
        let list_box = list_box.clone();
        let refresh = refresh.clone();
        edit_btn.connect_clicked(move |_| {
            let Some(row) = list_box.selected_row() else { return };
            let idx = row.index();
            if idx < 0 {
                return;
            }
            let Some(f) = functions.borrow().get(idx as usize).cloned() else { return };
            open_function_editor(&ui, f, functions.clone(), refresh.clone());
        });
    }
    {
        let list_box = list_box.clone();
        let functions = functions.clone();
        let window = shell.window().clone();
        let refresh = refresh.clone();
        delete_btn.connect_clicked(move |_| {
            let Some(row) = list_box.selected_row() else { return };
            let idx = row.index();
            if idx < 0 {
                return;
            }
            let functions = functions.clone();
            let refresh = refresh.clone();
            opusdm_ui::requester(&window, "Delete Function", "Delete this function?", None, &[("Delete", true), ("Cancel", false)], move |ok| {
                if !ok {
                    return;
                }
                functions.borrow_mut().remove(idx as usize);
                if let Err(e) = function::save_functions(&functions.borrow()) {
                    opusdm_ipc::err_log!("opusdm-lister", "could not save functions.json: {e}");
                }
                refresh();
            });
        });
    }

    shell.present();
}

struct EditorState {
    function: RefCell<Function>,
    steps_box: gtk::Box,
    log_buffer: gtk::TextBuffer,
    save_btn: gtk::Button,
    test_btn: gtk::Button,
    window: gtk::Window,
}

/// Open the editor for one function. `all_functions` is the manager's shared
/// list; Save replaces this function's entry (by id) in place, persists the
/// whole list, and calls `on_saved` so the manager window's list repaints.
fn open_function_editor(ui: &Rc<Ui>, function: Function, all_functions: Rc<RefCell<Vec<Function>>>, on_saved: Rc<dyn Fn()>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Edit Function").size(520, 480).scrolled();
    let root = shell.content();

    let name_entry = gtk::Entry::new();
    name_entry.set_text(&function.name);
    root.append(&name_entry);

    let steps_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.append(&steps_box);

    let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let add_internal = gtk::Button::with_label("Add Internal Step...");
    let add_external = gtk::Button::with_label("Add External Step...");
    add_row.append(&add_internal);
    add_row.append(&add_external);
    root.append(&add_row);

    let log_view = gtk::TextView::new();
    log_view.set_editable(false);
    log_view.set_monospace(true);
    let log_buffer = log_view.buffer();
    let log_scroller = gtk::ScrolledWindow::builder().child(&log_view).min_content_height(100).build();
    root.append(&log_scroller);

    let save_btn = shell.button("Save");
    let test_btn = shell.button("Test");
    shell.button_spacer();
    shell.close_button("Close");

    let state = Rc::new(EditorState {
        function: RefCell::new(function),
        steps_box,
        log_buffer,
        save_btn: save_btn.clone(),
        test_btn: test_btn.clone(),
        window: shell.window().clone(),
    });

    {
        let state = state.clone();
        name_entry.connect_changed(move |e| {
            state.function.borrow_mut().name = e.text().to_string();
        });
    }
    {
        let state = state.clone();
        add_internal.connect_clicked(move |_| prompt_add_internal(&state));
    }
    {
        let state = state.clone();
        add_external.connect_clicked(move |_| prompt_add_external(&state));
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let all_functions = all_functions.clone();
        save_btn.connect_clicked(move |_| {
            let f = state.function.borrow().clone();
            let saved = {
                let mut all = all_functions.borrow_mut();
                match all.iter_mut().find(|existing| existing.id == f.id) {
                    Some(slot) => *slot = f,
                    None => all.push(f),
                }
                function::save_functions(&all)
            };
            match saved {
                Ok(()) => on_saved(),
                Err(e) => crate::actions::flash(&ui, &format!("could not save function: {e}")),
            }
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        test_btn.connect_clicked(move |_| {
            let f = state.function.borrow().clone();
            state.log_buffer.set_text("");
            *ui.function_log.borrow_mut() = Some(state.log_buffer.clone());
            let _ = ui.hub.client.send(&opusdm_ipc::ToHub::RunFunction { steps: f.steps });
        });
    }

    rebuild_steps(&state);
    update_button_states(&state);
    shell.present();
}

/// Repaint `state.steps_box` from `state.function`'s steps, and refresh
/// Save/Test enabled state to match `function::validate`.
fn rebuild_steps(state: &Rc<EditorState>) {
    while let Some(row) = state.steps_box.first_child() {
        state.steps_box.remove(&row);
    }
    let steps: Vec<FnStep> = state.function.borrow().steps.clone();
    for (i, step) in steps.iter().enumerate() {
        state.steps_box.append(&build_step_row(state, i, step));
    }
    update_button_states(state);
}

fn update_button_states(state: &Rc<EditorState>) {
    let valid = function::validate(&state.function.borrow()).is_empty();
    state.save_btn.set_sensitive(valid);
    state.test_btn.set_sensitive(valid);
}

fn build_step_row(state: &Rc<EditorState>, idx: usize, step: &FnStep) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let summary = match step {
        FnStep::Internal { cmd, .. } => format!("Internal: {}", cmd.label()),
        FnStep::External { argv, .. } => format!("External: {}", argv.join(" ")),
    };
    let label = gtk::Label::new(Some(&summary));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    header.append(&label);

    for (icon, apply) in step_move_and_remove(idx) {
        let btn = gtk::Button::from_icon_name(icon);
        btn.add_css_class("flat");
        let state = state.clone();
        btn.connect_clicked(move |_| {
            apply(&mut state.function.borrow_mut().steps);
            rebuild_steps(&state);
        });
        header.append(&btn);
    }
    row.append(&header);

    // Required-parameter form for an Internal step, shown inline under its
    // summary; External steps have nothing further to configure here (the
    // argv itself, entered at Add time, is their one required field).
    if let FnStep::Internal { cmd, .. } = step {
        let specs = schema::params_for(*cmd);
        if !specs.is_empty() {
            let form = gtk::Box::new(gtk::Orientation::Vertical, 2);
            form.set_margin_start(18);
            let required = gtk::Label::new(Some("Required"));
            required.set_xalign(0.0);
            required.add_css_class("heading");
            form.append(&required);
            for spec in specs.iter().filter(|s| s.required) {
                form.append(&build_param_field(state, idx, spec));
            }
            row.append(&form);
        }
    }
    row
}

/// Move-up/move-down/remove for the step at `idx`, mirroring
/// `crates/desktop-config/src/menu_ui.rs::MenuInner::move_and_remove`.
fn step_move_and_remove(idx: usize) -> [(&'static str, Box<dyn Fn(&mut Vec<FnStep>)>); 3] {
    [
        ("go-up-symbolic", Box::new(move |steps: &mut Vec<FnStep>| {
            if idx > 0 {
                steps.swap(idx, idx - 1);
            }
        })),
        ("go-down-symbolic", Box::new(move |steps: &mut Vec<FnStep>| {
            if idx + 1 < steps.len() {
                steps.swap(idx, idx + 1);
            }
        })),
        ("list-remove-symbolic", Box::new(move |steps: &mut Vec<FnStep>| {
            if idx < steps.len() {
                steps.remove(idx);
            }
        })),
    ]
}

/// One required-parameter entry, writing back into `InternalArgs` on every
/// keystroke and re-validating so Save/Test flip live.
fn build_param_field(state: &Rc<EditorState>, step_idx: usize, spec: &schema::ParamSpec) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::new(Some(spec.label));
    label.set_xalign(0.0);
    row.append(&label);

    let entry = gtk::Entry::new();
    if let Some(FnStep::Internal { args, .. }) = state.function.borrow().steps.get(step_idx) {
        if let Some(v) = args.get(spec.key) {
            entry.set_text(v);
        }
    }
    let key = spec.key;
    let state_clone = state.clone();
    entry.connect_changed(move |e| {
        let text = e.text().to_string();
        if let Some(FnStep::Internal { args, .. }) = state_clone.function.borrow_mut().steps.get_mut(step_idx) {
            args.0.insert(key.to_string(), text);
        }
        update_button_states(&state_clone);
    });
    row.append(&entry);
    row
}

const INTERNAL_CMDS: &[InternalCmd] = &[
    InternalCmd::Copy,
    InternalCmd::Move,
    InternalCmd::Delete,
    InternalCmd::MakeDir,
    InternalCmd::NewFile,
    InternalCmd::Rename,
];

/// Ask which `InternalCmd` to add, append it with empty args, and rebuild.
/// Uses `DialogShell` directly (like the per-function editor window itself)
/// rather than `crates/ui/src/requester.rs`'s helpers, since those are
/// built specifically around a text body or a single `gtk::Entry` and a
/// dropdown does not fit either shape without changing that file's shared
/// `build()` signature - a bigger change than one small dialog justifies.
fn prompt_add_internal(state: &Rc<EditorState>) {
    let shell = opusdm_ui::DialogShell::new(&state.window, "Add Internal Step");
    let labels: Vec<&str> = INTERNAL_CMDS.iter().map(|c| c.label()).collect();
    let dropdown = gtk::DropDown::from_strings(&labels);
    shell.content().append(&dropdown);

    let add_btn = shell.button("Add");
    shell.close_button("Cancel");

    let state = state.clone();
    let window = shell.window().clone();
    add_btn.connect_clicked(move |_| {
        let cmd = INTERNAL_CMDS[dropdown.selected() as usize];
        state.function.borrow_mut().steps.push(FnStep::Internal { cmd, args: InternalArgs::default() });
        rebuild_steps(&state);
        window.destroy();
    });

    shell.present();
}

/// Ask for a shell command line, split it on whitespace into `argv`, append
/// an External step running it in the SOURCE Lister's directory, and
/// rebuild.
fn prompt_add_external(state: &Rc<EditorState>) {
    let state_clone = state.clone();
    opusdm_ui::requester_entry(&state.window, "Add External Step (e.g. notify-send done {o})", "", &[("Add", true), ("Cancel", false)], move |ok, text| {
        if !ok || text.trim().is_empty() {
            return;
        }
        let argv: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        state_clone.function.borrow_mut().steps.push(FnStep::External { argv, cwd_source: true });
        rebuild_steps(&state_clone);
    });
}
