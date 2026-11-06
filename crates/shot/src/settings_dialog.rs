//! The Settings window of the editor: the defaults for the folder, the file
//! name, the format and quality, the capture delay and the automatic copy.

use crate::config::{self, ShotConfig};
use crate::export::{self, Format};
use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ui::LabelWidth;
use std::path::PathBuf;
use std::rc::Rc;

/// Width of the labels in front of the fields, in characters.
const LABEL_CHARS: i32 = 14;

/// Space between the rows and around the content.
const SPACING: i32 = 8;
const MARGIN: i32 = 16;

/// Width of the folder and name entries, in characters.
const ENTRY_CHARS: i32 = 36;

/// Width of the quality slider.
const QUALITY_WIDTH: i32 = 200;

/// Minimum width of a button, like the OpusDM requesters.
const BUTTON_MIN_WIDTH: i32 = 90;

/// The widgets the handlers read.
struct Fields {
    dir: gtk::Entry,
    name: gtk::Entry,
    format: gtk::DropDown,
    quality: gtk::Scale,
    quality_row: gtk::Box,
    delay: gtk::SpinButton,
    auto_copy: gtk::CheckButton,
}

/// Show the modal Settings window over `parent`, filled with `current`.
/// `on_save` runs once with the new settings when the user presses Save;
/// Cancel, Esc and closing the window do nothing.
pub fn show(parent: &impl IsA<gtk::Window>, current: &ShotConfig, on_save: impl Fn(ShotConfig) + 'static) {
    let fields = Rc::new(build_fields(current));
    let dialog = gtk::Window::builder()
        .title("Screenshot settings")
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .build();
    dialog.add_css_class(opusdm_ui::REQUESTER_CLASS);

    let body = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
    body.set_margin_top(MARGIN);
    body.set_margin_bottom(MARGIN);
    body.set_margin_start(MARGIN);
    body.set_margin_end(MARGIN);

    let browse = gtk::Button::with_label("Browse...");
    let dir_row = opusdm_ui::labeled_row("Save folder", LabelWidth::Chars(LABEL_CHARS), SPACING, &fields.dir);
    dir_row.append(&browse);
    body.append(&dir_row);
    body.append(&opusdm_ui::labeled_row("File name", LabelWidth::Chars(LABEL_CHARS), SPACING, &fields.name));
    body.append(&opusdm_ui::labeled_row("Format", LabelWidth::Chars(LABEL_CHARS), SPACING, &fields.format));
    body.append(&fields.quality_row);
    let delay_row = opusdm_ui::labeled_row("Delay", LabelWidth::Chars(LABEL_CHARS), SPACING, &fields.delay);
    delay_row.append(&gtk::Label::new(Some("seconds")));
    body.append(&delay_row);
    body.append(&opusdm_ui::labeled_row("", LabelWidth::Chars(LABEL_CHARS), SPACING, &fields.auto_copy));

    let save = gtk::Button::with_label("Save");
    save.set_size_request(BUTTON_MIN_WIDTH, -1);
    save.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label("Cancel");
    cancel.set_size_request(BUTTON_MIN_WIDTH, -1);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
    buttons.set_margin_top(SPACING);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    buttons.append(&save);
    buttons.append(&spacer);
    buttons.append(&cancel);
    body.append(&buttons);
    dialog.set_child(Some(&body));
    dialog.set_default_widget(Some(&save));

    {
        let f = fields.clone();
        fields.format.connect_selected_notify(move |_| refresh_quality(&f));
    }
    {
        let save = save.clone();
        let f = fields.clone();
        let check = move || save.set_sensitive(!f.dir.text().trim().is_empty() && !f.name.text().trim().is_empty());
        check();
        fields.dir.connect_changed({ let check = check.clone(); move |_| check() });
        fields.name.connect_changed(move |_| check());
    }
    {
        let (fields, dialog) = (fields.clone(), dialog.clone());
        browse.connect_clicked(move |_| browse_folder(&dialog, &fields.dir));
    }
    {
        let (fields, dialog) = (fields.clone(), dialog.clone());
        save.connect_clicked(move |_| {
            let settings = read_settings(&fields);
            // destroy() drops the widget tree and so the closures holding
            // `on_save`, before the caller works.
            dialog.destroy();
            on_save(settings);
        });
    }
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.destroy());
    }
    let keys = gtk::EventControllerKey::new();
    {
        let cancel = cancel.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != gtk::gdk::Key::Escape { return gtk::glib::Propagation::Proceed; }
            cancel.emit_clicked();
            gtk::glib::Propagation::Stop
        });
    }
    dialog.add_controller(keys);
    dialog.present();
}

/// Create the fields with the values of `current`.
fn build_fields(current: &ShotConfig) -> Fields {
    let dir = gtk::Entry::new();
    dir.set_text(&current.dir.to_string_lossy());
    dir.set_width_chars(ENTRY_CHARS);
    dir.set_hexpand(true);
    dir.set_tooltip_text(Some("Folder of Quick Save and first folder of the Save file dialog; created when missing"));

    let name = gtk::Entry::new();
    name.set_text(&current.name);
    name.set_width_chars(ENTRY_CHARS);
    name.set_hexpand(true);
    name.set_activates_default(true);
    name.set_tooltip_text(Some("File name without extension. YYYYMMDD becomes the date and HHMMSS the time"));

    let format = gtk::DropDown::from_strings(&Format::ALL.map(Format::label));
    format.set_selected(Format::ALL.iter().position(|f| *f == current.format_enum()).unwrap_or(0) as u32);

    let quality = gtk::Scale::with_range(gtk::Orientation::Horizontal, export::MIN_QUALITY as f64, export::MAX_QUALITY as f64, 1.0);
    quality.set_value(current.quality as f64);
    quality.set_draw_value(true);
    quality.set_width_request(QUALITY_WIDTH);
    let quality_row = opusdm_ui::labeled_row("Quality", LabelWidth::Chars(LABEL_CHARS), SPACING, &quality);

    let delay = gtk::SpinButton::with_range(0.0, config::MAX_DELAY_SECS as f64, 1.0);
    delay.set_value(current.delay_secs as f64);
    delay.set_tooltip_text(Some("Seconds to wait before the screen is captured, from the next launch"));

    let auto_copy = gtk::CheckButton::with_label("Auto copy to the clipboard");
    auto_copy.set_active(current.auto_copy);
    auto_copy.set_tooltip_text(Some("Copy the screenshot to the clipboard when the editor opens and again when it is saved"));

    let fields = Fields { dir, name, format, quality, quality_row, delay, auto_copy };
    refresh_quality(&fields);
    fields
}

/// Show the quality slider only for the formats that have a quality.
fn refresh_quality(fields: &Fields) {
    fields.quality_row.set_visible(selected_format(fields).has_quality());
}

/// The format picked in the drop-down.
fn selected_format(fields: &Fields) -> Format {
    Format::ALL.get(fields.format.selected() as usize).copied().unwrap_or(Format::Png)
}

/// Let the user pick the folder with the file dialog of the system.
fn browse_folder(parent: &gtk::Window, entry: &gtk::Entry) {
    let dialog = gtk::FileDialog::builder().title("Save folder").modal(true).build();
    let current = PathBuf::from(entry.text().as_str());
    if current.is_dir() { dialog.set_initial_folder(Some(&gio::File::for_path(current))); }
    let entry = entry.clone();
    dialog.select_folder(Some(parent), gio::Cancellable::NONE, move |result| match result {
        Ok(folder) => {
            if let Some(path) = folder.path() { entry.set_text(&path.to_string_lossy()); }
        }
        Err(e) if e.matches(gtk::DialogError::Dismissed) => {}
        Err(e) => opusdm_ipc::err_log!("opusdm-shot", "folder dialog failed: {e}"),
    });
}

/// The values of the fields.
fn read_settings(fields: &Fields) -> ShotConfig {
    fields.delay.update();
    ShotConfig {
        delay_secs: fields.delay.value_as_int().max(0) as u32,
        dir: expand_home(fields.dir.text().trim()),
        name: fields.name.text().trim().to_string(),
        format: selected_format(fields).extension().to_string(),
        quality: fields.quality.value().round() as u8,
        auto_copy: fields.auto_copy.is_active(),
    }
}

/// `text` as a path, with a leading `~/` replaced by the home folder.
fn expand_home(text: &str) -> PathBuf {
    match (text.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GTK may be used from one test thread only, so every widget check is
    /// in this one test.
    #[test]
    fn the_fields_read_back_and_the_quality_follows_the_format() {
        if gtk::init().is_err() { return; }
        let current = ShotConfig { delay_secs: 4, dir: "/tmp/x".into(), name: "n".into(), format: "jpg".into(), quality: 70, auto_copy: true };
        let fields = build_fields(&current);
        assert!(fields.quality_row.is_visible(), "JPG has a quality");
        assert_eq!(read_settings(&fields), current);
        fields.format.set_selected(0);
        assert!(!fields.quality_row.is_visible(), "PNG has no quality");
        assert_eq!(read_settings(&fields).format, "png");
    }

    #[test]
    fn a_tilde_folder_is_expanded() {
        assert_eq!(expand_home("/a/b"), PathBuf::from("/a/b"));
        if let Some(home) = dirs::home_dir() { assert_eq!(expand_home("~/x"), home.join("x")); }
    }
}
