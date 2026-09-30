//! The bank manager (list of banks: New, Edit, Delete, Open, Close) and the
//! editor of one bank (preview, button toolbar, button form, appearance).
//! The button and validation rules live in `opusdm_ipc::buttonbank`.

use crate::grid;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::function::{load_functions, Function};
use opusdm_ipc::{Banks, Btn, ButtonBank, MouseButton, Orientation};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

/// What changed in the store, for the owner of the windows.
pub enum BankChange {
    /// Bank `id` was created or saved.
    Saved(String),
    /// Bank `id` was deleted.
    Deleted(String),
}

/// What the manager and editor ask of their owner.
#[derive(Clone)]
pub struct Hooks {
    pub changed: Rc<dyn Fn(BankChange)>,
    /// Open (show) the window of bank `id`.
    pub open: Rc<dyn Fn(&str)>,
    /// Close the window of bank `id`.
    pub close: Rc<dyn Fn(&str)>,
}

/// Text of the "no Function" entry of the Function drop-downs.
const NO_FUNCTION: &str = "(none)";

/// Show a modal error message over `parent`.
fn show_error(parent: &impl IsA<gtk::Window>, message: &str) {
    opusdm_ui::requester(parent, "Error", message, None, &[("OK", ())], |_| {});
}

/// Persist bank `id`, returning the OS error as text.
fn save(banks: &Banks, id: &str) -> Result<(), String> {
    banks.save_bank(id).map_err(|e| format!("Cannot save the bank: {e}"))
}

/// Replace bank `id` with `edited` in the store and write its file to `dir`.
/// The store keeps the old bank when the new one is invalid or the file
/// cannot be written, so memory and disk never disagree. The store is never
/// borrowed mutably while the file is written.
fn save_edit(banks: &RefCell<Banks>, id: &str, edited: ButtonBank, dir: &Path) -> Result<(), String> {
    let previous = banks.borrow().get(id).cloned();
    let replaced = banks.borrow_mut().replace(id, edited);
    replaced?;
    let written = banks.borrow().save_bank_to(dir, id);
    if let Err(e) = written {
        if let Some(previous) = previous {
            if let Some(slot) = banks.borrow_mut().banks.iter_mut().find(|b| b.id == id) {
                *slot = previous;
            }
        }
        return Err(format!("Cannot save the bank: {e}"));
    }
    Ok(())
}

/// Open the manager window.
pub fn open_manager(parent: &impl IsA<gtk::Window>, banks: Rc<RefCell<Banks>>, hooks: Hooks) {
    let shell = opusdm_ui::DialogShell::new(parent, "Button Banks").size(380, 340);
    let list_box = gtk::ListBox::new();
    let scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).build();
    shell.content().append(&scroller);

    let refresh: Rc<dyn Fn()> = {
        let list_box = list_box.clone();
        let banks = banks.clone();
        Rc::new(move || {
            while let Some(row) = list_box.first_child() {
                list_box.remove(&row);
            }
            for bank in &banks.borrow().banks {
                let label = gtk::Label::new(Some(&bank.name));
                label.set_xalign(0.0);
                label.set_margin_start(6);
                label.set_margin_end(6);
                label.set_margin_top(4);
                label.set_margin_bottom(4);
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&label));
                list_box.append(&row);
            }
        })
    };
    refresh();

    let new_btn = shell.button("New...");
    let edit_btn = shell.button("Edit...");
    let delete_btn = shell.button("Delete");
    let open_btn = shell.button("Open");
    let close_btn = shell.button("Close Bank");
    shell.button_spacer();
    shell.close_button("Close");
    let window = shell.window().clone();

    let selected_id = {
        let list_box = list_box.clone();
        let banks = banks.clone();
        move || -> Option<String> {
            let index = usize::try_from(list_box.selected_row()?.index()).ok()?;
            banks.borrow().banks.get(index).map(|b| b.id.clone())
        }
    };

    {
        let (banks, hooks, refresh, window) = (banks.clone(), hooks.clone(), refresh.clone(), window.clone());
        new_btn.connect_clicked(move |_| {
            let (banks, hooks, refresh, window_for_cb) = (banks.clone(), hooks.clone(), refresh.clone(), window.clone());
            opusdm_ui::requester_entry(&window, "New bank", "", &[("Create", true), ("Cancel", false)], move |ok, name| {
                if !ok {
                    return;
                }
                let created = banks.borrow_mut().create_in_dir(&opusdm_ipc::buttonbank::banks_dir(), &name);
                match created.and_then(|id| save(&banks.borrow(), &id).map(|()| id)) {
                    Ok(id) => {
                        refresh();
                        (hooks.changed)(BankChange::Saved(id.clone()));
                        open_editor(&window_for_cb, banks.clone(), &id, hooks.clone(), refresh.clone());
                    }
                    Err(message) => show_error(&window_for_cb, &message),
                }
            });
        });
    }
    {
        let (banks, hooks, refresh, window, selected_id) =
            (banks.clone(), hooks.clone(), refresh.clone(), window.clone(), selected_id.clone());
        edit_btn.connect_clicked(move |_| {
            let Some(id) = selected_id() else { return };
            open_editor(&window, banks.clone(), &id, hooks.clone(), refresh.clone());
        });
    }
    {
        let (banks, hooks, refresh, window, selected_id) =
            (banks.clone(), hooks.clone(), refresh.clone(), window.clone(), selected_id.clone());
        delete_btn.connect_clicked(move |_| {
            let Some(id) = selected_id() else { return };
            let name = banks.borrow().get(&id).map(|b| b.name.clone()).unwrap_or_default();
            let (banks, hooks, refresh, window_for_cb) = (banks.clone(), hooks.clone(), refresh.clone(), window.clone());
            opusdm_ui::requester(
                &window,
                "Request",
                &format!("Delete the bank \"{name}\"?"),
                None,
                &[("Delete", true), ("Cancel", false)],
                move |delete| {
                    if !delete {
                        return;
                    }
                    banks.borrow_mut().delete(&id);
                    if let Err(e) = Banks::remove_bank_file(&id) {
                        show_error(&window_for_cb, &format!("Cannot delete the bank file: {e}"));
                    }
                    refresh();
                    (hooks.changed)(BankChange::Deleted(id.clone()));
                },
            );
        });
    }
    {
        let (hooks, selected_id) = (hooks.clone(), selected_id.clone());
        open_btn.connect_clicked(move |_| {
            if let Some(id) = selected_id() {
                (hooks.open)(&id);
            }
        });
    }
    {
        let (hooks, selected_id) = (hooks.clone(), selected_id.clone());
        close_btn.connect_clicked(move |_| {
            if let Some(id) = selected_id() {
                (hooks.close)(&id);
            }
        });
    }
    shell.present();
}

/// The widgets of the button form.
struct Form {
    label: gtk::Entry,
    icon: gtk::Entry,
    /// Left, right and middle Function drop-downs.
    functions: [gtk::DropDown; 3],
    name: gtk::Entry,
    orientation: gtk::DropDown,
    borderless: gtk::CheckButton,
    columns: gtk::SpinButton,
}

/// Everything the editor's callbacks share.
struct State {
    window: gtk::Window,
    banks: Rc<RefCell<Banks>>,
    id: String,
    /// The working copy being edited.
    bank: RefCell<ButtonBank>,
    selected: Cell<Option<usize>>,
    clipboard: RefCell<Option<Btn>>,
    functions: Vec<Function>,
    preview: gtk::Box,
    form: Form,
    error: gtk::Label,
    save_btn: gtk::Button,
    /// True while the form is being filled from the model, so the change
    /// handlers do not write the values straight back.
    loading: Cell<bool>,
}

/// Open the editor of bank `id`. `after_save` runs after every successful
/// save (the manager refreshes its list with it).
pub fn open_editor(parent: &impl IsA<gtk::Window>, banks: Rc<RefCell<Banks>>, id: &str, hooks: Hooks, after_save: Rc<dyn Fn()>) {
    let Some(bank) = banks.borrow().get(id).cloned() else { return };
    let functions = load_functions();
    let shell = opusdm_ui::DialogShell::new(parent, &format!("Edit bank: {}", bank.name)).size(640, 460);

    let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let names: Vec<&str> = std::iter::once(NO_FUNCTION).chain(functions.iter().map(|f| f.name.as_str())).collect();
    let function_dropdown = || gtk::DropDown::from_strings(&names);
    let form = Form {
        label: gtk::Entry::new(),
        icon: gtk::Entry::new(),
        functions: [function_dropdown(), function_dropdown(), function_dropdown()],
        name: gtk::Entry::new(),
        orientation: gtk::DropDown::from_strings(&["Horizontal", "Vertical"]),
        borderless: gtk::CheckButton::with_label("Borderless"),
        columns: gtk::SpinButton::with_range(f64::from(opusdm_ipc::buttonbank::MIN_COLUMNS), f64::from(opusdm_ipc::buttonbank::MAX_COLUMNS), 1.0),
    };
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.add_css_class("error");

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let tool = |label: &str| {
        let b = gtk::Button::with_label(label);
        toolbar.append(&b);
        b
    };
    let (add_btn, insert_btn, delete_btn, copy_btn, paste_btn, up_btn, down_btn) =
        (tool("Add"), tool("Insert"), tool("Delete"), tool("Copy"), tool("Paste"), tool("Up"), tool("Down"));

    let form_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let row = |caption: &str, widget: &gtk::Widget| {
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let l = gtk::Label::new(Some(caption));
        l.set_width_chars(10);
        l.set_xalign(0.0);
        r.append(&l);
        widget.set_hexpand(true);
        r.append(widget);
        form_box.append(&r);
    };
    row("Bank name", form.name.upcast_ref());
    row("Orientation", form.orientation.upcast_ref());
    row("Columns", form.columns.upcast_ref());
    form_box.append(&form.borderless);
    form_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    row("Label", form.label.upcast_ref());
    row("Icon file", form.icon.upcast_ref());
    row("Left click", form.functions[0].upcast_ref());
    row("Right click", form.functions[1].upcast_ref());
    row("Middle click", form.functions[2].upcast_ref());

    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    left.append(&toolbar);
    left.append(&gtk::ScrolledWindow::builder().child(&preview).vexpand(true).hexpand(true).build());
    let split = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    split.append(&left);
    split.append(&form_box);
    shell.content().append(&split);
    shell.content().append(&error);

    let save_btn = shell.button("Save");
    shell.close_button("Close");

    let state = Rc::new(State {
        window: shell.window().clone(),
        banks,
        id: id.to_string(),
        bank: RefCell::new(bank),
        selected: Cell::new(None),
        clipboard: RefCell::new(None),
        functions,
        preview,
        form,
        error,
        save_btn: save_btn.clone(),
        loading: Cell::new(false),
    });

    wire_toolbar(&state, [&add_btn, &insert_btn, &delete_btn, &copy_btn, &paste_btn, &up_btn, &down_btn]);
    wire_form(&state);
    {
        let (state, hooks, after_save) = (state.clone(), hooks, after_save);
        save_btn.connect_clicked(move |_| {
            let edited = state.bank.borrow().clone();
            let result = save_edit(&state.banks, &state.id, edited, &opusdm_ipc::buttonbank::banks_dir());
            match result {
                Ok(()) => {
                    (hooks.changed)(BankChange::Saved(state.id.clone()));
                    after_save();
                }
                Err(message) => show_error(&state.window, &message),
            }
        });
    }
    refresh_all(&state);
    shell.present();
}

/// Rebuild the preview, fill the form from the selected button and
/// re-validate.
fn refresh_all(state: &Rc<State>) {
    while let Some(child) = state.preview.first_child() {
        state.preview.remove(&child);
    }
    let on_click = {
        let state = state.clone();
        Rc::new(move |index: usize, _mouse: MouseButton| {
            state.selected.set(Some(index));
            load_form(&state);
        })
    };
    let ids: Vec<String> = state.functions.iter().map(|f| f.id.clone()).collect();
    let grid = grid::build_grid(&state.bank.borrow(), on_click, &|id| ids.iter().any(|i| i == id));
    state.preview.append(&grid);
    load_form(state);
}

/// Copy the bank settings and the selected button into the form widgets.
fn load_form(state: &Rc<State>) {
    state.loading.set(true);
    let bank = state.bank.borrow();
    let form = &state.form;
    form.name.set_text(&bank.name);
    form.orientation.set_selected(u32::from(bank.orientation == Orientation::Vertical));
    form.columns.set_value(f64::from(bank.columns));
    form.borderless.set_active(bank.borderless);
    let btn = state.selected.get().and_then(|i| bank.buttons.get(i));
    form.label.set_text(btn.map(|b| b.label.as_str()).unwrap_or(""));
    form.icon.set_text(&btn.and_then(|b| b.icon.as_ref()).map(|p| p.display().to_string()).unwrap_or_default());
    let bound = [btn.and_then(|b| b.left.as_ref()), btn.and_then(|b| b.right.as_ref()), btn.and_then(|b| b.middle.as_ref())];
    for (dropdown, id) in form.functions.iter().zip(bound) {
        let position = id.and_then(|id| state.functions.iter().position(|f| &f.id == id)).map_or(0, |p| p + 1);
        dropdown.set_selected(position as u32);
        dropdown.set_sensitive(btn.is_some());
    }
    form.label.set_sensitive(btn.is_some());
    form.icon.set_sensitive(btn.is_some());
    drop(bank);
    state.loading.set(false);
    validate(state);
}

/// Show why the bank cannot be saved, and enable Save only when it can.
fn validate(state: &Rc<State>) {
    let bank = state.bank.borrow();
    let clash = state
        .banks
        .borrow()
        .banks
        .iter()
        .any(|b| b.id != state.id && b.name.to_lowercase() == bank.name.trim().to_lowercase());
    let problem = match bank.validate() {
        Err(message) => Some(message),
        Ok(()) if clash => Some(format!("A bank named \"{}\" already exists", bank.name.trim())),
        Ok(()) if bank.buttons.iter().any(|b| b.label.is_empty() && b.icon.is_none()) => {
            state.error.set_text("Warning: a button has neither a label nor an icon");
            state.save_btn.set_sensitive(true);
            return;
        }
        Ok(()) => None,
    };
    state.error.set_text(problem.as_deref().unwrap_or(""));
    state.save_btn.set_sensitive(problem.is_none());
}

/// Write the form widgets back into the working copy.
fn wire_form(state: &Rc<State>) {
    let form = &state.form;
    let apply = |state: &Rc<State>, edit: &dyn Fn(&mut ButtonBank, Option<usize>)| {
        if state.loading.get() {
            return;
        }
        edit(&mut state.bank.borrow_mut(), state.selected.get());
        refresh_preview_only(state);
        validate(state);
    };
    {
        let state = state.clone();
        form.name.connect_changed(move |e| {
            let text = e.text().to_string();
            apply(&state, &|b, _| b.name = text.clone());
        });
    }
    {
        let state = state.clone();
        form.orientation.connect_selected_notify(move |d| {
            let vertical = d.selected() == 1;
            apply(&state, &|b, _| b.orientation = if vertical { Orientation::Vertical } else { Orientation::Horizontal });
        });
    }
    {
        let state = state.clone();
        form.columns.connect_value_changed(move |s| {
            let value = s.value() as u32;
            apply(&state, &|b, _| b.columns = value);
        });
    }
    {
        let state = state.clone();
        form.borderless.connect_toggled(move |c| {
            let on = c.is_active();
            apply(&state, &|b, _| b.borderless = on);
        });
    }
    {
        let state = state.clone();
        form.label.connect_changed(move |e| {
            let text = e.text().to_string();
            apply(&state, &|b, i| {
                if let Some(btn) = i.and_then(|i| b.buttons.get_mut(i)) {
                    btn.label = text.clone();
                }
            });
        });
    }
    {
        let state = state.clone();
        form.icon.connect_changed(move |e| {
            let text = e.text().trim().to_string();
            apply(&state, &|b, i| {
                if let Some(btn) = i.and_then(|i| b.buttons.get_mut(i)) {
                    btn.icon = if text.is_empty() { None } else { Some(text.clone().into()) };
                }
            });
        });
    }
    for (slot, dropdown) in form.functions.iter().enumerate() {
        let state = state.clone();
        dropdown.connect_selected_notify(move |d| {
            let choice = d.selected() as usize;
            let id = choice.checked_sub(1).and_then(|p| state.functions.get(p)).map(|f| f.id.clone());
            apply(&state, &|b, i| {
                let Some(btn) = i.and_then(|i| b.buttons.get_mut(i)) else { return };
                match slot {
                    0 => btn.left = id.clone(),
                    1 => btn.right = id.clone(),
                    _ => btn.middle = id.clone(),
                }
            });
        });
    }
}

/// Rebuild only the preview grid (the form keeps what the user is typing).
fn refresh_preview_only(state: &Rc<State>) {
    while let Some(child) = state.preview.first_child() {
        state.preview.remove(&child);
    }
    let on_click = {
        let state = state.clone();
        Rc::new(move |index: usize, _mouse: MouseButton| {
            state.selected.set(Some(index));
            load_form(&state);
        })
    };
    let ids: Vec<String> = state.functions.iter().map(|f| f.id.clone()).collect();
    state.preview.append(&grid::build_grid(&state.bank.borrow(), on_click, &|id| ids.iter().any(|i| i == id)));
}

/// What one toolbar button does to the editor state.
type ToolbarAction = Rc<dyn Fn(&Rc<State>)>;

/// Wire Add, Insert, Delete, Copy, Paste, Up and Down.
fn wire_toolbar(state: &Rc<State>, buttons: [&gtk::Button; 7]) {
    let [add, insert, delete, copy, paste, up, down] = buttons;
    let edit = |btn: &gtk::Button, action: ToolbarAction| {
        let state = state.clone();
        btn.connect_clicked(move |_| {
            action(&state);
            refresh_all(&state);
        });
    };
    let report = |state: &Rc<State>, result: Result<usize, String>| match result {
        Ok(index) => state.selected.set(Some(index)),
        Err(message) => show_error(&state.window, &message),
    };
    edit(add, Rc::new(move |s| {
        let end = s.bank.borrow().buttons.len();
        let result = s.bank.borrow_mut().insert_button(end, Btn { label: "Button".to_string(), ..Btn::default() });
        report(s, result);
    }));
    edit(insert, Rc::new(move |s| {
        let at = s.selected.get().unwrap_or(0);
        let result = s.bank.borrow_mut().insert_button(at, Btn { label: "Button".to_string(), ..Btn::default() });
        report(s, result);
    }));
    edit(delete, Rc::new(|s| {
        let Some(i) = s.selected.get() else { return };
        s.bank.borrow_mut().remove_button(i);
        let len = s.bank.borrow().buttons.len();
        s.selected.set(if len == 0 { None } else { Some(i.min(len - 1)) });
    }));
    edit(copy, Rc::new(|s| {
        let Some(i) = s.selected.get() else { return };
        *s.clipboard.borrow_mut() = s.bank.borrow().buttons.get(i).cloned();
    }));
    edit(paste, Rc::new(move |s| {
        let Some(button) = s.clipboard.borrow().clone() else { return };
        let at = s.selected.get().map_or(s.bank.borrow().buttons.len(), |i| i + 1);
        let result = s.bank.borrow_mut().insert_button(at, button);
        report(s, result);
    }));
    edit(up, Rc::new(|s| {
        let Some(i) = s.selected.get() else { return };
        if let Some(new) = s.bank.borrow_mut().move_button(i, -1) {
            s.selected.set(Some(new));
        }
    }));
    edit(down, Rc::new(|s| {
        let Some(i) = s.selected.get() else { return };
        if let Some(new) = s.bank.borrow_mut().move_button(i, 1) {
            s.selected.set(Some(new));
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store with one saved bank called "Tools", and its directory.
    fn store() -> (tempfile::TempDir, RefCell<Banks>, String) {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        let id = banks.create_in_dir(td.path(), "Tools").unwrap();
        banks.save_bank_to(td.path(), &id).unwrap();
        (td, RefCell::new(banks), id)
    }

    fn edited(name: &str) -> ButtonBank {
        ButtonBank { name: name.to_string(), buttons: vec![Btn { label: "One".to_string(), ..Btn::default() }], ..ButtonBank::default() }
    }

    #[test]
    fn save_edit_updates_the_store_and_writes_the_file_without_a_double_borrow() {
        let (td, banks, id) = store();
        save_edit(&banks, &id, edited("Renamed"), td.path()).unwrap();
        assert_eq!(banks.borrow().get(&id).unwrap().buttons.len(), 1);
        let on_disk = Banks::load_from_dir(td.path());
        assert_eq!(on_disk.get(&id).unwrap().name, "Renamed");
    }

    #[test]
    fn save_edit_leaves_the_store_alone_when_the_bank_is_invalid() {
        let (td, banks, id) = store();
        assert!(save_edit(&banks, &id, edited("  "), td.path()).is_err());
        assert_eq!(banks.borrow().get(&id).unwrap().name, "Tools");
    }

    #[test]
    fn save_edit_restores_the_store_when_the_file_cannot_be_written() {
        let (td, banks, id) = store();
        let blocker = td.path().join("blocker");
        std::fs::write(&blocker, "a file, not a directory").unwrap();
        let error = save_edit(&banks, &id, edited("Renamed"), &blocker.join("sub")).unwrap_err();
        assert!(error.contains("Cannot save"), "{error}");
        assert_eq!(banks.borrow().get(&id).unwrap().name, "Tools");
        assert!(banks.borrow().get(&id).unwrap().buttons.is_empty());
    }
}
