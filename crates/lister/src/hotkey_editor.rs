//! The hotkey editor (F-03): one row per binding (key, function, Global),
//! key capture, live validation, and the "Find Key" requester. See
//! `docs/superpowers/specs/2026-09-29-hotkeys-usermenu-scripts-design.md`.

use crate::keys::{Action, Focus, Motion};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, format_key, parse_key, BindingError, Hotkey, KeyCombo};
use opusdm_ipc::function::{self, Function, FunctionId};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// What a key combination does, as "Find Key" reports it.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyMeaning {
    /// A user hotkey runs a function.
    UserFunction { name: String, global: bool },
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

/// The built-in binding `combo` triggers in the file list, if any. Alt and
/// Super never reach the built-in table. Shift turns a letter into its
/// uppercase keyval (`g` moves to the first row, `G` to the last).
fn builtin_title(combo: &KeyCombo) -> Option<String> {
    if combo.mods.alt || combo.mods.sup {
        return None;
    }
    let name = combo.normalized_name();
    let shifted_letter = combo.mods.shift && name.len() == 1 && name.chars().all(|c| c.is_ascii_alphabetic());
    let key = if shifted_letter { gdk::Key::from_name(name.to_ascii_uppercase())? } else { gdk::Key::from_name(&name)? };
    let action = crate::keys::resolve(key, combo.mods.ctrl, combo.mods.shift, Focus::List)?;
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
        Action::Move(Motion::Down) => "Move the cursor down".to_string(),
        Action::Move(Motion::Up) => "Move the cursor up".to_string(),
        Action::Move(Motion::First) => "Move the cursor to the first row".to_string(),
        Action::Move(Motion::Last) => "Move the cursor to the last row".to_string(),
        Action::Enter => "Open the row under the cursor".to_string(),
        Action::ToggleSelectAndAdvance => "Toggle the selection and move down".to_string(),
        Action::SwapRoleFocus => "Switch to the SOURCE/DEST Lister".to_string(),
        Action::OpenPalette => "Open the command palette".to_string(),
        Action::RevealFilter => "Show the filter bar".to_string(),
        Action::ToggleTerminal => "Toggle the terminal drawer".to_string(),
        Action::PreviewOrSize => "Quick Look, or directory size".to_string(),
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
    save_btn: gtk::Button,
    window: gtk::Window,
}

/// Open the "Edit Hotkeys" window, working on a copy of the Lister's cached
/// hotkeys. Save sends only the hotkey section back to the hub.
pub fn open_hotkey_editor(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Edit Hotkeys").size(640, 420).scrolled();
    let root = shell.content();

    let rows_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.append(&rows_box);
    let add_btn = gtk::Button::with_label("Add Hotkey");
    add_btn.set_halign(gtk::Align::Start);
    root.append(&add_btn);

    let save_btn = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Close");

    let state = Rc::new(State {
        hotkeys: RefCell::new(ui.hub.bindings.borrow().hotkeys.clone()),
        functions: function::load_functions(),
        rows_box,
        save_btn: save_btn.clone(),
        window: shell.window().clone(),
    });

    {
        let state = state.clone();
        add_btn.connect_clicked(move |_| {
            state.hotkeys.borrow_mut().push(Hotkey::default());
            rebuild(&state);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        save_btn.connect_clicked(move |_| {
            let mut updated = ui.hub.bindings.borrow().clone();
            updated.hotkeys = state.hotkeys.borrow().clone();
            let _ = ui.hub.client.send(&opusdm_ipc::ToHub::SetBindings(updated));
            crate::actions::flash(&ui, "hotkeys saved");
        });
    }

    rebuild(&state);
    shell.present();
}

/// Repaint every row from the state and re-run the validation that gates
/// Save.
fn rebuild(state: &Rc<State>) {
    while let Some(child) = state.rows_box.first_child() {
        state.rows_box.remove(&child);
    }
    let hotkeys = state.hotkeys.borrow().clone();
    let errors = bindings::validate_hotkeys(&hotkeys, &known_ids(&state.functions));
    for (index, hotkey) in hotkeys.iter().enumerate() {
        state.rows_box.append(&build_row(state, index, hotkey, &errors));
    }
    state.save_btn.set_sensitive(errors.is_empty());
}

/// One editable row: key button, function drop-down, Global check box,
/// remove button, then this row's errors and built-in note underneath.
fn build_row(state: &Rc<State>, index: usize, hotkey: &Hotkey, errors: &[BindingError]) -> gtk::Box {
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

    let dropdown = crate::binding_ui::function_dropdown(&state.functions, &hotkey.function);
    dropdown.set_hexpand(true);
    {
        let state = state.clone();
        dropdown.connect_selected_notify(move |d| {
            let Some(id) = crate::binding_ui::selected_function(d, &state.functions) else { return };
            if let Some(h) = state.hotkeys.borrow_mut().get_mut(index) {
                if h.function == id {
                    return;
                }
                h.function = id;
            }
            rebuild(&state);
        });
    }
    line.append(&dropdown);

    let global = gtk::CheckButton::with_label("Global");
    global.set_active(hotkey.global);
    {
        let state = state.clone();
        global.connect_toggled(move |c| {
            if let Some(h) = state.hotkeys.borrow_mut().get_mut(index) {
                h.global = c.is_active();
            }
            rebuild(&state);
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

    for error in errors.iter().filter(|e| e.hotkey_index() == Some(index)) {
        row.append(&note_label(&error.to_string(), true));
    }
    if let Ok(combo) = parse_key(&hotkey.key) {
        if let KeyMeaning::Builtin { title } = find_key(&[], &[], &combo) {
            row.append(&note_label(&format!("Overrides the built-in binding: {title}"), false));
        }
    }
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
            let meaning = find_key(&ui.hub.bindings.borrow().hotkeys, &function::load_functions(), &combo);
            result.set_text(&match meaning {
                KeyMeaning::UserFunction { name, global } => {
                    format!("Runs the function \"{name}\"{}", if global { " (global hotkey)" } else { "" })
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
    fn a_dangling_hotkey_is_reported_as_a_missing_function() {
        let hotkeys = [Hotkey { key: "Ctrl+Alt+u".into(), function: "gone".into(), global: false }];
        assert_eq!(
            find_key(&hotkeys, &[], &combo("Ctrl+Alt+u")),
            KeyMeaning::UserFunction { name: "(missing function)".into(), global: false }
        );
    }

    #[test]
    fn a_built_in_key_is_reported_with_the_palette_title() {
        match find_key(&[], &[], &combo("F7")) {
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
        match find_key(&[], &[], &combo("g")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("first"), "{title}"),
            other => panic!("{other:?}"),
        }
        match find_key(&[], &[], &combo("Shift+g")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("last"), "{title}"),
            other => panic!("{other:?}"),
        }
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
