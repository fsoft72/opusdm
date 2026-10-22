//! The hotkey editor (F-03): one row per binding (key, function or inline
//! bash/exec command, Global),
//! key capture, live validation, and the "Find Key" requester. See
//! `docs/superpowers/specs/2026-09-29-hotkeys-usermenu-scripts-design.md`.

use crate::client::{Client, Inbound};
use crate::keys::{Action, Focus};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, format_key, parse_key, BindingError, Hotkey, KeyCombo};
use opusdm_ipc::function::{Function, FunctionId};
use opusdm_ui::EmbeddedTool;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

/// What a key combination does, as "Find Key" reports it.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyMeaning {
    /// A user hotkey runs a function.
    UserFunction { name: String, global: bool },
    /// A user hotkey runs an inline bash or exec command.
    UserCommand { command: String, global: bool },
    /// A built-in binding, named by its palette title or a short description.
    Builtin { title: String },
    /// Nothing is bound.
    Unassigned,
}

/// What `combo` does: a user hotkey first (it wins over built-ins), then a
/// built-in binding, else nothing.
pub fn find_key(hotkeys: &[Hotkey], functions: &[Function], combo: &KeyCombo) -> KeyMeaning {
    let identity = combo.identity();
    for hotkey in hotkeys {
        let Ok(bound) = parse_key(&hotkey.key) else { continue };
        if bound.identity() != identity {
            continue;
        }
        let reference = opusdm_ipc::funcref::parse(&hotkey.function);
        if let Some(command) = opusdm_ipc::funcref::describe(&reference) {
            return KeyMeaning::UserCommand { command, global: hotkey.global };
        }
        let name = functions
            .iter()
            .find(|f| f.id == hotkey.function)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "(missing function)".to_string());
        return KeyMeaning::UserFunction { name, global: hotkey.global };
    }
    match builtin_title(combo) {
        Some(title) => KeyMeaning::Builtin { title },
        None => KeyMeaning::Unassigned,
    }
}

/// The built-in binding `combo` triggers in the file list, if any. Super
/// never reaches the built-in table, and Alt only does for the history keys.
/// Shift turns a letter into its uppercase keyval.
fn builtin_title(combo: &KeyCombo) -> Option<String> {
    if combo.mods.sup { return None; }
    let name = combo.normalized_name();
    let shifted_letter = combo.mods.shift && name.len() == 1 && name.chars().all(|c| c.is_ascii_alphabetic());
    let key = if shifted_letter { gdk::Key::from_name(name.to_ascii_uppercase())? } else { gdk::Key::from_name(&name)? };
    let action = crate::keys::resolve(key, combo.mods.ctrl, combo.mods.shift, combo.mods.alt, Focus::List)?;
    Some(action_title(&action))
}

/// A short description of a built-in action.
fn action_title(action: &Action) -> String {
    match action {
        Action::Command(id) => crate::palette::builtin_commands()
            .iter()
            .find(|c| &c.id == id)
            .map(|c| c.title.to_string())
            .unwrap_or_else(|| format!("{id:?}")),
        Action::Enter => "Open the row under the cursor".to_string(),
        Action::EnterNew => "Open the folder under the cursor in a new Lister".to_string(),
        Action::ToggleSelectAndAdvance => "Toggle the selection and move down".to_string(),
        Action::SwapRoleFocus => "Switch to the SOURCE/DEST Lister".to_string(),
        Action::OpenPalette => "Open the command palette".to_string(),
        Action::RevealFilter => "Show the filter bar".to_string(),
        Action::FocusPath => "Focus the path bar".to_string(),
        Action::ToggleTerminal => "Toggle the terminal drawer".to_string(),
        Action::Escape => "Dismiss the filter, terminal or selection".to_string(),
    }
}

/// The set of saved function ids, for validation.
fn known_ids(functions: &[Function]) -> HashSet<FunctionId> {
    functions.iter().map(|f| f.id.clone()).collect()
}

/// Everything the editor's callbacks share.
struct State {
    hotkeys: RefCell<Vec<Hotkey>>,
    functions: Vec<Function>,
    rows_box: gtk::Box,
    /// One box per row, holding that row's error and built-in notes, so they
    /// can be repainted without rebuilding the row.
    notes: RefCell<Vec<gtk::Box>>,
    save_btn: gtk::Button,
    window: gtk::Window,
    /// Runs after every edit and rebuild, once the validation was repainted.
    on_change: Rc<dyn Fn()>,
}

/// Command line flag that makes the Lister open only the Edit Hotkeys window.
pub const EDIT_HOTKEYS_FLAG: &str = "--edit-hotkeys";

/// Start the Edit Hotkeys window as its own process (this binary run with
/// [`EDIT_HOTKEYS_FLAG`]), so it lives on its own instead of over the Lister
/// that asked for it.
pub fn launch_editor() -> std::io::Result<()> {
    crate::function_editor::launch_tool(EDIT_HOTKEYS_FLAG)
}

/// Open the "Edit Hotkeys" window on its own, owned by `app` (which exits
/// when it is closed). It works on the hub's copy of the hotkeys and
/// functions (`ToHub::QueryConfig`), and Save sends only the hotkey section to the hub, which is the
/// only writer of the binding files. A running hub is required. A second
/// launch only raises the window already open.
pub fn open_standalone(app: &gtk::Application) {
    if let Some(window) = app.windows().first() {
        window.present();
        return;
    }
    let shell = opusdm_ui::DialogShell::standalone(app, "Edit Hotkeys");
    let mut client = match Client::connect_tool(&opusdm_ipc::sockpath::hub_sock_path()) {
        Ok(client) => client,
        Err(e) => {
            let window = shell.window().clone();
            shell.present();
            let to_close = window.clone();
            opusdm_ui::requester(&window, "Edit Hotkeys", &format!("cannot reach the OpusDM hub: {e}"), None, &[("OK", ())], move |_| {
                to_close.destroy();
            });
            return;
        }
    };
    let inbox = client.take_inbox();
    let window = shell.window().clone();
    // Start from the hub's copy: it owns the binding files and the Functions.
    gtk::glib::spawn_future_local(async move {
        let (current, functions) = match crate::client::fetch_config(&client, &inbox).await {
            Ok(config) => config,
            Err(e) => {
                shell.present();
                let to_close = window.clone();
                opusdm_ui::requester(&window, "Edit Hotkeys", &format!("cannot read the hotkeys from the hub: {e}"), None, &[("OK", ())], move |_| {
                    to_close.destroy();
                });
                return;
            }
        };
        let hotkeys = current.hotkeys.clone();
        let save_window = window.clone();
        let save: Rc<dyn Fn(Vec<Hotkey>)> = Rc::new(move |hotkeys| {
            opusdm_ipc::debug_log!("lister", "Edit Hotkeys: saving {} hotkeys", hotkeys.len());
            let updated = bindings::Bindings { hotkeys, ..current.clone() };
            match client.send(&opusdm_ipc::ToHub::SetBindings(updated)) {
                Ok(()) => {
                    opusdm_ipc::debug_log!("lister", "Edit Hotkeys: hotkeys saved");
                    save_window.close();
                }
                Err(e) => {
                    opusdm_ui::requester(&save_window, "Edit Hotkeys", &format!("lost the connection to the hub: {e}"), None, &[("OK", ())], |_| {});
                }
            }
        });
        build_editor(shell, hotkeys, functions, save);
        while let Ok(inbound) = inbox.recv().await {
            if let Inbound::HubGone = inbound {
                // The hub quit: nothing could be saved any more.
                window.destroy();
                return;
            }
        }
    });
}

/// The editor body: the rows and the Add button, with what is needed to tell
/// whether they differ from the saved hotkeys.
struct Body {
    state: Rc<State>,
    root: gtk::Box,
    /// The hotkeys as last saved (or loaded).
    saved: RefCell<Vec<Hotkey>>,
}

impl Body {
    /// Whether the rows differ from the saved hotkeys.
    fn is_dirty(&self) -> bool {
        *self.state.hotkeys.borrow() != *self.saved.borrow()
    }

    /// Whether the rows pass the validation that gates Save.
    fn is_valid(&self) -> bool {
        bindings::validate_hotkeys(&self.state.hotkeys.borrow(), &known_ids(&self.state.functions)).is_empty()
    }

    /// Remember the rows as the saved hotkeys.
    fn mark_saved(&self) {
        *self.saved.borrow_mut() = self.state.hotkeys.borrow().clone();
    }

    /// Put the saved hotkeys back in the rows.
    fn revert(&self) {
        *self.state.hotkeys.borrow_mut() = self.saved.borrow().clone();
        rebuild(&self.state);
    }
}

/// Build the editor body over `hotkeys`. `window` owns the dialogs the rows
/// open; `save_btn` is enabled only while nothing is wrong.
fn build_body(window: &gtk::Window, hotkeys: Vec<Hotkey>, functions: Vec<Function>, save_btn: gtk::Button, on_change: Rc<dyn Fn()>) -> Body {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let rows_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.append(&rows_box);
    let add_btn = gtk::Button::with_label("Add Hotkey");
    add_btn.set_halign(gtk::Align::Start);
    root.append(&add_btn);

    let saved = hotkeys.clone();
    let state = Rc::new(State {
        hotkeys: RefCell::new(hotkeys),
        functions,
        rows_box,
        notes: RefCell::new(Vec::new()),
        save_btn,
        window: window.clone(),
        on_change,
    });
    {
        let state = state.clone();
        add_btn.connect_clicked(move |_| {
            state.hotkeys.borrow_mut().push(Hotkey::default());
            rebuild(&state);
        });
    }
    rebuild(&state);
    Body { state, root, saved: RefCell::new(saved) }
}

/// Fill `shell` with the hotkey editor. `save` receives the hotkeys when
/// Save is pressed; the caller decides what happens to the window.
fn build_editor(shell: opusdm_ui::DialogShell, hotkeys: Vec<Hotkey>, functions: Vec<Function>, save: Rc<dyn Fn(Vec<Hotkey>)>) {
    let shell = shell.size(640, 420).scrolled();
    let save_btn = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Close");
    let body = build_body(shell.window(), hotkeys, functions, save_btn.clone(), Rc::new(|| {}));
    shell.content().append(&body.root);
    let state = body.state.clone();
    save_btn.connect_clicked(move |_| save(state.hotkeys.borrow().clone()));
    shell.present();
}

/// Repaint every row from the state and re-run the validation that gates
/// Save.
fn rebuild(state: &Rc<State>) {
    while let Some(child) = state.rows_box.first_child() {
        state.rows_box.remove(&child);
    }
    state.notes.borrow_mut().clear();
    let hotkeys = state.hotkeys.borrow().clone();
    for (index, hotkey) in hotkeys.iter().enumerate() {
        state.rows_box.append(&build_row(state, index, hotkey));
    }
    revalidate(state);
}

/// Re-run the validation and repaint every row's notes in place, without
/// touching the widgets that have the keyboard focus. Save is enabled only
/// when nothing is wrong.
fn revalidate(state: &Rc<State>) {
    let hotkeys = state.hotkeys.borrow().clone();
    let errors = bindings::validate_hotkeys(&hotkeys, &known_ids(&state.functions));
    for (index, notes) in state.notes.borrow().iter().enumerate() {
        let Some(hotkey) = hotkeys.get(index) else { continue };
        fill_notes(notes, index, hotkey, &errors);
    }
    state.save_btn.set_sensitive(errors.is_empty());
    (state.on_change)();
}

/// Replace the content of a row's `notes` box with that row's errors and its
/// built-in override note.
fn fill_notes(notes: &gtk::Box, index: usize, hotkey: &Hotkey, errors: &[BindingError]) {
    while let Some(child) = notes.first_child() {
        notes.remove(&child);
    }
    for error in errors.iter().filter(|e| e.hotkey_index() == Some(index)) {
        notes.append(&note_label(&error.to_string(), true));
    }
    if let Ok(combo) = parse_key(&hotkey.key) {
        if let KeyMeaning::Builtin { title } = find_key(&[], &[], &combo) {
            notes.append(&note_label(&format!("Overrides the built-in binding: {title}"), false));
        }
    }
}

/// One editable row: key button, function picker (saved function, bash
/// command or program), Global check box, remove button, then this row's
/// notes underneath.
fn build_row(state: &Rc<State>, index: usize, hotkey: &Hotkey) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let key_text = if hotkey.key.is_empty() { "Press a key...".to_string() } else { hotkey.key.clone() };
    let key_btn = gtk::Button::with_label(&key_text);
    key_btn.set_size_request(190, -1);
    {
        let state = state.clone();
        key_btn.connect_clicked(move |_| capture_key(&state, index));
    }
    line.append(&key_btn);

    let picker = {
        let on_change_state = state.clone();
        opusdm_ui::RefPicker::new(&state.functions, &hotkey.function, &state.window, None, move |reference| {
            if let Some(h) = on_change_state.hotkeys.borrow_mut().get_mut(index) {
                if h.function == reference {
                    return;
                }
                h.function = reference;
            }
            revalidate(&on_change_state);
        })
    };
    picker.widget().set_hexpand(true);
    line.append(picker.widget());

    let global = gtk::CheckButton::with_label("Global");
    global.set_active(hotkey.global);
    {
        let state = state.clone();
        global.connect_toggled(move |c| {
            if let Some(h) = state.hotkeys.borrow_mut().get_mut(index) {
                h.global = c.is_active();
            }
            revalidate(&state);
        });
    }
    line.append(&global);

    let remove = gtk::Button::from_icon_name("list-remove-symbolic");
    remove.add_css_class("flat");
    {
        let state = state.clone();
        remove.connect_clicked(move |_| {
            let mut hotkeys = state.hotkeys.borrow_mut();
            if index < hotkeys.len() {
                hotkeys.remove(index);
            }
            drop(hotkeys);
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

/// A small left-aligned label under a row; `is_error` adds the error style.
fn note_label(text: &str, is_error: bool) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_margin_start(6);
    if is_error {
        label.add_css_class("error");
    } else {
        label.add_css_class("dim-label");
    }
    label
}

/// Ask for a key combination in a small modal window and store its canonical
/// text in hotkey `index`. Escape cancels (so Escape itself cannot be bound).
fn capture_key(state: &Rc<State>, index: usize) {
    let shell = opusdm_ui::DialogShell::new(&state.window, "Press a key combination").size(340, 120).resizable(false);
    shell.content().append(&gtk::Label::new(Some("Press the key combination now.\nEscape cancels.")));
    shell.close_button("Cancel");
    let window = shell.window().clone();

    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let state = state.clone();
        let window = window.clone();
        controller.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Escape {
                window.close();
                return glib::Propagation::Stop;
            }
            let Some(combo) = crate::hotkeys::combo_from_event(key, mods) else {
                return glib::Propagation::Stop;
            };
            if let Some(h) = state.hotkeys.borrow_mut().get_mut(index) {
                h.key = format_key(&combo);
            }
            window.close();
            rebuild(&state);
            glib::Propagation::Stop
        });
    }
    window.add_controller(controller);
    shell.present();
}

/// The connection of the embedded editor to the hub. Nothing reads the inbox
/// between a load and a save, so a hub that quit shows up as a failed save.
struct Link {
    client: Client,
    inbox: async_channel::Receiver<Inbound>,
    /// A save is waiting for the hub's answer.
    saving: Cell<bool>,
}

/// The Hotkeys editor inside another window (Advanced tab of Settings): the
/// rows, a status line and its own Save and Revert buttons. It works on the
/// hub's copy, like the standalone window.
pub struct HotkeysTool {
    root: gtk::Box,
    window: gtk::Window,
    body: Rc<RefCell<Option<Body>>>,
    link: RefCell<Option<Rc<Link>>>,
    status: gtk::Label,
    save_btn: gtk::Button,
    revert_btn: gtk::Button,
}

/// Open the Hotkeys editor for the host `window`. The rows appear once the hub
/// answered; if it cannot be reached the page says so.
pub fn embedded(window: &gtk::Window) -> Rc<HotkeysTool> {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);
    let slot = gtk::ScrolledWindow::builder().vexpand(true).hexpand(true).build();
    slot.set_child(Some(&gtk::Label::new(Some("Loading the hotkeys..."))));
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.add_css_class("error");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let revert_btn = gtk::Button::with_label("Revert");
    let save_btn = gtk::Button::with_label("Save");
    save_btn.set_sensitive(false);
    revert_btn.set_sensitive(false);
    buttons.append(&revert_btn);
    buttons.append(&save_btn);
    root.append(&slot);
    root.append(&status);
    root.append(&buttons);

    let tool = Rc::new(HotkeysTool { root, window: window.clone(), body: Rc::new(RefCell::new(None)), link: RefCell::new(None), status, save_btn: save_btn.clone(), revert_btn: revert_btn.clone() });
    let weak = Rc::downgrade(&tool);
    save_btn.connect_clicked({
        let weak = weak.clone();
        move |_| match weak.upgrade() {
            Some(tool) => tool.save(Box::new(|_| {})),
            None => opusdm_ipc::err_log!("opusdm-lister", "Hotkeys: the editor is gone, Save ignored"),
        }
    });
    revert_btn.connect_clicked({
        let weak = weak.clone();
        move |_| match weak.upgrade() {
            Some(tool) => tool.revert(Box::new(|| {})),
            None => opusdm_ipc::err_log!("opusdm-lister", "Hotkeys: the editor is gone, Revert ignored"),
        }
    });
    tool.load(slot, weak);
    tool
}

impl HotkeysTool {
    /// Connect to the hub, fetch its copy and put the rows in `slot`.
    fn load(&self, slot: gtk::ScrolledWindow, weak: std::rc::Weak<HotkeysTool>) {
        let show_error = move |slot: &gtk::ScrolledWindow, text: String| {
            opusdm_ipc::err_log!("opusdm-lister", "Hotkeys: {text}");
            slot.set_child(Some(&gtk::Label::new(Some(&text))));
        };
        let mut client = match Client::connect_tool(&opusdm_ipc::sockpath::hub_sock_path()) {
            Ok(client) => client,
            Err(e) => return show_error(&slot, format!("cannot reach the OpusDM hub: {e}")),
        };
        let inbox = client.take_inbox();
        let link = Rc::new(Link { client, inbox, saving: Cell::new(false) });
        *self.link.borrow_mut() = Some(link.clone());
        gtk::glib::spawn_future_local(async move {
            let result = crate::client::fetch_config(&link.client, &link.inbox).await;
            let Some(tool) = weak.upgrade() else {
                opusdm_ipc::err_log!("opusdm-lister", "Hotkeys: the editor was closed before the hub answered");
                return;
            };
            match result {
                Ok((current, functions)) => {
                    // Weak: the body owns the state that owns this closure.
                    let (body_slot, revert_btn) = (Rc::downgrade(&tool.body), tool.revert_btn.clone());
                    let on_change: Rc<dyn Fn()> = Rc::new(move || {
                        let dirty = body_slot.upgrade().is_some_and(|b| b.borrow().as_ref().is_some_and(Body::is_dirty));
                        revert_btn.set_sensitive(dirty);
                    });
                    let body = build_body(&tool.window, current.hotkeys, functions, tool.save_btn.clone(), on_change);
                    slot.set_child(Some(&body.root));
                    *tool.body.borrow_mut() = Some(body);
                    tool.refresh_buttons();
                }
                Err(e) => show_error(&slot, format!("cannot read the hotkeys from the hub: {e}")),
            }
        });
    }

    /// Revert is available while there is something to revert.
    fn refresh_buttons(&self) {
        let dirty = self.body.borrow().as_ref().is_some_and(Body::is_dirty);
        self.revert_btn.set_sensitive(dirty);
    }
}

impl EmbeddedTool for HotkeysTool {
    fn title(&self) -> &str {
        "Hotkeys"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    fn is_dirty(&self) -> bool {
        self.body.borrow().as_ref().is_some_and(Body::is_dirty)
    }

    /// Ask the hub for its copy again (the Menu and Scripts sections may have
    /// changed since this editor started), then send it with the new hotkeys.
    fn save(&self, done: Box<dyn FnOnce(bool)>) {
        let (Some(link), true) = (self.link.borrow().clone(), self.body.borrow().is_some()) else {
            self.status.set_text("The hotkeys are not loaded: nothing to save.");
            return done(false);
        };
        if !self.body.borrow().as_ref().is_some_and(Body::is_valid) {
            self.status.set_text("Fix the marked hotkeys before saving.");
            return done(false);
        }
        if link.saving.replace(true) {
            return done(false);
        }
        let hotkeys = self.body.borrow().as_ref().map(|b| b.state.hotkeys.borrow().clone()).unwrap_or_default();
        let (status, revert_btn, body) = (self.status.clone(), self.revert_btn.clone(), self.body.clone());
        gtk::glib::spawn_future_local(async move {
            let result = async {
                let (current, _) = crate::client::fetch_config(&link.client, &link.inbox).await?;
                let updated = bindings::Bindings { hotkeys, ..current };
                link.client.send(&opusdm_ipc::ToHub::SetBindings(updated)).map_err(|e| format!("lost the connection to the hub: {e}"))
            }
            .await;
            link.saving.set(false);
            match result {
                Ok(()) => {
                    if let Some(body) = body.borrow().as_ref() { body.mark_saved(); }
                    revert_btn.set_sensitive(false);
                    status.set_text("");
                    done(true);
                }
                Err(e) => {
                    opusdm_ipc::err_log!("opusdm-lister", "Hotkeys: cannot save: {e}");
                    status.set_text(&format!("Cannot save: {e}"));
                    done(false);
                }
            }
        });
    }

    fn revert(&self, done: Box<dyn FnOnce()>) {
        if let Some(body) = self.body.borrow().as_ref() { body.revert(); }
        self.status.set_text("");
        self.refresh_buttons();
        done();
    }
}

/// Open the "Find Key" requester: press a combination, read what it does.
pub fn open_find_key(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Find Key").size(400, 160);
    let prompt = gtk::Label::new(Some("Press a key combination"));
    let result = gtk::Label::new(None);
    result.set_wrap(true);
    shell.content().append(&prompt);
    shell.content().append(&result);
    shell.close_button("Close");
    let window = shell.window().clone();

    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let ui = ui.clone();
        controller.connect_key_pressed(move |_, key, _, mods| {
            let Some(combo) = crate::hotkeys::combo_from_event(key, mods) else {
                return glib::Propagation::Stop;
            };
            prompt.set_text(&format_key(&combo));
            let meaning = find_key(&ui.hub.bindings.borrow().hotkeys, &ui.hub.functions.borrow(), &combo);
            result.set_text(&match meaning {
                KeyMeaning::UserFunction { name, global } => {
                    format!("Runs the function \"{name}\"{}", if global { " (global hotkey)" } else { "" })
                }
                KeyMeaning::UserCommand { command, global } => {
                    format!("Runs the command \"{command}\"{}", if global { " (global hotkey)" } else { "" })
                }
                KeyMeaning::Builtin { title } => format!("Built-in: {title}"),
                KeyMeaning::Unassigned => "Not assigned".to_string(),
            });
            glib::Propagation::Stop
        });
    }
    window.add_controller(controller);
    shell.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::bindings::{parse_key, Hotkey};
    use opusdm_ipc::Function;

    fn combo(text: &str) -> KeyCombo {
        parse_key(text).unwrap()
    }

    fn function(id: &str, name: &str) -> Function {
        let mut f = Function::new(name);
        f.id = id.to_string();
        f
    }

    #[test]
    fn a_user_hotkey_is_reported_with_its_function_name_and_global_flag() {
        let hotkeys = [Hotkey { key: "Ctrl+Alt+u".into(), function: "f1".into(), global: true }];
        let functions = [function("f1", "Copy and notify")];
        assert_eq!(
            find_key(&hotkeys, &functions, &combo("ctrl+alt+U")),
            KeyMeaning::UserFunction { name: "Copy and notify".into(), global: true }
        );
    }

    #[test]
    fn an_inline_hotkey_is_reported_as_a_command_not_a_function() {
        let hotkeys = [Hotkey { key: "Ctrl+Alt+t".into(), function: "exec:kitty --class x".into(), global: true }];
        assert_eq!(
            find_key(&hotkeys, &[], &combo("Ctrl+Alt+t")),
            KeyMeaning::UserCommand { command: "exec: kitty --class x".into(), global: true }
        );
        let hotkeys = [Hotkey { key: "F9".into(), function: "bash+out:ls -l".into(), global: false }];
        assert_eq!(
            find_key(&hotkeys, &[], &combo("F9")),
            KeyMeaning::UserCommand { command: "bash: ls -l".into(), global: false }
        );
    }

    #[test]
    fn a_dangling_hotkey_is_reported_as_a_missing_function() {
        let hotkeys = [Hotkey { key: "Ctrl+Alt+u".into(), function: "gone".into(), global: false }];
        assert_eq!(
            find_key(&hotkeys, &[], &combo("Ctrl+Alt+u")),
            KeyMeaning::UserFunction { name: "(missing function)".into(), global: false }
        );
    }

    #[test]
    fn a_built_in_key_is_reported_with_the_palette_title() {
        match find_key(&[], &[], &combo("Ctrl+n")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("Make Directory"), "{title}"),
            other => panic!("{other:?}"),
        }
        match find_key(&[], &[], &combo("Ctrl+c")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("Clipboard"), "{title}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn shift_turns_a_letter_into_its_uppercase_binding() {
        match find_key(&[], &[], &combo("Ctrl+a")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("Select All"), "{title}"),
            other => panic!("{other:?}"),
        }
        match find_key(&[], &[], &combo("Ctrl+Shift+a")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("Reselect"), "{title}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn alt_left_is_reported_as_the_history_built_in() {
        match find_key(&[], &[], &combo("Alt+Left")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("History Back"), "{title}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_user_hotkey_on_alt_right_wins_over_the_history_built_in() {
        let hotkeys = [Hotkey { key: "Alt+Right".into(), function: "f1".into(), global: false }];
        let functions = [function("f1", "Mine")];
        assert_eq!(
            find_key(&hotkeys, &functions, &combo("Alt+Right")),
            KeyMeaning::UserFunction { name: "Mine".into(), global: false }
        );
    }

    #[test]
    fn an_unbound_key_is_unassigned() {
        assert_eq!(find_key(&[], &[], &combo("Ctrl+Alt+u")), KeyMeaning::Unassigned);
        assert_eq!(find_key(&[], &[], &combo("Alt+c")), KeyMeaning::Unassigned, "Alt never reaches a built-in");
    }

    #[test]
    fn a_user_hotkey_wins_over_a_built_in_in_find_key() {
        let hotkeys = [Hotkey { key: "F7".into(), function: "f1".into(), global: false }];
        let functions = [function("f1", "Mine")];
        assert_eq!(
            find_key(&hotkeys, &functions, &combo("F7")),
            KeyMeaning::UserFunction { name: "Mine".into(), global: false }
        );
    }
}
