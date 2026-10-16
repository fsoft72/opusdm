//! The Preferences dialog: General (font, cursor, scrollback), Theme (pick
//! and edit the colours, export a new theme) and Hotkeys. Changes are
//! previewed live; Save keeps them, Cancel puts the saved ones back.

use crate::actions::Action;
use crate::config::{CursorShape, TerminalConfig, MAX_SCROLLBACK_LINES};
use crate::shared::Shared;
use crate::theme::{user_theme_path, user_themes_dir};
use crate::theme_editor::ThemeEditor;
use gtk4 as gtk;
use gtk::{gdk, gio, glib, pango};
use gtk::prelude::*;
use std::path::Path;
use std::rc::Rc;

/// Gap between the columns and rows of the option grids.
const GRID_SPACING: i32 = 8;
/// Room above a section heading.
const HEADING_MARGIN_TOP: i32 = 6;

/// The widgets holding the edited values.
struct Form {
    theme: Rc<ThemeEditor>,
    reset_theme: gtk::Button,
    export_theme: gtk::Button,
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
    connect_preview(shared, &form);
    connect_theme_buttons(shared, &form, shell.window());
    // Closing without Save (Cancel, Escape, the window button) drops the
    // previewed options; after Save this re-applies what was just saved.
    {
        let shared = shared.clone();
        shell.window().connect_close_request(move |_| {
            shared.preview(&shared.config());
            glib::Propagation::Proceed
        });
    }

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
            if !form.theme.is_dirty() { return save_and_close(&shared, &form, &window); }
            // The edited colours live only in the dialog: offer to export
            // them before they are lost.
            let (shared, form, parent) = (shared.clone(), form.clone(), window.clone());
            opusdm_ui::requester(
                &window,
                "Preferences",
                "The theme colours were changed but not exported as a theme.",
                Some("Export them as a new theme, or discard the changes and save the other options."),
                &[("Export...", true), ("Discard", false)],
                move |export| {
                    if !export {
                        form.theme.reset();
                        return save_and_close(&shared, &form, &parent);
                    }
                    let (shared2, form2, parent2) = (shared.clone(), form.clone(), parent.clone());
                    export_theme(&shared, &form.theme, &parent, move || save_and_close(&shared2, &form2, &parent2));
                },
            );
        });
    }
    shell.present();
}

/// Store the options of `form`, apply them and close `window`. An invalid
/// hotkey or a failed save is reported and keeps the dialog open.
fn save_and_close(shared: &Shared, form: &Form, window: &gtk::Window) {
    match read_form(form, shared.config()).and_then(|config| shared.update_config(config)) {
        Ok(()) => window.close(),
        Err(message) => opusdm_ui::requester(window, "Preferences", &message, None, &[("OK", ())], |_| {}),
    }
}

/// Wire the Reset and Export buttons of the Theme tab.
fn connect_theme_buttons(shared: &Rc<Shared>, form: &Rc<Form>, window: &gtk::Window) {
    {
        let editor = form.theme.clone();
        form.reset_theme.connect_clicked(move |_| editor.reset());
    }
    let shared = shared.clone();
    let editor = form.theme.clone();
    let window = window.clone();
    form.export_theme.connect_clicked(move |_| export_theme(&shared, &editor, &window, || {}));
}

/// Ask a name and export the edited colours of `editor` as a user theme,
/// asking before an existing theme file is replaced. `on_done` runs after a
/// successful export.
fn export_theme(shared: &Rc<Shared>, editor: &Rc<ThemeEditor>, window: &gtk::Window, on_done: impl Fn() + 'static) {
    let initial = format!("{} (custom)", editor.selected_name());
    let (shared, editor, parent) = (shared.clone(), editor.clone(), window.clone());
    let on_done = Rc::new(on_done);
    opusdm_ui::requester_entry(window, "Name of the new theme", &initial, &[("Export", true), ("Cancel", false)], move |ok, name| {
        let name = name.trim().to_string();
        if !ok || name.is_empty() { return; }
        let path = user_theme_path(&user_themes_dir(), &name);
        let write = {
            let (shared, editor, parent, on_done) = (shared.clone(), editor.clone(), parent.clone(), on_done.clone());
            move || match editor.export(&name) {
                Ok(path) => {
                    opusdm_ipc::err_log!("opusdm-terminal", "theme exported to {}", path.display());
                    shared.reload_themes();
                    on_done();
                }
                Err(message) => opusdm_ui::requester(&parent, "Export Theme", &message, None, &[("OK", ())], |_| {}),
            }
        };
        if !path.exists() { return write(); }
        opusdm_ui::requester(
            &parent,
            "Export Theme",
            &format!("A theme file {} already exists.", path.display()),
            Some("Replace it with the edited colours?"),
            &[("Replace", true), ("Cancel", false)],
            move |replace| if replace { write() },
        );
    });
}

/// A section heading with some room above it.
fn heading(text: &str) -> gtk::Label {
    let label = opusdm_ui::section_heading(text);
    label.set_margin_top(HEADING_MARGIN_TOP);
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
/// notebook with the General page (appearance, scrolling), the Theme page
/// and the Hotkeys page.
fn build_form(shared: &Shared, config: &TerminalConfig, content: &gtk::Box) -> Form {
    let general = page_box();
    let keys_page = page_box();
    general.append(&heading("Appearance"));
    let look = grid();
    let font = gtk::FontDialogButton::new(Some(gtk::FontDialog::builder().title("Terminal Font").build()));
    font.set_font_desc(&pango::FontDescription::from_string(&config.font));
    row(&look, 0, "Font", &font);

    let shapes: Vec<&str> = CursorShape::ALL.iter().map(|s| s.label()).collect();
    let cursor_shape = gtk::DropDown::from_strings(&shapes);
    let shape_index = CursorShape::ALL.iter().position(|s| *s == config.cursor_shape).unwrap_or(0);
    cursor_shape.set_selected(shape_index as u32);
    row(&look, 1, "Cursor", &cursor_shape);
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

    let theme = ThemeEditor::new(shared.themes(), &shared.theme().name);
    let theme_page = page_box();
    theme_page.append(&theme.widget);
    let reset_theme = gtk::Button::with_label("Reset");
    reset_theme.set_tooltip_text(Some("Put the colours of the selected theme back"));
    let export_theme = gtk::Button::with_label("Export as New Theme...");
    export_theme.set_tooltip_text(Some("Save these colours as a new theme in the themes folder"));
    let theme_buttons = gtk::Box::new(gtk::Orientation::Horizontal, GRID_SPACING);
    theme_buttons.set_halign(gtk::Align::End);
    theme_buttons.set_margin_top(GRID_SPACING);
    theme_buttons.append(&reset_theme);
    theme_buttons.append(&export_theme);
    theme_page.append(&theme_buttons);

    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);
    notebook.append_page(&scrolled(&general), Some(&gtk::Label::new(Some("General"))));
    notebook.append_page(&scrolled(&theme_page), Some(&gtk::Label::new(Some("Theme"))));
    notebook.append_page(&scrolled(&keys_page), Some(&gtk::Label::new(Some("Hotkeys"))));
    content.set_vexpand(true);
    content.append(&notebook);

    Form { theme, reset_theme, export_theme, font, scrollback, cursor_shape, cursor_blink, audible_bell, bold_is_bright, hotkeys }
}

/// The config described by `form`, starting from `config`. Fails with a
/// message naming the first hotkey GTK cannot parse, or two actions on the
/// same keys.
fn read_form(form: &Form, config: TerminalConfig) -> Result<TerminalConfig, String> {
    let mut config = read_general(form, config);
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

/// `config` with the General options (appearance, scrolling) of `form`.
fn read_general(form: &Form, mut config: TerminalConfig) -> TerminalConfig {
    config.theme = form.theme.selected_name();
    if let Some(desc) = form.font.font_desc() {
        config.font = desc.to_string();
    }
    config.scrollback_lines = form.scrollback.value_as_int().max(0) as u32;
    config.cursor_shape = CursorShape::ALL.get(form.cursor_shape.selected() as usize).copied().unwrap_or_default();
    config.cursor_blink = form.cursor_blink.is_active();
    config.audible_bell = form.audible_bell.is_active();
    config.bold_is_bright = form.bold_is_bright.is_active();
    config
}

/// Apply the General options of `form` to every open terminal as soon as
/// one of them changes, so a theme or a font can be tried before saving.
fn connect_preview(shared: &Rc<Shared>, form: &Rc<Form>) {
    let refresh = {
        let shared = shared.clone();
        let form = Rc::downgrade(form);
        Rc::new(move || {
            let Some(form) = form.upgrade() else { return };
            let config = read_general(&form, shared.config());
            shared.preview_with(&config, &form.theme.edited_theme(&config.theme));
        })
    };
    let r = refresh.clone();
    form.theme.connect_changed(move || r());
    let r = refresh.clone();
    form.font.connect_font_desc_notify(move |_| r());
    let r = refresh.clone();
    form.cursor_shape.connect_selected_notify(move |_| r());
    let r = refresh.clone();
    form.scrollback.connect_value_changed(move |_| r());
    for check in [&form.cursor_blink, &form.audible_bell, &form.bold_is_bright] {
        let r = refresh.clone();
        check.connect_toggled(move |_| r());
    }
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
    let msg = opusdm_ipc::ToHub::SpawnLister { start_path: Some(dir.to_path_buf()) };
    opusdm_ipc::sockpath::send_oneshot(&opusdm_ipc::sockpath::hub_sock_path(), &[msg])
}
