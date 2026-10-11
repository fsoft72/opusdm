//! The Function Editor: a manager window listing saved functions (New/
//! Edit/Delete), and a per-function editor window (step list, parameter
//! forms, Test, Save). The manager runs as its own process
//! (`opusdm-lister --manage-functions`), linked to the hub without being a
//! Lister. See `docs/superpowers/specs/2026-09-28-function-editor-design.md`.

use crate::client::{Client, Inbound};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::function::{self, schema, Function, FnStep, InternalArgs, InternalCmd};
use opusdm_ipc::{Resolution, ToHub, ToLister};
use std::cell::RefCell;
use std::rc::Rc;

/// An edit of the step list for one step (move up, move down, remove).
type StepEdit = Box<dyn Fn(&mut Vec<FnStep>)>;

/// Command line flag that makes the Lister open only the Manage Functions
/// window.
pub const MANAGE_FUNCTIONS_FLAG: &str = "--manage-functions";

/// Start the Manage Functions window as its own process (this binary run
/// with [`MANAGE_FUNCTIONS_FLAG`]), so it lives on its own instead of over
/// the Lister that asked for it.
pub fn launch_manager() -> std::io::Result<()> {
    launch_tool(MANAGE_FUNCTIONS_FLAG)
}

/// Start this binary with `flag`, which makes it open one tool window
/// instead of a file window. A thread reaps the process when it exits.
pub fn launch_tool(flag: &str) -> std::io::Result<()> {
    let bin = std::env::current_exe()?;
    let mut child = std::process::Command::new(bin).arg(flag).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// What the Manage Functions window shares with its editors: the hub link,
/// the hub's latest list of functions and the log of the editor whose Test
/// is running.
struct Host {
    client: Client,
    functions: RefCell<Vec<Function>>,
    log: RefCell<Option<gtk::TextBuffer>>,
    window: gtk::Window,
}

impl Host {
    /// Tell the user `text` went wrong, over the manager window.
    fn report(&self, text: &str) {
        opusdm_ui::requester(&self.window, "Manage Functions", text, None, &[("OK", ())], |_| {});
    }

    /// Append `text` to the log of the editor running Test, if one is open.
    fn append_log(&self, text: &str) {
        let Some(buf) = self.log.borrow().clone() else { return };
        let mut end = buf.end_iter();
        buf.insert(&mut end, text);
    }

    /// Handle one message from the hub: Test output, and the questions a
    /// running Test asks (confirm steps and copy conflicts). A failure (for
    /// instance no SOURCE Lister to run on) is also shown in a requester.
    fn handle(self: &Rc<Self>, msg: ToLister) {
        match msg {
            ToLister::FunctionOutput { step_idx, line } => self.append_log(&format!("[step {step_idx}] {line}\n")),
            ToLister::FunctionStepDone { step_idx, exit_code } => {
                self.append_log(&format!("[step {step_idx}] done (exit {exit_code:?})\n"))
            }
            ToLister::FunctionFailed { step_idx, error } => {
                self.append_log(&format!("[step {step_idx}] FAILED: {error}\n"));
                self.report(&error);
            }
            ToLister::FunctionDone => self.append_log("function finished\n"),
            ToLister::OutputLine { line, .. } => self.append_log(&format!("{line}\n")),
            ToLister::ConfirmPrompt { req_id, message } => {
                let host = self.clone();
                opusdm_ui::requester(&self.window, "Confirm", &message, None, &[("OK", true), ("Cancel", false)], move |ok| {
                    host.send(&ToHub::ConfirmReply { req_id, ok });
                });
            }
            ToLister::ConflictPrompt { req_id, path, src, dst, .. } => {
                let host = self.clone();
                let detail = format!("source: {} bytes\ndestination: {} bytes", src.size, dst.size);
                opusdm_ui::requester(
                    &self.window,
                    "Request",
                    &format!("{} already exists", path.display()),
                    Some(&detail),
                    &[
                        ("Overwrite", Resolution::Overwrite),
                        ("Skip", Resolution::Skip),
                        ("Overwrite All", Resolution::OverwriteAll),
                        ("Skip All", Resolution::SkipAll),
                        ("Cancel", Resolution::Cancel),
                    ],
                    move |resolution| host.send(&ToHub::ConflictReply { req_id, resolution }),
                );
            }
            _ => {}
        }
    }

    /// Send `msg` to the hub, reporting a failure to the user.
    fn send(&self, msg: &ToHub) {
        if let Err(e) = self.client.send(msg) {
            self.report(&format!("lost the connection to the hub: {e}"));
        }
    }
}

/// Open the "Manage Functions" window on its own, owned by `app` (which
/// exits when it is closed): every saved function, with New/Edit/Delete
/// buttons. It talks to the hub without registering as a Lister; a running
/// hub is required, since the hub is the only writer of `functions.json`.
/// A second launch only raises the window already open.
pub fn open_standalone(app: &gtk::Application) {
    if let Some(window) = app.windows().first() {
        window.present();
        return;
    }
    let shell = opusdm_ui::DialogShell::standalone(app, "Manage Functions").size(420, 360);
    let mut client = match Client::connect_tool(&opusdm_ipc::sockpath::hub_sock_path()) {
        Ok(client) => client,
        Err(e) => {
            let window = shell.window().clone();
            shell.present();
            let to_close = window.clone();
            opusdm_ui::requester(&window, "Manage Functions", &format!("cannot reach the OpusDM hub: {e}"), None, &[("OK", ())], move |_| {
                to_close.destroy();
            });
            return;
        }
    };
    let inbox = client.take_inbox();
    // Start from the hub's copy of the Functions: it owns `functions.json`.
    gtk::glib::spawn_future_local(async move {
        let functions = match crate::client::fetch_config(&client, &inbox).await {
            Ok((_, functions)) => functions,
            Err(e) => {
                let window = shell.window().clone();
                shell.present();
                let to_close = window.clone();
                opusdm_ui::requester(&window, "Manage Functions", &format!("cannot read the Functions from the hub: {e}"), None, &[("OK", ())], move |_| {
                    to_close.destroy();
                });
                return;
            }
        };
        let host = Rc::new(Host { client, functions: RefCell::new(functions), log: RefCell::new(None), window: shell.window().clone() });
        build_manager(&host, shell);
        while let Ok(inbound) = inbox.recv().await {
            match inbound {
                Inbound::Msg(msg) => host.handle(msg),
                Inbound::HubGone => {
                    // The hub quit: nothing could be saved any more.
                    host.window.destroy();
                    return;
                }
            }
        }
    });
}

/// Fill `shell` with the function list and its New/Edit/Delete buttons.
fn build_manager(host: &Rc<Host>, shell: opusdm_ui::DialogShell) {
    let root = shell.content();

    let list_box = gtk::ListBox::new();
    let functions: Rc<RefCell<Vec<Function>>> = Rc::new(RefCell::new(host.functions.borrow().clone()));

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
        let host = host.clone();
        let functions = functions.clone();
        let window = shell.window().clone();
        let refresh = refresh.clone();
        new_btn.connect_clicked(move |_| {
            let host = host.clone();
            let functions = functions.clone();
            let window = window.clone();
            let refresh = refresh.clone();
            opusdm_ui::requester_entry(&window, "New Function", "", &[("Create", true), ("Cancel", false)], move |ok, name| {
                if !ok || name.trim().is_empty() {
                    return;
                }
                let f = Function::new(name.trim());
                open_function_editor(&host, f, functions.clone(), refresh.clone());
            });
        });
    }
    {
        let host = host.clone();
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
            open_function_editor(&host, f, functions.clone(), refresh.clone());
        });
    }
    {
        let host = host.clone();
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
            let (host, functions, refresh) = (host.clone(), functions.clone(), refresh.clone());
            let Some(id) = functions.borrow().get(idx as usize).map(|f| f.id.clone()) else { return };
            opusdm_ui::requester(&window, "Delete Function", "Delete this function?", None, &[("Delete", true), ("Cancel", false)], move |ok| {
                if !ok {
                    return;
                }
                if let Err(e) = delete_function(&host, &id) {
                    host.report(&format!("could not delete function: {e}"));
                }
                *functions.borrow_mut() = host.functions.borrow().clone();
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

/// Replace the saved Functions: tell the hub (which owns `functions.json`)
/// and update the manager's copy at once.
fn publish_functions(host: &Host, list: Vec<Function>) -> std::io::Result<()> {
    host.client.send(&ToHub::SetFunctions(list.clone()))?;
    *host.functions.borrow_mut() = list;
    Ok(())
}

/// Add `f`, or replace the saved function with the same id, on top of the
/// latest list.
fn upsert_function(host: &Host, f: Function) -> std::io::Result<()> {
    let mut all = host.functions.borrow().clone();
    match all.iter_mut().find(|existing| existing.id == f.id) {
        Some(slot) => *slot = f,
        None => all.push(f),
    }
    publish_functions(host, all)
}

/// Remove the saved function `id` from the latest list.
fn delete_function(host: &Host, id: &str) -> std::io::Result<()> {
    let mut all = host.functions.borrow().clone();
    all.retain(|f| f.id != id);
    publish_functions(host, all)
}

/// Open the editor for one function. `all_functions` is the manager's shared
/// list; Save replaces this function's entry (by id) in place, persists the
/// whole list, and calls `on_saved` so the manager window's list repaints.
fn open_function_editor(host: &Rc<Host>, function: Function, all_functions: Rc<RefCell<Vec<Function>>>, on_saved: Rc<dyn Fn()>) {
    let shell = opusdm_ui::DialogShell::new(&host.window, "Edit Function").size(520, 480).scrolled();
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
        let host = host.clone();
        let state = state.clone();
        let all_functions = all_functions.clone();
        save_btn.connect_clicked(move |_| {
            let f = state.function.borrow().clone();
            match upsert_function(&host, f) {
                Ok(()) => {
                    // Show the manager the list as the hub now has it.
                    *all_functions.borrow_mut() = host.functions.borrow().clone();
                    on_saved();
                }
                Err(e) => host.report(&format!("could not save function: {e}")),
            }
        });
    }
    {
        let host = host.clone();
        let state = state.clone();
        test_btn.connect_clicked(move |_| {
            let f = state.function.borrow().clone();
            state.log_buffer.set_text("");
            *host.log.borrow_mut() = Some(state.log_buffer.clone());
            host.send(&ToHub::RunFunction { steps: f.steps });
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

/// Move-up/move-down/remove for the step at `idx`.
fn step_move_and_remove(idx: usize) -> [(&'static str, StepEdit); 3] {
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

/// One checkbox parameter: writes `"true"` into `InternalArgs` when checked
/// and removes the key when unchecked.
fn build_flag_field(state: &Rc<EditorState>, step_idx: usize, spec: &schema::ParamSpec) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let check = gtk::CheckButton::with_label(spec.label);
    if let Some(FnStep::Internal { args, .. }) = state.function.borrow().steps.get(step_idx) {
        check.set_active(args.flag(spec.key));
    }
    let key = spec.key;
    let state_clone = state.clone();
    check.connect_toggled(move |c| {
        if let Some(FnStep::Internal { args, .. }) = state_clone.function.borrow_mut().steps.get_mut(step_idx) {
            if c.is_active() {
                args.0.insert(key.to_string(), opusdm_ipc::function::BOOL_TRUE.to_string());
            } else {
                args.0.remove(key);
            }
        }
        update_button_states(&state_clone);
    });
    row.append(&check);
    row
}

/// The entries of a bank drop-down: the saved bank names, preceded by the
/// value the step already holds when it is not among them (marked missing),
/// so opening a step whose bank was renamed or deleted never rewrites it
/// silently. Returns the entries and the index of `current` in them.
fn bank_choices(names: &[String], current: Option<&str>) -> (Vec<String>, Option<usize>) {
    let current = current.map(str::trim).filter(|c| !c.is_empty());
    match current {
        Some(c) if !names.iter().any(|n| n == c) => {
            let mut all = vec![format!("{c} (missing)")];
            all.extend(names.iter().cloned());
            (all, Some(0))
        }
        Some(c) => (names.to_vec(), names.iter().position(|n| n == c)),
        None => (names.to_vec(), None),
    }
}

/// One button-bank parameter: a drop-down of the saved banks, read from disk
/// now. A step with no bank yet gets the first one stored, so what the
/// drop-down shows is what gets saved. A bank the step holds that no longer
/// exists stays selected as "(missing)" until the user picks another. With no
/// banks at all the field says so and the required parameter stays missing.
fn build_bank_field(state: &Rc<EditorState>, step_idx: usize, spec: &schema::ParamSpec) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::new(Some(spec.label));
    label.set_xalign(0.0);
    row.append(&label);

    let names = opusdm_ipc::buttonbank::Banks::load().names();
    let key = spec.key;
    let current = match state.function.borrow().steps.get(step_idx) {
        Some(FnStep::Internal { args, .. }) => args.get(key).map(str::to_string),
        _ => None,
    };
    if names.is_empty() && current.as_deref().is_none_or(|c| c.trim().is_empty()) {
        row.append(&gtk::Label::new(Some("(no button banks yet, create one first)")));
        return row;
    }
    let (choices, selected) = bank_choices(&names, current.as_deref());
    let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&refs);
    match selected {
        Some(i) => dropdown.set_selected(i as u32),
        None => {
            if let Some(FnStep::Internal { args, .. }) = state.function.borrow_mut().steps.get_mut(step_idx) {
                args.0.insert(key.to_string(), choices[0].clone());
            }
        }
    }
    let state_clone = state.clone();
    dropdown.connect_selected_notify(move |d| {
        let Some(choice) = choices.get(d.selected() as usize) else { return };
        // Picking the "(missing)" entry again keeps the stale value as it was.
        let value = choice.strip_suffix(" (missing)").unwrap_or(choice).to_string();
        if let Some(FnStep::Internal { args, .. }) = state_clone.function.borrow_mut().steps.get_mut(step_idx) {
            args.0.insert(key.to_string(), value);
        }
        update_button_states(&state_clone);
    });
    row.append(&dropdown);
    row
}

/// One parameter entry, writing back into `InternalArgs` on every
/// keystroke and re-validating so Save/Test flip live. Checkbox parameters
/// go through [`build_flag_field`].
fn build_param_field(state: &Rc<EditorState>, step_idx: usize, spec: &schema::ParamSpec) -> gtk::Box {
    if spec.kind == schema::ParamKind::Bool {
        return build_flag_field(state, step_idx, spec);
    }
    if spec.kind == schema::ParamKind::Bank {
        return build_bank_field(state, step_idx, spec);
    }
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
    InternalCmd::CopyAs,
    InternalCmd::MoveAs,
    InternalCmd::Duplicate,
    InternalCmd::ChangeDir,
    InternalCmd::SetBackground,
    InternalCmd::LeaveOut,
    InternalCmd::PutAway,
    InternalCmd::Hide,
    InternalCmd::Reveal,
    InternalCmd::ToggleHide,
    InternalCmd::DesktopPrev,
    InternalCmd::DesktopNext,
    InternalCmd::OpenInputConfig,
    InternalCmd::OpenBank,
    InternalCmd::CloseBank,
    InternalCmd::ToggleBank,
    InternalCmd::HideBank,
    InternalCmd::Confirm,
    InternalCmd::Run,
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

#[cfg(test)]
mod tests {
    use super::bank_choices;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_known_bank_is_selected_in_place() {
        assert_eq!(bank_choices(&names(&["A", "B"]), Some("B")), (names(&["A", "B"]), Some(1)));
    }

    #[test]
    fn a_missing_bank_stays_selected_as_missing_instead_of_being_replaced() {
        assert_eq!(bank_choices(&names(&["A", "B"]), Some("Old")), (names(&["Old (missing)", "A", "B"]), Some(0)));
        assert_eq!(bank_choices(&[], Some("Old")), (names(&["Old (missing)"]), Some(0)));
    }

    #[test]
    fn a_step_without_a_bank_selects_nothing_yet() {
        assert_eq!(bank_choices(&names(&["A"]), None), (names(&["A"]), None));
        assert_eq!(bank_choices(&names(&["A"]), Some("  ")), (names(&["A"]), None));
    }
}
