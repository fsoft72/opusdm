//! "Open with...": a window listing the applications that handle the type of
//! a file, plus an input box (with the Run window's suggestions) to pick any
//! other installed application by name.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_apps::search::{self, Catalog};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// Default window size.
const DIALOG_WIDTH: i32 = 480;
const DIALOG_HEIGHT: i32 = 520;

/// Icon size of a row.
const ROW_ICON_SIZE: i32 = 24;

/// Everything the dialog's callbacks share.
struct State {
    ui: Rc<Ui>,
    window: gtk::Window,
    files: Vec<gio::File>,
    /// The handlers of the file type, in the order of `handlers_list`.
    handlers: Vec<gio::AppInfo>,
    handlers_list: gtk::ListBox,
    input: gtk::Entry,
    suggestions_list: gtk::ListBox,
    catalog: Catalog,
    /// Catalog entry index of each row of `suggestions_list`.
    suggestions: RefCell<Vec<usize>>,
}

/// The content type of the file called `name`, guessed from the name.
fn content_type_of(name: &str) -> String {
    let (guess, _uncertain) = gio::content_type_guess(Some(name), None);
    guess.to_string()
}

/// Open the "Open with..." window for `paths`; the handlers shown are those
/// of the first path.
pub fn open_with(ui: &Rc<Ui>, paths: Vec<PathBuf>) {
    let Some(first) = paths.first() else {
        crate::actions::flash(ui, "no file selected");
        return;
    };
    let name = first.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let content_type = if first.is_dir() { "inode/directory".to_string() } else { content_type_of(&name) };
    let handlers = gio::AppInfo::all_for_type(&content_type);

    let shell = opusdm_ui::DialogShell::new(&ui.window, "Open with...").size(DIALOG_WIDTH, DIALOG_HEIGHT);
    let content = shell.content();
    let title = if paths.len() > 1 { format!("Open {} files with:", paths.len()) } else { format!("Open \"{name}\" with:") };
    let heading = gtk::Label::new(Some(&title));
    heading.set_xalign(0.0);
    content.append(&heading);

    let handlers_list = gtk::ListBox::new();
    handlers_list.set_selection_mode(gtk::SelectionMode::Single);
    for app in &handlers {
        handlers_list.append(&build_row(app.icon().as_ref(), &app.name(), &app.description().map(|d| d.to_string()).unwrap_or_default()));
    }
    if let Some(row) = handlers_list.row_at_index(0) {
        handlers_list.select_row(Some(&row));
    }
    let scroller = gtk::ScrolledWindow::builder().child(&handlers_list).vexpand(true).hexpand(true).build();
    scroller.set_has_frame(true);
    if handlers.is_empty() {
        let none = gtk::Label::new(Some(&format!("No application is registered for {content_type}")));
        none.set_margin_top(12);
        handlers_list.set_placeholder(Some(&none));
    }
    content.append(&scroller);

    let suggestions_list = gtk::ListBox::new();
    suggestions_list.set_selection_mode(gtk::SelectionMode::Single);
    suggestions_list.set_visible(false);
    content.append(&suggestions_list);
    let input = gtk::Entry::new();
    input.set_placeholder_text(Some("Or type the name of an application"));
    input.set_secondary_icon_name(Some("edit-clear-symbolic"));
    content.append(&input);

    let open_btn = shell.button("Open");
    shell.close_button("Cancel");
    let window = shell.present();

    let state = Rc::new(State {
        ui: ui.clone(),
        window,
        files: paths.iter().map(gio::File::for_path).collect(),
        handlers,
        handlers_list,
        input: input.clone(),
        suggestions_list,
        catalog: Catalog::new(opusdm_apps::entries::installed_entries()),
        suggestions: RefCell::new(Vec::new()),
    });
    wire(&state, &open_btn);
    input.grab_focus();
}

/// One row: icon, name and the dimmed comment.
fn build_row(icon: Option<&gio::Icon>, name: &str, comment: &str) -> gtk::ListBoxRow {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.set_margin_top(4);
    line.set_margin_bottom(4);
    line.set_margin_start(6);
    line.set_margin_end(6);
    let image = match icon {
        Some(icon) => gtk::Image::from_gicon(icon),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(ROW_ICON_SIZE);
    line.append(&image);
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    line.append(&label);
    let dim = gtk::Label::new(Some(comment));
    dim.set_opacity(0.6);
    dim.set_xalign(0.0);
    dim.set_hexpand(true);
    dim.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.append(&dim);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&line));
    row
}

/// Connect the lists, the input, the Open button and the keys.
fn wire(state: &Rc<State>, open_btn: &gtk::Button) {
    {
        let state = state.clone();
        state.input.clone().connect_changed(move |_| refresh(&state));
    }
    {
        let state = state.clone();
        state.input.clone().connect_activate(move |_| run_input(&state));
    }
    state.input.connect_icon_press(|input, position| {
        if position == gtk::EntryIconPosition::Secondary {
            input.set_text("");
        }
    });
    {
        let state = state.clone();
        state.handlers_list.clone().connect_row_activated(move |_, row| run_handler(&state, row.index()));
    }
    {
        let state = state.clone();
        state.suggestions_list.clone().connect_row_activated(move |_, row| run_suggestion(&state, row.index()));
    }
    {
        let state = state.clone();
        open_btn.connect_clicked(move |_| {
            if !state.input.text().trim().is_empty() {
                return run_input(&state);
            }
            match state.handlers_list.selected_row() {
                Some(row) => run_handler(&state, row.index()),
                None => crate::actions::flash(&state.ui, "no application selected"),
            }
        });
    }

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let state = state.clone();
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gtk::gdk::Key::Escape => {
                state.window.close();
                gtk::glib::Propagation::Stop
            }
            gtk::gdk::Key::Down if state.suggestions_list.is_visible() => {
                move_selection(&state, 1);
                gtk::glib::Propagation::Stop
            }
            gtk::gdk::Key::Up if state.suggestions_list.is_visible() => {
                move_selection(&state, -1);
                gtk::glib::Propagation::Stop
            }
            _ => gtk::glib::Propagation::Proceed,
        });
    }
    state.input.add_controller(keys);
}

/// Move the suggestion selection by `step` rows, staying inside the list.
fn move_selection(state: &State, step: i32) {
    let count = state.suggestions.borrow().len() as i32;
    if count == 0 {
        return;
    }
    let current = state.suggestions_list.selected_row().map(|row| row.index()).unwrap_or(-1);
    if let Some(row) = state.suggestions_list.row_at_index((current + step).clamp(0, count - 1)) {
        state.suggestions_list.select_row(Some(&row));
    }
}

/// Recompute the suggestions for the typed text and repaint the list, with
/// the first row selected.
fn refresh(state: &Rc<State>) {
    while let Some(row) = state.suggestions_list.first_child() {
        state.suggestions_list.remove(&row);
    }
    let found = search::suggest(&state.catalog, &state.input.text(), search::MAX_SUGGESTIONS);
    let mut indexes = Vec::with_capacity(found.len());
    for suggestion in &found {
        let entry = &state.catalog.entries()[suggestion.entry];
        let icon = entry.icon.as_deref().and_then(|text| gio::Icon::for_string(text).ok());
        state.suggestions_list.append(&build_row(icon.as_ref(), &entry.name, &entry.comment));
        indexes.push(suggestion.entry);
    }
    state.suggestions_list.set_visible(!found.is_empty());
    if let Some(row) = state.suggestions_list.row_at_index(0) {
        state.suggestions_list.select_row(Some(&row));
    }
    *state.suggestions.borrow_mut() = indexes;
}

/// Open the files with the handler on row `index` of the type's list.
fn run_handler(state: &Rc<State>, index: i32) {
    let Some(app) = usize::try_from(index).ok().and_then(|i| state.handlers.get(i)) else {
        opusdm_ipc::err_log!("opusdm-lister", "open with: no handler on row {index}");
        return;
    };
    launch(state, app);
}

/// Open the files with the suggested application on row `index`.
fn run_suggestion(state: &Rc<State>, index: i32) {
    let entry = usize::try_from(index).ok().and_then(|i| state.suggestions.borrow().get(i).copied());
    let Some(entry) = entry else {
        opusdm_ipc::err_log!("opusdm-lister", "open with: no suggestion on row {index}");
        return;
    };
    let id = &state.catalog.entries()[entry].id;
    match gio::DesktopAppInfo::new(id) {
        Some(info) => launch(state, info.upcast_ref()),
        None => crate::actions::flash(&state.ui, &format!("application '{id}' is not installed")),
    }
}

/// Enter in the input: open with the selected (else first) suggestion.
fn run_input(state: &Rc<State>) {
    if state.suggestions.borrow().is_empty() {
        return crate::actions::flash(&state.ui, "no application matches");
    }
    let index = state.suggestions_list.selected_row().map(|row| row.index()).unwrap_or(0);
    run_suggestion(state, index);
}

/// Start `app` on the files; success closes the window, a failure is shown
/// in the Lister's message line.
fn launch(state: &Rc<State>, app: &gio::AppInfo) {
    match opusdm_apps::apps::open_files(app, &state.files) {
        Ok(()) => state.window.close(),
        Err(msg) => {
            opusdm_ipc::err_log!("opusdm-lister", "{msg}");
            crate::actions::flash(&state.ui, &msg);
        }
    }
}
