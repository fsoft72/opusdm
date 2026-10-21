//! File types in the Lister (F-06): finding the type of a file, running its
//! actions, and the File Type editor with the Filetype Creator.

use crate::filetype_form::{Cond, CondKind, Draft};
use crate::model::Row;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::filetypes::{self, FileFacts, FileType, FileTypes};
use opusdm_ipc::function::{Function, FunctionId};
use opusdm_ui::EmbeddedTool;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

/// The user file type of the file at `path`, or `None` when none matches (or
/// `row` is a directory). `filetypes.json` comes from the in-memory cache,
/// which drops its copy as soon as the file changes, so an edit applies at
/// once. Only the few leading bytes a magic rule needs are read from `path`.
pub fn type_for(path: &Path, row: &Row) -> Option<FileType> {
    if row.is_dir { return None; }
    let types = crate::config_cache::filetypes();
    let head = filetypes::read_head(path, types.head_needed());
    let facts = FileFacts { name: &row.name, size: row.size, modified: row.modified_unix, head: &head };
    types.find(&facts).cloned()
}

/// Run `function` on the file called `name`: it becomes the only selected
/// row, because functions work on the selection.
pub fn run_on_row(ui: &Rc<Ui>, name: &str, function: &FunctionId) {
    ui.view.selection.unselect_all();
    ui.select_names(&[name.to_string()]);
    ui.start_run(opusdm_ipc::ToHub::RunFunctionById { id: function.clone() });
}

/// The "Open With" entries of `file_type`: reference and label (the
/// function name, or the command for an inline bash or exec one, the latter
/// with no `exec:` prefix). Saved
/// functions that no longer exist and inline commands that cannot run are
/// left out.
pub fn menu_entries(file_type: &FileType, functions: &[Function]) -> Vec<(FunctionId, String)> {
    file_type
        .actions
        .menu
        .iter()
        .filter(|id| match opusdm_ipc::funcref::parse(id) {
            opusdm_ipc::funcref::FuncRef::Saved(_) => functions.iter().any(|f| &f.id == *id),
            inline => opusdm_ipc::funcref::problem(&inline).is_none(),
        })
        .map(|id| (id.clone(), menu_label(id, functions)))
        .collect()
}

/// The label of an Open With entry. Like [`opusdm_ipc::funcref::title`], but an
/// exec command shows its command line with no `exec:` prefix.
fn menu_label(reference: &str, functions: &[Function]) -> String {
    match opusdm_ipc::funcref::parse(reference) {
        opusdm_ipc::funcref::FuncRef::Exec { line } => line,
        _ => opusdm_ipc::funcref::title(reference, functions),
    }
}

/// Problems in the actions of one file type: an inline bash or exec command
/// that cannot run, and an Open With entry nobody chose. A saved function id
/// is not checked (a deleted one is just left out of the menu).
fn action_problems(actions: &filetypes::Actions) -> Vec<String> {
    let issue = |reference: &str| opusdm_ipc::funcref::problem(&opusdm_ipc::funcref::parse(reference));
    let mut problems = Vec::new();
    for (caption, reference) in [("Double click", &actions.dblclick), ("Drop", &actions.drop)] {
        if let Some(reason) = reference.as_deref().and_then(issue) {
            problems.push(format!("{caption}: {reason}"));
        }
    }
    for (i, reference) in actions.menu.iter().enumerate() {
        if reference.is_empty() {
            problems.push(format!("Open With entry {}: choose what it runs", i + 1));
        } else if let Some(reason) = issue(reference) {
            problems.push(format!("Open With entry {}: {reason}", i + 1));
        }
    }
    problems
}

/// Everything the editor's callbacks share.
struct State {
    drafts: RefCell<Vec<Draft>>,
    current: Cell<Option<usize>>,
    functions: Vec<Function>,
    window: gtk::Window,
    list: gtk::ListBox,
    form: gtk::Box,
    status: gtk::Label,
    save_btn: gtk::Button,
    /// Set while widgets are being filled, so their change signals are ignored.
    building: Cell<bool>,
    /// Runs after every validation, so a host can follow the unsaved state.
    on_change: Rc<dyn Fn()>,
}

/// Open the File Types editor. With `sample`, a new type derived from that
/// file (the Filetype Creator) is added and selected.
pub fn open_editor(ui: &Rc<Ui>, sample: Option<(String, std::path::PathBuf)>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "File Types");
    let functions = ui.hub.functions.borrow().clone();
    let ui = ui.clone();
    build_editor(shell, functions, sample, Rc::new(move |text| crate::actions::flash(&ui, text)));
}

/// Open the File Types editor on its own, without a Lister window. The
/// functions come from the hub (from disk when no hub answers) and the save
/// result is reported on stderr.
pub fn open_standalone(app: &gtk::Application) {
    let shell = opusdm_ui::DialogShell::standalone(app, "File Types");
    gtk::glib::spawn_future_local(async move {
        let functions = match hub_functions().await {
            Ok(functions) => functions,
            Err(e) => {
                opusdm_ipc::err_log!("opusdm-lister", "File Types: {e}; using the Functions on disk");
                opusdm_ipc::function::load_functions()
            }
        };
        build_editor(shell, functions, None, Rc::new(|text| opusdm_ipc::err_log!("opusdm-lister", "File Types: {text}")));
    });
}

/// The saved Functions as the hub holds them.
async fn hub_functions() -> Result<Vec<Function>, String> {
    let mut client = crate::client::Client::connect_tool(&opusdm_ipc::sockpath::hub_sock_path()).map_err(|e| format!("cannot reach the OpusDM hub: {e}"))?;
    let inbox = client.take_inbox();
    crate::client::fetch_config(&client, &inbox).await.map(|(_, functions)| functions)
}

/// Why a save did not happen.
enum SaveError {
    /// The drafts have problems; the text lists them.
    Invalid(String),
    /// The file could not be written; the text says why.
    Write(String),
}

/// The editor body (list, form and status line) and what tells whether it
/// differs from the saved file types.
struct Body {
    state: Rc<State>,
    root: gtk::Box,
    /// The drafts as last saved (or loaded).
    saved: RefCell<Vec<Draft>>,
}

impl Body {
    /// Whether the drafts differ from the saved file types.
    fn is_dirty(&self) -> bool {
        *self.state.drafts.borrow() != *self.saved.borrow()
    }

    /// Save the drafts to disk.
    fn save(&self) -> Result<(), SaveError> {
        let types = build_types(&self.state).map_err(|messages| SaveError::Invalid(messages.join("; ")))?;
        filetypes::save_filetypes(&types).map_err(|e| SaveError::Write(format!("could not save file types: {e}")))?;
        crate::config_cache::invalidate_filetypes();
        *self.saved.borrow_mut() = self.state.drafts.borrow().clone();
        Ok(())
    }

    /// Put the saved file types back in the list and the form.
    fn revert(&self) {
        let state = &self.state;
        *state.drafts.borrow_mut() = self.saved.borrow().clone();
        let count = state.drafts.borrow().len();
        state.current.set(state.current.get().filter(|i| *i < count).or(if count > 0 { Some(0) } else { None }));
        refresh_list(state);
        show_form(state);
    }
}

/// Build the editor body. `window` owns the dialogs the form opens; `save_btn`
/// is enabled only while nothing is wrong. With `sample`, a new type derived
/// from that file (the Filetype Creator) is added and selected.
fn build_body(
    window: &gtk::Window,
    functions: Vec<opusdm_ipc::function::Function>,
    sample: Option<(String, std::path::PathBuf)>,
    save_btn: gtk::Button,
    on_change: Rc<dyn Fn()>,
) -> Body {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    body.set_vexpand(true);
    root.append(&body);

    // Left: the ordered list and its buttons.
    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let list = gtk::ListBox::new();
    let list_scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).min_content_width(220).build();
    left.append(&list_scroll);
    let list_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let add_btn = gtk::Button::with_label("New");
    let remove_btn = gtk::Button::with_label("Delete");
    let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
    let down_btn = gtk::Button::from_icon_name("go-down-symbolic");
    for b in [&add_btn, &remove_btn, &up_btn, &down_btn] {
        list_buttons.append(b);
    }
    left.append(&list_buttons);
    body.append(&left);

    // Right: the form of the selected type.
    let form = gtk::Box::new(gtk::Orientation::Vertical, 6);
    form.set_hexpand(true);
    let form_scroll = gtk::ScrolledWindow::builder().child(&form).hexpand(true).vexpand(true).build();
    body.append(&form_scroll);

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.add_css_class("error");
    root.append(&status);

    let drafts: Vec<Draft> = filetypes::load_filetypes().types.iter().map(Draft::from_type).collect();
    let saved = drafts.clone();
    let state = Rc::new(State {
        drafts: RefCell::new(drafts),
        current: Cell::new(None),
        functions,
        window: window.clone(),
        list,
        form,
        status,
        save_btn,
        building: Cell::new(false),
        on_change,
    });

    if let Some((name, path)) = sample {
        let head = filetypes::read_head(&path, filetypes::HEAD_BYTES);
        let rule = filetypes::derive_rule(&name, &head);
        let mut draft = Draft::blank(&filetypes::derive_name(&name));
        if let Some(cond) = Cond::from_rule(&rule) {
            draft.conds = vec![cond];
        }
        let mut drafts = state.drafts.borrow_mut();
        drafts.push(draft);
        state.current.set(Some(drafts.len() - 1));
    } else if !state.drafts.borrow().is_empty() {
        state.current.set(Some(0));
    }

    {
        let state = state.clone();
        state.list.clone().connect_row_selected(move |_, row| {
            if state.building.get() {
                return;
            }
            let index = row.map(|r| r.index() as usize);
            if index == state.current.get() {
                return;
            }
            state.current.set(index);
            show_form(&state);
        });
    }
    {
        let state = state.clone();
        add_btn.connect_clicked(move |_| {
            let mut drafts = state.drafts.borrow_mut();
            drafts.push(Draft::blank("New file type"));
            state.current.set(Some(drafts.len() - 1));
            drop(drafts);
            refresh_list(&state);
            show_form(&state);
        });
    }
    {
        let state = state.clone();
        remove_btn.connect_clicked(move |_| {
            let Some(i) = state.current.get() else { return };
            let mut drafts = state.drafts.borrow_mut();
            if i >= drafts.len() {
                return;
            }
            drafts.remove(i);
            state.current.set(if drafts.is_empty() { None } else { Some(i.min(drafts.len() - 1)) });
            drop(drafts);
            refresh_list(&state);
            show_form(&state);
        });
    }
    for (button, step) in [(up_btn, -1isize), (down_btn, 1isize)] {
        let state = state.clone();
        button.connect_clicked(move |_| {
            let Some(i) = state.current.get() else { return };
            let mut drafts = state.drafts.borrow_mut();
            let Some(j) = i.checked_add_signed(step).filter(|j| *j < drafts.len()) else { return };
            drafts.swap(i, j);
            state.current.set(Some(j));
            drop(drafts);
            refresh_list(&state);
        });
    }
    refresh_list(&state);
    show_form(&state);
    validate(&state);
    Body { state, root, saved: RefCell::new(saved) }
}

/// Fill `shell` with the File Types editor. `report` receives the outcome of
/// each Save.
fn build_editor(
    shell: opusdm_ui::DialogShell,
    functions: Vec<opusdm_ipc::function::Function>,
    sample: Option<(String, std::path::PathBuf)>,
    report: Rc<dyn Fn(&str)>,
) {
    let shell = shell.size(820, 520);
    let save_btn = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Close");
    let body = build_body(shell.window(), functions, sample, save_btn.clone(), Rc::new(|| {}));
    shell.content().append(&body.root);
    let status = body.state.status.clone();
    save_btn.connect_clicked(move |_| match body.save() {
        Ok(()) => report("file types saved"),
        Err(SaveError::Write(message)) => report(&message),
        Err(SaveError::Invalid(message)) => status.set_text(&message),
    });
    shell.present();
}

/// The File Types editor inside another window (Advanced tab of Settings): the
/// list and form, and its own Save and Revert buttons.
pub struct FileTypesTool {
    root: gtk::Box,
    body: Rc<RefCell<Option<Body>>>,
    save_btn: gtk::Button,
    revert_btn: gtk::Button,
}

/// Open the File Types editor for the host `window`. The Functions come from
/// the hub (from disk when no hub answers), so the editor appears a moment
/// after the page.
pub fn embedded(window: &gtk::Window) -> Rc<FileTypesTool> {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);
    let slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    slot.set_vexpand(true);
    slot.append(&gtk::Label::new(Some("Loading the file types...")));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let revert_btn = gtk::Button::with_label("Revert");
    let save_btn = gtk::Button::with_label("Save");
    revert_btn.set_sensitive(false);
    save_btn.set_sensitive(false);
    buttons.append(&revert_btn);
    buttons.append(&save_btn);
    root.append(&slot);
    root.append(&buttons);

    let tool = Rc::new(FileTypesTool { root, body: Rc::new(RefCell::new(None)), save_btn: save_btn.clone(), revert_btn: revert_btn.clone() });
    let weak = Rc::downgrade(&tool);
    save_btn.connect_clicked({
        let weak = weak.clone();
        move |_| match weak.upgrade() {
            Some(tool) => tool.save(Box::new(|_| {})),
            None => opusdm_ipc::err_log!("opusdm-lister", "File Types: the editor is gone, Save ignored"),
        }
    });
    revert_btn.connect_clicked({
        let weak = weak.clone();
        move |_| match weak.upgrade() {
            Some(tool) => tool.revert(),
            None => opusdm_ipc::err_log!("opusdm-lister", "File Types: the editor is gone, Revert ignored"),
        }
    });
    let window = window.clone();
    gtk::glib::spawn_future_local(async move {
        let functions = match hub_functions().await {
            Ok(functions) => functions,
            Err(e) => {
                opusdm_ipc::err_log!("opusdm-lister", "File Types: {e}; using the Functions on disk");
                opusdm_ipc::function::load_functions()
            }
        };
        let Some(tool) = weak.upgrade() else {
            opusdm_ipc::err_log!("opusdm-lister", "File Types: the editor was closed before the Functions arrived");
            return;
        };
        // Weak: the body owns the state that owns this closure.
        let (body_slot, revert_btn) = (Rc::downgrade(&tool.body), tool.revert_btn.clone());
        let on_change: Rc<dyn Fn()> = Rc::new(move || {
            let dirty = body_slot.upgrade().is_some_and(|b| b.borrow().as_ref().is_some_and(Body::is_dirty));
            revert_btn.set_sensitive(dirty);
        });
        let body = build_body(&window, functions, None, tool.save_btn.clone(), on_change);
        while let Some(child) = slot.first_child() {
            slot.remove(&child);
        }
        slot.append(&body.root);
        *tool.body.borrow_mut() = Some(body);
    });
    tool
}

impl EmbeddedTool for FileTypesTool {
    fn title(&self) -> &str {
        "File Types"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    fn is_dirty(&self) -> bool {
        self.body.borrow().as_ref().is_some_and(Body::is_dirty)
    }

    fn save(&self, done: Box<dyn FnOnce(bool)>) {
        let body = self.body.borrow();
        let Some(body) = body.as_ref() else {
            opusdm_ipc::err_log!("opusdm-lister", "File Types: Save before the editor was loaded");
            return done(false);
        };
        match body.save() {
            Ok(()) => {
                self.revert_btn.set_sensitive(false);
                done(true);
            }
            Err(SaveError::Invalid(message)) => {
                body.state.status.set_text(&message);
                done(false);
            }
            Err(SaveError::Write(message)) => {
                opusdm_ipc::err_log!("opusdm-lister", "File Types: {message}");
                body.state.status.set_text(&message);
                done(false);
            }
        }
    }

    fn revert(&self) {
        if let Some(body) = self.body.borrow().as_ref() { body.revert(); }
        self.revert_btn.set_sensitive(false);
    }
}

/// Convert every draft; the error lists each problem found.
fn build_types(state: &State) -> Result<FileTypes, Vec<String>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut types = Vec::new();
    let mut errors = Vec::new();
    for draft in state.drafts.borrow().iter() {
        match draft.to_type(now) {
            Ok(t) => types.push(t),
            Err(e) => errors.push(format!("{}: {e}", if draft.name.trim().is_empty() { "(unnamed)" } else { draft.name.trim() })),
        }
    }
    let list = FileTypes { types };
    errors.extend(list.problems());
    for t in &list.types {
        errors.extend(action_problems(&t.actions).into_iter().map(|e| format!("{}: {e}", t.name)));
    }
    if errors.is_empty() {
        Ok(list)
    } else {
        Err(errors)
    }
}

/// Show the first problem and gate Save.
fn validate(state: &State) {
    match build_types(state) {
        Ok(_) => {
            state.status.set_text("");
            state.save_btn.set_sensitive(true);
        }
        Err(errors) => {
            state.status.set_text(&errors.join("; "));
            state.save_btn.set_sensitive(false);
        }
    }
    (state.on_change)();
}

/// Repaint the list of names, keeping the selection on `current`.
fn refresh_list(state: &Rc<State>) {
    state.building.set(true);
    while let Some(row) = state.list.first_child() {
        state.list.remove(&row);
    }
    for draft in state.drafts.borrow().iter() {
        let label = gtk::Label::new(Some(if draft.name.trim().is_empty() { "(unnamed)" } else { draft.name.as_str() }));
        label.set_xalign(0.0);
        label.set_margin_start(6);
        label.set_margin_end(6);
        label.set_margin_top(4);
        label.set_margin_bottom(4);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&label));
        state.list.append(&row);
    }
    if let Some(row) = state.current.get().and_then(|i| state.list.row_at_index(i as i32)) {
        state.list.select_row(Some(&row));
    }
    state.building.set(false);
}

/// Run `edit` on the draft being edited, then re-validate.
fn edit_current(state: &Rc<State>, edit: impl FnOnce(&mut Draft)) {
    if state.building.get() {
        return;
    }
    let Some(i) = state.current.get() else { return };
    if let Some(draft) = state.drafts.borrow_mut().get_mut(i) {
        edit(draft);
    }
    validate(state);
}

/// Rebuild the right-hand form for the selected type.
fn show_form(state: &Rc<State>) {
    state.building.set(true);
    while let Some(child) = state.form.first_child() {
        state.form.remove(&child);
    }
    let draft = state.current.get().and_then(|i| state.drafts.borrow().get(i).cloned());
    let Some(draft) = draft else {
        state.form.append(&gtk::Label::new(Some("No file type selected. Use New, or the Filetype Creator.")));
        state.building.set(false);
        return;
    };

    state.form.append(&captioned("Name", &name_entry(state, &draft)));

    if draft.locked_rule.is_some() {
        let note = gtk::Label::new(Some("This type uses nested rules. Edit filetypes.json to change them."));
        note.set_xalign(0.0);
        state.form.append(&note);
    } else {
        state.form.append(&match_section(state, &draft));
    }

    state.form.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    state.form.append(&actions_section(state, &draft));
    state.building.set(false);
}

/// A bold caption above `widget`.
fn captioned(caption: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let label = gtk::Label::new(None);
    label.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(caption)));
    label.set_xalign(0.0);
    b.append(&label);
    b.append(widget);
    b
}

/// The type name entry.
fn name_entry(state: &Rc<State>, draft: &Draft) -> gtk::Entry {
    let entry = gtk::Entry::new();
    entry.set_text(&draft.name);
    let state = state.clone();
    entry.connect_changed(move |e| {
        let text = e.text().to_string();
        edit_current(&state, |d| d.name = text);
        // Keep the list label in step without rebuilding the form.
        if let Some(i) = state.current.get() {
            if let Some(label) = state.list.row_at_index(i as i32).and_then(|r| r.child()).and_downcast::<gtk::Label>() {
                let shown = e.text();
                label.set_text(if shown.trim().is_empty() { "(unnamed)" } else { shown.as_str() });
            }
        }
    });
    entry
}

/// The "Match" part: all/any drop-down and the condition rows.
fn match_section(state: &Rc<State>, draft: &Draft) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let title = gtk::Label::new(None);
    title.set_markup("<b>Match</b>");
    head.append(&title);
    let mode = gtk::DropDown::from_strings(&["all of", "any of"]);
    mode.set_selected(u32::from(draft.any));
    {
        let state = state.clone();
        mode.connect_selected_notify(move |d| {
            let any = d.selected() == 1;
            edit_current(&state, |draft| draft.any = any);
        });
    }
    head.append(&mode);
    head.append(&gtk::Label::new(Some("these conditions")));
    section.append(&head);

    for (index, cond) in draft.conds.iter().enumerate() {
        section.append(&cond_row(state, index, cond));
    }
    let add = gtk::Button::with_label("Add Condition");
    add.set_halign(gtk::Align::Start);
    {
        let state = state.clone();
        add.connect_clicked(move |_| {
            edit_current(&state, |d| d.conds.push(Cond::new(CondKind::Name)));
            show_form(&state);
        });
    }
    section.append(&add);
    section
}

/// One condition row: kind, two entries, remove.
fn cond_row(state: &Rc<State>, index: usize, cond: &Cond) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let labels: Vec<&str> = CondKind::ALL.iter().map(|k| k.label()).collect();
    let kind = gtk::DropDown::from_strings(&labels);
    kind.set_selected(CondKind::ALL.iter().position(|k| *k == cond.kind).unwrap_or(0) as u32);
    {
        let state = state.clone();
        kind.connect_selected_notify(move |d| {
            if state.building.get() {
                return;
            }
            let Some(new_kind) = CondKind::ALL.get(d.selected() as usize).copied() else { return };
            edit_current(&state, |draft| {
                if let Some(c) = draft.conds.get_mut(index).filter(|c| c.kind != new_kind) {
                    *c = Cond::new(new_kind);
                }
            });
            show_form(&state);
        });
    }
    row.append(&kind);

    let (hint_a, hint_b) = cond.kind.hints();
    for (second, text, hint) in [(false, &cond.a, hint_a), (true, &cond.b, hint_b)] {
        if second && hint.is_empty() {
            continue;
        }
        let entry = gtk::Entry::new();
        entry.set_text(text);
        entry.set_placeholder_text(Some(hint));
        entry.set_hexpand(true);
        let state = state.clone();
        entry.connect_changed(move |e| {
            let text = e.text().to_string();
            edit_current(&state, |draft| {
                if let Some(c) = draft.conds.get_mut(index) {
                    if second {
                        c.b = text;
                    } else {
                        c.a = text;
                    }
                }
            });
        });
        row.append(&entry);
    }

    let remove = gtk::Button::from_icon_name("list-remove-symbolic");
    remove.add_css_class("flat");
    {
        let state = state.clone();
        remove.connect_clicked(move |_| {
            edit_current(&state, |d| {
                if index < d.conds.len() {
                    d.conds.remove(index);
                }
            });
            show_form(&state);
        });
    }
    row.append(&remove);
    row
}

/// The "Actions" part: double click, drop and Open With.
fn actions_section(state: &Rc<State>, draft: &Draft) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let title = gtk::Label::new(None);
    title.set_markup("<b>Actions</b>");
    title.set_xalign(0.0);
    section.append(&title);

    section.append(&labelled("Double click", &optional_picker(state, "(default application)", draft.dblclick.as_deref(), |d, chosen| d.dblclick = chosen)));
    section.append(&labelled("Drop (stored, not wired yet)", &optional_picker(state, "(nothing)", draft.drop.as_deref(), |d, chosen| d.drop = chosen)));

    let menu_title = gtk::Label::new(Some("Open With menu"));
    menu_title.set_xalign(0.0);
    section.append(&menu_title);
    for (index, id) in draft.menu.iter().enumerate() {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let picker = {
            let state = state.clone();
            opusdm_ui::RefPicker::new(&state.functions.clone(), id, &state.window.clone(), None, move |chosen| {
                edit_current(&state, |draft| {
                    if let Some(slot) = draft.menu.get_mut(index) {
                        *slot = chosen;
                    }
                });
            })
        };
        picker.widget().set_hexpand(true);
        line.append(picker.widget());
        let remove = gtk::Button::from_icon_name("list-remove-symbolic");
        remove.add_css_class("flat");
        {
            let state = state.clone();
            remove.connect_clicked(move |_| {
                edit_current(&state, |d| {
                    if index < d.menu.len() {
                        d.menu.remove(index);
                    }
                });
                show_form(&state);
            });
        }
        line.append(&remove);
        section.append(&line);
    }
    let add = gtk::Button::with_label("Add Menu Entry");
    add.set_halign(gtk::Align::Start);
    {
        let state = state.clone();
        add.connect_clicked(move |_| {
            // A new entry starts on the first saved function (or empty, which the
            // picker shows as nothing chosen); the user picks what it runs.
            let first = state.functions.first().map(|f| f.id.clone()).unwrap_or_default();
            edit_current(&state, |d| d.menu.push(first));
            show_form(&state);
        });
    }
    section.append(&add);
    section
}

/// A picker for an optional action: `none` names the "nothing" choice, `current`
/// is the stored reference, and `set` stores the new one (`None` for nothing)
/// in the draft being edited.
fn optional_picker(state: &Rc<State>, none: &str, current: Option<&str>, set: impl Fn(&mut Draft, Option<FunctionId>) + 'static) -> gtk::Box {
    let picker = {
        let state = state.clone();
        opusdm_ui::RefPicker::new(&state.functions.clone(), current.unwrap_or(""), &state.window.clone(), Some(none), move |chosen| {
            let chosen = if chosen.is_empty() { None } else { Some(chosen) };
            edit_current(&state, |draft| set(draft, chosen));
        })
    };
    picker.widget().clone()
}

/// `caption` on the left, `widget` stretching on the right.
fn labelled(caption: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::new(Some(caption));
    label.set_width_chars(26);
    label.set_xalign(0.0);
    line.append(&label);
    widget.set_hexpand(true);
    line.append(widget);
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::filetypes::{Actions, MatchRule};

    fn func(id: &str, name: &str) -> Function {
        let mut f = Function::new(name);
        f.id = id.to_string();
        f
    }

    #[test]
    fn menu_entries_use_function_names_and_skip_missing_ones() {
        let t = FileType {
            name: "T".into(),
            rule: MatchRule::Glob { pattern: "*".into() },
            actions: Actions { dblclick: None, drop: None, menu: vec!["a".into(), "gone".into(), "b".into()] },
        };
        let functions = vec![func("a", "Alpha"), func("b", "Beta")];
        assert_eq!(
            menu_entries(&t, &functions),
            vec![("a".to_string(), "Alpha".to_string()), ("b".to_string(), "Beta".to_string())]
        );
    }

    #[test]
    fn action_problems_flag_broken_inline_commands_and_empty_open_with_entries() {
        let actions = Actions {
            dblclick: Some("bash:".into()),
            drop: Some("bash:ls".into()),
            menu: vec!["a".into(), String::new(), "exec:opusdm-no-such-program-xyz".into()],
        };
        let problems = action_problems(&actions);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].starts_with("Double click:"));
        assert!(problems[1].starts_with("Open With entry 2:") && problems[1].contains("choose"));
        assert!(problems[2].starts_with("Open With entry 3:") && problems[2].contains("was not found"));
        let clean = Actions { dblclick: None, drop: None, menu: vec!["a".into(), "bash:ls".into()] };
        assert!(action_problems(&clean).is_empty(), "a saved id (even a deleted one) is not flagged here");
    }

    #[test]
    fn menu_entries_keep_runnable_inline_commands_and_drop_broken_ones() {
        let t = FileType {
            name: "T".into(),
            rule: MatchRule::Glob { pattern: "*".into() },
            actions: Actions {
                dblclick: None,
                drop: None,
                menu: vec!["bash:ls".into(), "a".into(), "exec:opusdm-no-such-program-xyz".into(), "bash:".into(), "gone".into()],
            },
        };
        let functions = vec![func("a", "Alpha")];
        assert_eq!(
            menu_entries(&t, &functions),
            vec![("bash:ls".to_string(), "bash: ls".to_string()), ("a".to_string(), "Alpha".to_string())]
        );
    }

    #[test]
    fn an_exec_entry_is_labelled_with_its_command_line_and_no_prefix() {
        let t = FileType {
            name: "T".into(),
            rule: MatchRule::Glob { pattern: "*".into() },
            actions: Actions { dblclick: None, drop: None, menu: vec!["exec:ls -l {f}".into()] },
        };
        assert_eq!(menu_entries(&t, &[]), vec![("exec:ls -l {f}".to_string(), "ls -l {f}".to_string())]);
    }
}
