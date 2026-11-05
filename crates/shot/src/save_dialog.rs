//! The Save requester of the editor: file name, format (with quality and
//! lossless switch) and the size of the saved image. Its Save button hands the
//! choice to the caller, which then opens the file dialog of the system.

use crate::export::{self, Format, Options};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ui::LabelWidth;
use std::cell::Cell;
use std::rc::Rc;

/// Width of the labels in front of the fields, in characters.
const LABEL_CHARS: i32 = 10;

/// Space between the rows and around the content.
const SPACING: i32 = 8;
const MARGIN: i32 = 16;

/// Width of the file name entry, in characters.
const NAME_CHARS: i32 = 30;

/// Width of the quality slider.
const QUALITY_WIDTH: i32 = 200;

/// Minimum width of a button, like the OpusDM requesters.
const BUTTON_MIN_WIDTH: i32 = 90;

/// What the user confirmed in the requester.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveChoice {
    /// File name proposed to the file dialog, without the extension.
    pub stem: String,
    pub options: Options,
    /// Width and height in pixels of the saved image.
    pub size: (u32, u32),
}

/// The widgets the handlers read.
struct Fields {
    name: gtk::Entry,
    format: gtk::DropDown,
    quality: gtk::Scale,
    quality_row: gtk::Box,
    lossless: gtk::CheckButton,
    lossless_row: gtk::Box,
    width: gtk::SpinButton,
    height: gtk::SpinButton,
    /// The image size the width and height keep the aspect ratio of.
    image_size: (u32, u32),
    /// Set while the code moves one size field, so its handler ignores it.
    syncing: Cell<bool>,
}

/// Show the modal Save requester over `parent`, filled with `stem`, `options`
/// and the size of the image to save (`image_size`, before any scaling).
/// `on_save` runs once with the choice when the user presses Save; Cancel, Esc
/// and closing the window do nothing.
pub fn show(parent: &impl IsA<gtk::Window>, stem: &str, options: Options, image_size: (u32, u32), on_save: impl Fn(SaveChoice) + 'static) {
    let fields = Rc::new(build_fields(stem, options, image_size));
    let dialog = gtk::Window::builder()
        .title("Save screenshot")
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
    body.append(&opusdm_ui::labeled_row("File name", LabelWidth::Chars(LABEL_CHARS), SPACING, &fields.name));
    body.append(&opusdm_ui::labeled_row("Format", LabelWidth::Chars(LABEL_CHARS), SPACING, &fields.format));
    body.append(&fields.quality_row);
    body.append(&fields.lossless_row);
    body.append(&opusdm_ui::section_heading("Size"));
    body.append(&size_row("Width", &fields.width));
    body.append(&size_row("Height", &fields.height));

    let save = bold_button("Save");
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

    wire_fields(&fields, &save);
    {
        let (fields, dialog) = (fields.clone(), dialog.clone());
        save.connect_clicked(move |_| {
            let choice = read_choice(&fields);
            // destroy() drops the widget tree and so the closures holding
            // `on_save`, before the save work starts.
            dialog.destroy();
            on_save(choice);
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
    fields.name.grab_focus();
}

/// Create the fields with their first values.
fn build_fields(stem: &str, options: Options, image_size: (u32, u32)) -> Fields {
    let name = gtk::Entry::new();
    name.set_text(stem);
    name.set_width_chars(NAME_CHARS);
    name.set_hexpand(true);
    name.set_activates_default(true);

    let format = gtk::DropDown::from_strings(&Format::ALL.map(Format::label));
    format.set_selected(Format::ALL.iter().position(|f| *f == options.format).unwrap_or(0) as u32);

    let quality = gtk::Scale::with_range(gtk::Orientation::Horizontal, export::MIN_QUALITY as f64, export::MAX_QUALITY as f64, 1.0);
    quality.set_value(options.quality as f64);
    quality.set_draw_value(true);
    quality.set_width_request(QUALITY_WIDTH);
    let quality_row = opusdm_ui::labeled_row("Quality", LabelWidth::Chars(LABEL_CHARS), SPACING, &quality);

    let lossless = gtk::CheckButton::with_label("Lossless");
    lossless.set_active(options.lossless);
    let lossless_row = opusdm_ui::labeled_row("", LabelWidth::Chars(LABEL_CHARS), SPACING, &lossless);

    let (iw, ih) = (image_size.0.max(1), image_size.1.max(1));
    let width = size_spin(iw);
    let height = size_spin(ih);
    width.set_tooltip_text(Some("Width in pixels of the saved image; the height follows to keep the aspect ratio"));
    height.set_tooltip_text(Some("Height in pixels of the saved image; the width follows to keep the aspect ratio"));

    let fields = Fields { name, format, quality, quality_row, lossless, lossless_row, width, height, image_size: (iw, ih), syncing: Cell::new(false) };
    refresh_options(&fields);
    fields
}

/// A size field from one pixel to the largest scale of `size`, set to `size`.
fn size_spin(size: u32) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(1.0, (size * export::MAX_SCALE_PERCENT / 100) as f64, 1.0);
    spin.set_value(size as f64);
    spin.set_numeric(true);
    spin
}

/// A labelled size field followed by `px`.
fn size_row(label: &str, spin: &gtk::SpinButton) -> gtk::Box {
    let row = opusdm_ui::labeled_row(label, LabelWidth::Chars(LABEL_CHARS), SPACING, spin);
    row.append(&gtk::Label::new(Some("px")));
    row
}

/// A requester button with bold text: the primary action.
fn bold_button(label: &str) -> gtk::Button {
    let text = gtk::Label::new(None);
    text.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(label)));
    let button = gtk::Button::new();
    button.set_child(Some(&text));
    button.set_size_request(BUTTON_MIN_WIDTH, -1);
    button
}

/// Connect the handlers: format and lossless switch show the fitting options,
/// each size field moves the other one, an empty name disables Save.
fn wire_fields(fields: &Rc<Fields>, save: &gtk::Button) {
    {
        let f = fields.clone();
        fields.format.connect_selected_notify(move |_| refresh_options(&f));
    }
    {
        let f = fields.clone();
        fields.lossless.connect_toggled(move |_| refresh_options(&f));
    }
    {
        let f = fields.clone();
        fields.width.connect_changed(move |spin| {
            let Some(typed) = typed_size(spin) else { return };
            let (_, h) = export::size_for_width(f.image_size.0, f.image_size.1, typed);
            set_synced(&f, &f.height, h);
        });
    }
    {
        let f = fields.clone();
        fields.height.connect_changed(move |spin| {
            let Some(typed) = typed_size(spin) else { return };
            let (w, _) = export::size_for_height(f.image_size.0, f.image_size.1, typed);
            set_synced(&f, &f.width, w);
        });
    }
    {
        let save = save.clone();
        save.set_sensitive(!fields.name.text().trim().is_empty());
        fields.name.connect_changed(move |e| save.set_sensitive(!e.text().trim().is_empty()));
    }
}

/// The number in the text of `spin` while it is typed, `None` when it is not
/// (yet) a size.
fn typed_size(spin: &gtk::SpinButton) -> Option<u32> {
    spin.text().trim().parse::<u32>().ok().filter(|v| *v > 0)
}

/// Set `spin` to `value` without running the handler that would move the
/// other field back.
fn set_synced(fields: &Fields, spin: &gtk::SpinButton, value: u32) {
    if fields.syncing.get() { return; }
    fields.syncing.set(true);
    spin.set_value(value as f64);
    fields.syncing.set(false);
}

/// Show the quality slider for JPG and lossy WebP, the lossless switch for WebP.
fn refresh_options(fields: &Fields) {
    let format = selected_format(fields);
    fields.quality_row.set_visible(format.has_quality() && !(format == Format::WebP && fields.lossless.is_active()));
    fields.lossless_row.set_visible(format == Format::WebP);
}

/// The format picked in the drop-down.
fn selected_format(fields: &Fields) -> Format {
    Format::ALL.get(fields.format.selected() as usize).copied().unwrap_or(Format::Png)
}

/// The values of the fields, taking numbers typed but not confirmed with Enter.
fn read_choice(fields: &Fields) -> SaveChoice {
    fields.width.update();
    fields.height.update();
    let options = Options { format: selected_format(fields), quality: fields.quality.value().round() as u8, lossless: fields.lossless.is_active() };
    let size = (fields.width.value_as_int().max(1) as u32, fields.height.value_as_int().max(1) as u32);
    SaveChoice { stem: fields.name.text().trim().to_string(), options, size }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GTK may be used from one test thread only, so every widget check is
    /// in this one test.
    #[test]
    fn the_size_fields_keep_the_aspect_ratio_and_the_choice_reads_back() {
        if gtk::init().is_err() { return; }
        let fields = Rc::new(build_fields("20261105-101010", Options::new(Format::Png), (1920, 1080)));
        let save = gtk::Button::new();
        wire_fields(&fields, &save);

        fields.width.set_text("960");
        assert_eq!(fields.height.value_as_int(), 540);
        fields.height.set_text("270");
        assert_eq!(fields.width.value_as_int(), 480);
        // The handler still fires after many changes.
        for w in [100, 200, 1920] { fields.width.set_text(&w.to_string()); }
        assert_eq!(fields.height.value_as_int(), 1080);

        assert!(!fields.quality_row.is_visible() && !fields.lossless_row.is_visible(), "PNG has no options");
        fields.format.set_selected(2);
        assert!(fields.quality_row.is_visible() && fields.lossless_row.is_visible(), "lossy WebP");
        fields.lossless.set_active(true);
        assert!(!fields.quality_row.is_visible(), "lossless WebP has no quality");

        assert!(save.is_sensitive());
        fields.name.set_text("  ");
        assert!(!save.is_sensitive());
        fields.name.set_text(" shot ");
        let choice = read_choice(&fields);
        assert_eq!(choice.stem, "shot");
        assert_eq!(choice.size, (1920, 1080));
        assert_eq!(choice.options.format, Format::WebP);
        assert!(choice.options.lossless);
    }
}
