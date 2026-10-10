//! The Preferences dialog: theme, font, scrollback, cursor and hotkeys.
//! Saving applies the new options to every open terminal at once.

use crate::actions::Action;
use crate::config::{CursorShape, TerminalConfig, MAX_SCROLLBACK_LINES};
use crate::shared::Shared;
use crate::theme::user_themes_dir;
use gtk4 as gtk;
use gtk::{gdk, gio, glib, pango};
use gtk::prelude::*;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::rc::Rc;

/// Gap between the columns and rows of the option grids.
const GRID_SPACING: i32 = 8;

/// The widgets holding the edited values.
struct Form {
    theme: gtk::DropDown,
    theme_names: Vec<String>,
    font: gtk::FontDialogButton,
    scrollback: gtk::SpinButton,
    cursor_shape: gtk::DropDown,
    cursor_blink: gtk::CheckButton,
    audible_bell: gtk::CheckButton,
    bold_is_bright: gtk::CheckButton,
    hotkeys: Vec<(Action, gtk::Entry)>,
}

/// Open the Preferences dialog over `parent`.
pub fn open(shared: &Rc<Shared>, parent: &gtk::Window) {
    shared.reload_themes();
    let config = shared.config();
    let shell = opusdm_ui::DialogShell::new(parent, "Terminal Preferences").size(560, 640);
    let form = Rc::new(build_form(shared, &config, shell.content()));

    let themes_btn = shell.button("Themes Folder");
    themes_btn.set_tooltip_text(Some("Open the folder of the user themes (one JSON file per theme)"));
    themes_btn.connect_clicked(open_themes_folder);
    shell.button_spacer();
    shell.close_button("Cancel");
    let save = shell.button("Save");
    save.add_css_class("suggested-action");
    {
        let shared = shared.clone();
        let form = form.clone();
        let window = shell.window().clone();
        save.connect_clicked(move |_| {
            match read_form(&form, shared.config()) {
                Ok(config) => {
                    shared.update_config(config);
                    window.close();
                }
                Err(message) => opusdm_ui::requester(&window, "Preferences", &message, None, &[("OK", ())], |_| {}),
            }
        });
    }
    shell.present();
}

/// A bold section heading.
fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(text)));
    label.set_xalign(0.0);
    label.set_margin_top(6);
    label
}

/// A grid for label/value rows.
fn grid() -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(GRID_SPACING as u32);
    grid.set_row_spacing(GRID_SPACING as u32);
    grid
}

/// Add a `label` + `widget` row to `grid`.
fn row(grid: &gtk::Grid, index: i32, label: &str, widget: &impl IsA<gtk::Widget>) {
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    grid.attach(&l, 0, index, 1, 1);
    widget.set_hexpand(true);
    grid.attach(widget, 1, index, 1, 1);
}

/// The padded vertical box of one notebook page.
fn page_box() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, GRID_SPACING);
    page.set_margin_start(GRID_SPACING);
    page.set_margin_end(GRID_SPACING);
    page.set_margin_top(GRID_SPACING);
    page.set_margin_bottom(GRID_SPACING);
    page
}

/// `page` inside a vertical scroller, so a long page never grows the dialog.
fn scrolled(page: &gtk::Box) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(page)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build()
}

/// Build the form widgets into `content`, filled from `config`: a
/// notebook with the General page (appearance, scrolling) and the Hotkeys
/// page.
fn build_form(shared: &Shared, config: &TerminalConfig, content: &gtk::Box) -> Form {
    let general = page_box();
    let keys_page = page_box();
    general.append(&heading("Appearance"));
    let look = grid();
    let theme_names: Vec<String> = shared.themes().into_iter().map(|t| t.name).collect();
    let name_refs: Vec<&str> = theme_names.iter().map(String::as_str).collect();
    let theme = gtk::DropDown::from_strings(&name_refs);
    let current = theme_names.iter().position(|n| *n == shared.theme().name).unwrap_or(0);
    theme.set_selected(current as u32);
    row(&look, 0, "Theme", &theme);

    let font = gtk::FontDialogButton::new(Some(gtk::FontDialog::builder().title("Terminal Font").build()));
    font.set_font_desc(&pango::FontDescription::from_string(&config.font));
    row(&look, 1, "Font", &font);

    let shapes: Vec<&str> = CursorShape::ALL.iter().map(|s| s.label()).collect();
    let cursor_shape = gtk::DropDown::from_strings(&shapes);
    let shape_index = CursorShape::ALL.iter().position(|s| *s == config.cursor_shape).unwrap_or(0);
    cursor_shape.set_selected(shape_index as u32);
    row(&look, 2, "Cursor", &cursor_shape);
    general.append(&look);

    let cursor_blink = gtk::CheckButton::with_label("Blinking cursor");
    cursor_blink.set_active(config.cursor_blink);
    let bold_is_bright = gtk::CheckButton::with_label("Show bold text in bright colours");
    bold_is_bright.set_active(config.bold_is_bright);
    let audible_bell = gtk::CheckButton::with_label("Audible bell");
    audible_bell.set_active(config.audible_bell);
    general.append(&cursor_blink);
    general.append(&bold_is_bright);
    general.append(&audible_bell);

    general.append(&heading("Scrolling"));
    let scroll = grid();
    let scrollback = gtk::SpinButton::with_range(0.0, f64::from(MAX_SCROLLBACK_LINES), 100.0);
    scrollback.set_value(f64::from(config.scrollback_lines));
    scrollback.set_tooltip_text(Some("0 keeps every line (unlimited)"));
    row(&scroll, 0, "Scrollback lines (0 = unlimited)", &scrollback);
    general.append(&scroll);

    let hint = gtk::Label::new(Some("Grab... records a key combination; or type it in GTK syntax, e.g. <Ctrl><Shift>e. Leave empty to disable."));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("dim-label");
    keys_page.append(&hint);
    let keys = grid();
    let hotkeys: Vec<(Action, gtk::Entry)> = Action::ALL
        .into_iter()
        .enumerate()
        .map(|(i, action)| {
            let entry = gtk::Entry::new();
            entry.set_text(&config.hotkey(action));
            entry.set_placeholder_text(Some("(none)"));
            entry.set_hexpand(true);
            let grab = gtk::Button::with_label("Grab...");
            grab.set_tooltip_text(Some("Press the key combination to bind"));
            {
                let entry = entry.clone();
                grab.connect_clicked(move |b| grab_key(b, &entry));
            }
            let cell = gtk::Box::new(gtk::Orientation::Horizontal, GRID_SPACING);
            cell.append(&entry);
            cell.append(&grab);
            row(&keys, i as i32, action.label(), &cell);
            (action, entry)
        })
        .collect();
    keys_page.append(&keys);


    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);
    notebook.append_page(&scrolled(&general), Some(&gtk::Label::new(Some("General"))));
    notebook.append_page(&scrolled(&keys_page), Some(&gtk::Label::new(Some("Hotkeys"))));
    content.set_vexpand(true);
    content.append(&notebook);

    Form { theme, theme_names, font, scrollback, cursor_shape, cursor_blink, audible_bell, bold_is_bright, hotkeys }
}

/// The config described by `form`, starting from `config`. Fails with a
/// message naming the first hotkey GTK cannot parse.
fn read_form(form: &Form, mut config: TerminalConfig) -> Result<TerminalConfig, String> {
    if let Some(name) = form.theme_names.get(form.theme.selected() as usize) {
        config.theme = name.clone();
    }
    if let Some(desc) = form.font.font_desc() {
        config.font = desc.to_string();
    }
    config.scrollback_lines = form.scrollback.value_as_int().max(0) as u32;
    config.cursor_shape = CursorShape::ALL.get(form.cursor_shape.selected() as usize).copied().unwrap_or_default();
    config.cursor_blink = form.cursor_blink.is_active();
    config.audible_bell = form.audible_bell.is_active();
    config.bold_is_bright = form.bold_is_bright.is_active();
    config.hotkeys.clear();
    let mut used: Vec<(Action, gdk::Key, gdk::ModifierType)> = Vec::new();
    for (action, entry) in &form.hotkeys {
        let accel = entry.text().trim().to_string();
        if accel.is_empty() {
            config.hotkeys.insert(action.id().to_string(), accel);
            continue;
        }
        let Some((key, mods)) = gtk::accelerator_parse(&accel) else {
            return Err(format!("The hotkey \"{accel}\" of \"{}\" is not valid.", action.label()));
        };
        if let Some((other, _, _)) = used.iter().find(|(_, k, m)| *k == key && *m == mods) {
            return Err(format!("\"{}\" and \"{}\" use the same hotkey {accel}.", other.label(), action.label()));
        }
        used.push((*action, key, mods));
        // Only the changed hotkeys are stored, so new defaults still reach
        // the actions the user never touched. Compared by key, not text:
        // a grabbed "<Shift><Control>e" equals the default "<Ctrl><Shift>e".
        if gtk::accelerator_parse(action.default_hotkey()) != Some((key, mods)) {
            config.hotkeys.insert(action.id().to_string(), accel);
        }
    }
    Ok(config)
}

/// Ask for a key combination in a small modal window and write it into
/// `entry` in GTK accelerator syntax. Escape cancels (so Escape itself
/// cannot be bound), as in the hotkey editor of the Lister.
fn grab_key(button: &gtk::Button, entry: &gtk::Entry) {
    let Some(parent) = button.root().and_downcast::<gtk::Window>() else { return };
    let shell = opusdm_ui::DialogShell::new(&parent, "Press a key combination").size(340, 120).resizable(false);
    shell.content().append(&gtk::Label::new(Some("Press the key combination now.\nLetters and digits need Ctrl, Alt or Super.\nEscape cancels.")));
    shell.close_button("Cancel");
    let window = shell.window().clone();

    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let entry = entry.clone();
        let window = window.clone();
        controller.connect_key_pressed(move |_, key, _, state| {
            if key == gdk::Key::Escape {
                window.close();
                return glib::Propagation::Stop;
            }
            let Some(accel) = grabbed_accel(key, state) else { return glib::Propagation::Stop };
            entry.set_text(&accel);
            window.close();
            glib::Propagation::Stop
        });
    }
    window.add_controller(controller);
    shell.present();
}

/// The accelerator text for a key press. `None` for a modifier-only press
/// and for a printable key without Ctrl, Alt or Super, which would steal
/// that character from the shell. Lock keys (Caps Lock, Num Lock) are
/// dropped from the modifiers.
fn grabbed_accel(key: gdk::Key, state: gdk::ModifierType) -> Option<String> {
    let mods = state & gtk::accelerator_get_default_mod_mask();
    if !gtk::accelerator_valid(key, mods) { return None; }
    let chord = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK;
    let printable = key.to_unicode().is_some_and(|c| !c.is_control());
    if printable && !mods.intersects(chord) { return None; }
    Some(gtk::accelerator_name(key.to_lower(), mods).to_string())
}

/// Create the user themes folder if needed and open it in a Lister. Without
/// a running hub (outside an OpusDM session) the system's default folder
/// application opens it instead.
fn open_themes_folder(button: &gtk::Button) {
    let dir = user_themes_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        opusdm_ipc::err_log!("opusdm-terminal", "cannot create {}: {e}", dir.display());
        return;
    }
    let Err(e) = open_in_lister(&dir) else { return };
    opusdm_ipc::err_log!("opusdm-terminal", "cannot reach the hub to open a Lister: {e}; using the default application");
    let window = button.root().and_downcast::<gtk::Window>();
    gtk::FileLauncher::new(Some(&gio::File::for_path(&dir))).launch(window.as_ref(), None::<&gio::Cancellable>, |result| {
        if let Err(e) = result {
            opusdm_ipc::err_log!("opusdm-terminal", "cannot open the themes folder: {e}");
        }
    });
}

/// Ask the running hub to open a Lister on `dir`.
fn open_in_lister(dir: &Path) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(opusdm_ipc::sockpath::hub_sock_path())?;
    opusdm_ipc::frame::send(&mut stream, &opusdm_ipc::ToHub::SpawnLister { start_path: Some(dir.to_path_buf()) })
}
