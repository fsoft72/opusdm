//! The Run window: an input box with a list of suggestions under it.

use crate::launch;
use crate::search::{self, Entry, Kind, Suggestion};
use gtk4 as gtk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::PanelPosition;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Width of the window in pixels.
const WINDOW_WIDTH: i32 = 600;

/// Height of the hub panel strip, which the window sits under when the panel
/// is at the top (the same value the panel crate uses).
const PANEL_HEIGHT: i32 = 40;

/// Padding around the window content.
const WINDOW_PADDING: i32 = 8;

/// Width in characters of the kind label column ("Applications").
const KIND_LABEL_CHARS: i32 = 12;

/// Icon size of a suggestion row.
const ROW_ICON_SIZE: i32 = 24;

/// CSS class of the window.
const WINDOW_CLASS: &str = "opusdm-run";

/// Window-specific style, added to the shared OpusDM theme.
pub const RUN_CSS: &str = "window.opusdm-run { border: 1px solid alpha(currentColor, 0.3); } \
     .run-input { font-size: 120%; } \
     .run-kind, .run-comment { opacity: 0.6; } \
     .run-error { color: #e5484d; }";

/// Everything the window's callbacks share.
struct State {
    window: gtk::ApplicationWindow,
    input: gtk::Entry,
    list: gtk::ListBox,
    error: gtk::Label,
    entries: Vec<Entry>,
    suggestions: RefCell<Vec<Suggestion>>,
}

/// Top left corner of the window on `monitor` (x, y, width, height): centered
/// horizontally, right under the panel when it is at the top.
pub fn top_left(monitor: (i32, i32, i32, i32), width: i32, panel: PanelPosition) -> (i32, i32) {
    let (x, y, w, _) = monitor;
    let below_panel = if panel == PanelPosition::Top { PANEL_HEIGHT } else { 0 };
    (x + (w - width).max(0) / 2, y + below_panel)
}

/// Open the Run window for `app`, or close it when it is already open (a
/// second launch of the hotkey toggles it).
pub fn open(app: &gtk::Application, entries: Vec<Entry>, panel: PanelPosition) {
    if let Some(window) = app.windows().first() {
        window.close();
        return;
    }
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Run")
        .decorated(false)
        .resizable(false)
        .default_width(WINDOW_WIDTH)
        .build();
    window.add_css_class(WINDOW_CLASS);

    let input = gtk::Entry::new();
    input.add_css_class("run-input");
    input.set_placeholder_text(Some("Run a program or a command"));
    input.set_primary_icon_name(Some("system-search-symbolic"));
    input.set_secondary_icon_name(Some("edit-clear-symbolic"));
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_visible(false);
    let error = gtk::Label::new(None);
    error.add_css_class("run-error");
    error.set_xalign(0.0);
    error.set_wrap(true);
    error.set_visible(false);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_margin_start(WINDOW_PADDING);
    root.set_margin_end(WINDOW_PADDING);
    root.set_margin_top(WINDOW_PADDING);
    root.set_margin_bottom(WINDOW_PADDING);
    root.append(&input);
    root.append(&error);
    root.append(&list);
    window.set_child(Some(&root));

    let state = Rc::new(State { window: window.clone(), input: input.clone(), list: list.clone(), error, entries, suggestions: RefCell::new(Vec::new()) });
    wire(&state);
    place_on_map(&window, panel);
    window.present();
    input.grab_focus();
}

/// Connect the input, the list, the keys and the focus handling.
fn wire(state: &Rc<State>) {
    {
        let state = state.clone();
        state.input.clone().connect_changed(move |_| refresh(&state));
    }
    {
        let state = state.clone();
        state.input.clone().connect_activate(move |_| {
            let index = state.list.selected_row().map(|row| row.index() as usize);
            run(&state, index);
        });
    }
    state.input.connect_icon_press(|input, position| {
        if position == gtk::EntryIconPosition::Secondary {
            input.set_text("");
        }
    });
    {
        let state = state.clone();
        state.list.clone().connect_row_activated(move |_, row| run(&state, Some(row.index() as usize)));
    }

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let state = state.clone();
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gtk::gdk::Key::Escape => {
                state.window.close();
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Down => {
                move_selection(&state, 1);
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Up => {
                move_selection(&state, -1);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
    }
    state.window.add_controller(keys);

    // Losing the focus closes the window, but only once it has had it: the
    // window manager focuses a new window a moment after it is shown.
    let was_active = Cell::new(false);
    state.window.connect_is_active_notify(move |window| {
        if window.is_active() {
            was_active.set(true);
        } else if was_active.get() {
            window.close();
        }
    });
}

/// Move the list selection by `step` rows, staying inside the list.
fn move_selection(state: &State, step: i32) {
    let count = state.suggestions.borrow().len() as i32;
    if count == 0 {
        return;
    }
    let current = state.list.selected_row().map(|row| row.index()).unwrap_or(-1);
    let next = (current + step).clamp(0, count - 1);
    if let Some(row) = state.list.row_at_index(next) {
        state.list.select_row(Some(&row));
    }
}

/// Recompute the suggestions for the typed text and repaint the list, with
/// the first row selected. Clears the last error.
fn refresh(state: &Rc<State>) {
    state.error.set_visible(false);
    while let Some(row) = state.list.first_child() {
        state.list.remove(&row);
    }
    let found = search::suggest(&state.entries, &state.input.text(), search::MAX_SUGGESTIONS);
    let mut previous: Option<Kind> = None;
    for suggestion in &found {
        let entry = &state.entries[suggestion.entry];
        state.list.append(&build_row(entry, previous != Some(entry.kind)));
        previous = Some(entry.kind);
    }
    state.list.set_visible(!found.is_empty());
    if let Some(row) = state.list.row_at_index(0) {
        state.list.select_row(Some(&row));
    }
    *state.suggestions.borrow_mut() = found;
}

/// One suggestion row: the kind label (only on the first row of a run of the
/// same kind), the icon, the name and the dimmed comment.
fn build_row(entry: &Entry, show_kind: bool) -> gtk::ListBoxRow {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.set_margin_top(4);
    line.set_margin_bottom(4);
    let kind = gtk::Label::new(Some(if !show_kind { "" } else if entry.kind == Kind::App { "Applications" } else { "Commands" }));
    kind.add_css_class("run-kind");
    kind.set_width_chars(KIND_LABEL_CHARS);
    kind.set_xalign(1.0);
    line.append(&kind);

    let fallback = if entry.kind == Kind::App { "application-x-executable" } else { "utilities-terminal" };
    let image = match entry.icon.as_deref().and_then(|text| gio::Icon::for_string(text).ok()) {
        Some(icon) => gtk::Image::from_gicon(&icon),
        None => gtk::Image::from_icon_name(fallback),
    };
    image.set_pixel_size(ROW_ICON_SIZE);
    line.append(&image);
    let name = gtk::Label::new(Some(&entry.name));
    name.set_xalign(0.0);
    line.append(&name);
    let comment = gtk::Label::new(Some(&entry.comment));
    comment.add_css_class("run-comment");
    comment.set_xalign(0.0);
    comment.set_hexpand(true);
    comment.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.append(&comment);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&line));
    row
}

/// Run suggestion `index`, or the typed text when there is none. Success
/// closes the window; a failure is shown under the input.
fn run(state: &Rc<State>, index: Option<usize>) {
    let chosen = index.and_then(|i| state.suggestions.borrow().get(i).cloned());
    let result = match chosen {
        Some(Suggestion { entry, args }) => {
            let entry = &state.entries[entry];
            match entry.kind {
                Kind::App => launch::launch_app(&entry.id),
                Kind::Command => launch::command_argv(&entry.id, &args).and_then(|argv| launch::spawn_detached(&argv)),
            }
        }
        None if state.input.text().trim().is_empty() => return,
        None => launch::parse(&state.input.text()).and_then(|argv| launch::spawn_detached(&argv)),
    };
    match result {
        Ok(()) => state.window.close(),
        Err(message) => {
            state.error.set_text(&message);
            state.error.set_visible(true);
        }
    }
}

/// Fetch the X11 window id backing `window`, if it runs under X11.
fn x11_window_id(window: &gtk::ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

/// Put the window at the top of the primary monitor once it is mapped, and
/// keep it above the others. The window manager places a window after map and
/// drops early state messages, so this is repeated for a second.
fn place_on_map(window: &gtk::ApplicationWindow, panel: PanelPosition) {
    window.connect_map(move |window| {
        let window = window.clone();
        opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || {
            let Some(xid) = x11_window_id(&window) else { return };
            let monitor = gtk::gdk::Display::default()
                .as_ref()
                .map(opusdm_ui::list_monitors)
                .and_then(|rects| rects.first().copied())
                .unwrap_or((0, 0, 1280, 800));
            let (x, y) = top_left(monitor, WINDOW_WIDTH, panel);
            let height = window.height().max(1) as u32;
            let placed = opusdm_ui::with_x11(|x11| {
                x11.pin_geometry(xid, x, y, WINDOW_WIDTH as u32, height)?;
                x11.add_wm_states(xid, &["_NET_WM_STATE_ABOVE", "_NET_WM_STATE_SKIP_TASKBAR"])
            });
            if let Err(e) = placed {
                opusdm_ipc::err_log!("opusdm-run", "cannot place the Run window: {e}");
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_centered_under_a_top_panel() {
        assert_eq!(top_left((0, 0, 1920, 1080), 600, PanelPosition::Top), (660, PANEL_HEIGHT));
        assert_eq!(top_left((1920, 100, 1280, 800), 600, PanelPosition::Top), (1920 + 340, 100 + PANEL_HEIGHT));
    }

    #[test]
    fn the_window_sits_at_the_top_edge_with_a_bottom_panel() {
        assert_eq!(top_left((0, 0, 1920, 1080), 600, PanelPosition::Bottom), (660, 0));
    }

    #[test]
    fn a_monitor_narrower_than_the_window_keeps_the_left_edge() {
        assert_eq!(top_left((0, 0, 400, 300), 600, PanelPosition::Bottom), (0, 0));
    }
}
