//! A terminal window: a notebook of tabs, each holding a split layout, plus
//! the hotkeys and the actions that work on the focused terminal.

use crate::actions::Action;
use crate::layout::{self, Direction, TAB_ROOT_CLASS};
use crate::pane;
use crate::shared::Shared;
use gtk4 as gtk;
use gtk::{gdk, gio, glib};
use std::path::Path;
use std::rc::Rc;
use vte4::prelude::*;

/// Title of a window (and label of a tab) whose shell set no title.
const DEFAULT_TITLE: &str = "OpusDM Terminal";

/// Default window size in pixels.
const DEFAULT_SIZE: (i32, i32) = (900, 560);

/// Shortest and longest tab label, in characters; a longer title is cut
/// with an ellipsis.
const TAB_LABEL_CHARS: (i32, i32) = (12, 28);

/// Open a new window with one tab running `command` (the shell when `None`)
/// in `cwd`.
pub fn open_window(shared: &Rc<Shared>, cwd: Option<&Path>, command: Option<&[String]>) {
    let window = gtk::ApplicationWindow::builder()
        .application(&shared.app)
        .title(DEFAULT_TITLE)
        .default_width(DEFAULT_SIZE.0)
        .default_height(DEFAULT_SIZE.1)
        .build();
    window.add_css_class("opusdm-terminal");

    let notebook = gtk::Notebook::new();
    notebook.set_scrollable(true);
    notebook.set_show_border(false);
    notebook.set_show_tabs(false);
    notebook.connect_page_added(|nb, _, _| nb.set_show_tabs(nb.n_pages() > 1));
    {
        let window = window.clone();
        notebook.connect_page_removed(move |nb, _, _| {
            nb.set_show_tabs(nb.n_pages() > 1);
            if nb.n_pages() == 0 { window.close(); }
        });
    }
    {
        let shared = shared.clone();
        notebook.connect_switch_page(move |_, page, _| {
            let page = page.clone();
            let shared = shared.clone();
            // The page is not mapped yet while the signal runs.
            glib::idle_add_local_once(move || focus_tab(&shared, &page));
        });
    }
    window.set_child(Some(&notebook));
    attach_hotkeys(&window, shared);
    new_tab(shared, &notebook, cwd, command);
    window.present();
}

/// Add a tab to `notebook` running `command` in `cwd` and switch to it.
pub fn new_tab(shared: &Rc<Shared>, notebook: &gtk::Notebook, cwd: Option<&Path>, command: Option<&[String]>) {
    let term = pane::new_terminal(shared, cwd, command);
    let Some(pane_box) = term.parent() else { return };
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class(TAB_ROOT_CLASS);
    root.append(&pane_box);
    pane_box.set_vexpand(true);

    let label = gtk::Label::new(Some(DEFAULT_TITLE));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(TAB_LABEL_CHARS.0);
    label.set_max_width_chars(TAB_LABEL_CHARS.1);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("flat");
    close.set_tooltip_text(Some("Close tab"));
    {
        let root = root.clone();
        close.connect_clicked(move |_| close_tab(root.upcast_ref()));
    }
    let tab_label = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    tab_label.append(&label);
    tab_label.append(&close);

    let index = notebook.append_page(&root, Some(&tab_label));
    notebook.set_tab_reorderable(&root, true);
    notebook.set_current_page(Some(index));
    term.grab_focus();
}

/// Close the tab whose root box is `root`.
pub fn close_tab(root: &gtk::Widget) {
    let Some(notebook) = root.ancestor(gtk::Notebook::static_type()).and_downcast::<gtk::Notebook>() else { return };
    if let Some(index) = notebook.page_num(root) { notebook.remove_page(Some(index)); }
}

/// Give the focus to the terminal of `page` used last (or its first one).
fn focus_tab(shared: &Rc<Shared>, page: &gtk::Widget) {
    let term = shared.last_focused_in(page).or_else(|| layout::terminals_in(page).into_iter().next());
    if let Some(term) = term { term.grab_focus(); }
}

/// Update the label of the tab of `term` and, when it has the focus, the
/// window title, from the title its shell set.
pub fn refresh_titles(shared: &Rc<Shared>, term: &vte4::Terminal) {
    let Some(root) = layout::tab_root_of(term.upcast_ref()) else { return };
    // A tab shows the title of the terminal used last in it.
    if shared.last_focused_in(&root).is_some_and(|t| &t != term) { return; }
    let title = term.window_title().map(|t| t.to_string()).filter(|t| !t.is_empty());
    let title = title.as_deref().unwrap_or(DEFAULT_TITLE);
    let notebook = root.ancestor(gtk::Notebook::static_type()).and_downcast::<gtk::Notebook>();
    let label = notebook
        .and_then(|nb| nb.tab_label(&root))
        .and_then(|b| b.first_child())
        .and_downcast::<gtk::Label>();
    if let Some(label) = label {
        label.set_text(title);
        label.set_tooltip_text(Some(title));
    }
    let Some(window) = term.root().and_downcast::<gtk::Window>() else { return };
    if term.has_focus() { window.set_title(Some(title)); }
}

/// Run the configured hotkeys before VTE sees the keys (capture phase).
fn attach_hotkeys(window: &gtk::ApplicationWindow, shared: &Rc<Shared>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let shared = shared.clone();
    let window_weak = window.downgrade();
    keys.connect_key_pressed(move |controller, _, _, _| {
        let Some(window) = window_weak.upgrade() else { return glib::Propagation::Proceed };
        let Some(event) = controller.current_event().and_then(|e| e.downcast::<gdk::KeyEvent>().ok()) else { return glib::Propagation::Proceed };
        let Some(action) = matching_action(&shared, &event) else { return glib::Propagation::Proceed };
        let Some(term) = current_terminal(&shared, &window) else { return glib::Propagation::Proceed };
        run_action(&shared, action, &term);
        glib::Propagation::Stop
    });
    window.add_controller(keys);
}

/// The action whose hotkey `event` is, if any.
fn matching_action(shared: &Shared, event: &gdk::KeyEvent) -> Option<Action> {
    let config = shared.config();
    Action::ALL.into_iter().find(|&action| {
        let accel = config.hotkey(action);
        if accel.is_empty() { return false; }
        gtk::accelerator_parse(&accel).is_some_and(|(key, mods)| event.matches(key, mods) == gdk::KeyMatch::Exact)
    })
}

/// The terminal the keyboard is in, or the one used last in the current tab.
fn current_terminal(shared: &Shared, window: &gtk::ApplicationWindow) -> Option<vte4::Terminal> {
    if let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) {
        let term = focus.downcast_ref::<vte4::Terminal>().cloned()
            .or_else(|| focus.ancestor(vte4::Terminal::static_type()).and_downcast());
        if term.is_some() { return term; }
    }
    let notebook = window.child().and_downcast::<gtk::Notebook>()?;
    let page = notebook.nth_page(notebook.current_page())?;
    shared.last_focused_in(&page).or_else(|| layout::terminals_in(&page).into_iter().next())
}

/// Run `action` on `term`.
pub fn run_action(shared: &Rc<Shared>, action: Action, term: &vte4::Terminal) {
    let notebook = || term.ancestor(gtk::Notebook::static_type()).and_downcast::<gtk::Notebook>();
    match action {
        Action::SplitHorizontal => layout::split(shared, term, gtk::Orientation::Vertical),
        Action::SplitVertical => layout::split(shared, term, gtk::Orientation::Horizontal),
        Action::ClosePane => layout::close_pane(shared, term),
        Action::NewTab => {
            let cwd = pane::working_directory(term);
            if let Some(nb) = notebook() { new_tab(shared, &nb, cwd.as_deref(), None); }
        }
        Action::NextTab => step_tab(notebook(), 1),
        Action::PrevTab => step_tab(notebook(), -1),
        Action::NewWindow => open_window(shared, pane::working_directory(term).as_deref(), None),
        Action::FocusNext => layout::focus_cycle(term, 1),
        Action::FocusPrev => layout::focus_cycle(term, -1),
        Action::FocusUp => layout::focus_direction(term, Direction::Up),
        Action::FocusDown => layout::focus_direction(term, Direction::Down),
        Action::FocusLeft => layout::focus_direction(term, Direction::Left),
        Action::FocusRight => layout::focus_direction(term, Direction::Right),
        Action::Copy => term.copy_clipboard_format(vte4::Format::Text),
        Action::Paste => term.paste_clipboard(),
        Action::SelectAll => term.select_all(),
        Action::ZoomIn => pane::zoom(term, Some(pane::zoom_factor(true))),
        Action::ZoomOut => pane::zoom(term, Some(pane::zoom_factor(false))),
        Action::ZoomReset => pane::zoom(term, None),
        Action::ResetClear => term.reset(true, true),
        Action::Export => export_contents(term),
        Action::Preferences => {
            if let Some(window) = term.root().and_downcast::<gtk::Window>() { crate::prefs::open(shared, &window); }
        }
    }
}

/// Switch `notebook` by `step` tabs, wrapping around.
fn step_tab(notebook: Option<gtk::Notebook>, step: i32) {
    let Some(notebook) = notebook else { return };
    let count = notebook.n_pages() as i32;
    if count < 2 { return; }
    let current = notebook.current_page().unwrap_or(0) as i32;
    notebook.set_current_page(Some((current + step).rem_euclid(count) as u32));
}

/// Ask for a file and write the whole text of `term` (scrollback included)
/// into it.
fn export_contents(term: &vte4::Terminal) {
    let Some(window) = term.root().and_downcast::<gtk::Window>() else { return };
    let stamp = glib::DateTime::now_local()
        .ok()
        .and_then(|d| d.format("%Y%m%d-%H%M%S").ok())
        .map_or_else(String::new, |s| format!("-{s}"));
    let dialog = gtk::FileDialog::builder()
        .title("Export Terminal Contents")
        .initial_name(format!("terminal{stamp}.txt"))
        .modal(true)
        .build();
    if let Some(cwd) = pane::working_directory(term) {
        dialog.set_initial_folder(Some(&gio::File::for_path(cwd)));
    }
    let term = term.clone();
    let parent = window.clone();
    dialog.save(Some(&window), None::<&gio::Cancellable>, move |result| {
        let Ok(file) = result else { return };
        if let Err(e) = write_contents(&term, &file) {
            let name = file.parse_name();
            opusdm_ipc::err_log!("opusdm-terminal", "cannot export to {name}: {e}");
            opusdm_ui::requester(&parent, "Export", &format!("Cannot write {name}"), Some(&e.to_string()), &[("OK", ())], |_| {});
        }
    });
}

/// Write the text of `term` into `file`, replacing it.
fn write_contents(term: &vte4::Terminal, file: &gio::File) -> Result<(), glib::Error> {
    let stream = file.replace(None, false, gio::FileCreateFlags::REPLACE_DESTINATION, None::<&gio::Cancellable>)?;
    term.write_contents_sync(&stream, vte4::WriteFlags::Default, None::<&gio::Cancellable>)?;
    stream.close(None::<&gio::Cancellable>)
}
