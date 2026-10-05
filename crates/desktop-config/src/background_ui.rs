//! The DESKTOP BACKGROUND section: the list of backgrounds the desktop
//! rotates through (pictures, videos, colours, gradients), the buttons that
//! add to it, and the interval, fade and shuffle controls.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::{Background, BackgroundEntry, ImageFit, FADE_MS_RANGE, GRADIENT_ANGLE_MAX, INTERVAL_SECS_RANGE};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::ui::{heading, ColorRow};

/// Thumbnail size, in pixels, of a picture row and of a theme picture chooser entry.
const ROW_THUMB: (i32, i32) = (64, 40);
const THEME_PICTURE_THUMB: (i32, i32) = (160, 100);
/// Minimum height of the list, in pixels.
const LIST_MIN_HEIGHT: i32 = 220;
const FIT_LABELS: [&str; 4] = ["Fill", "Fit", "Center", "Tile"];
/// Fits offered for a video: the others behave as Fill.
const VIDEO_FIT_LABELS: [&str; 2] = ["Fill", "Fit"];

/// The whole section.
pub struct BackgroundSection {
    pub root: gtk::Box,
    inner: Rc<Inner>,
    interval: gtk::SpinButton,
    fade: gtk::SpinButton,
    shuffle: gtk::CheckButton,
    /// "Add from theme...": hidden until [`Self::connect_theme_pictures`].
    from_theme: gtk::Button,
}

/// The list and its model. Rows are rebuilt from `entries` after every
/// structural change (add, remove, move); editing a value in place only
/// writes it back into the model.
struct Inner {
    window: gtk::ApplicationWindow,
    list: gtk::ListBox,
    entries: RefCell<Vec<BackgroundEntry>>,
}

impl BackgroundSection {
    /// Build the section, seeded from `initial`. `window` parents the colour
    /// and file dialogs.
    pub fn new(window: &gtk::ApplicationWindow, initial: &Background) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.append(&heading("DESKTOP BACKGROUND"));

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).min_content_height(LIST_MIN_HEIGHT).build();
        scroll.add_css_class("frame");
        root.append(&scroll);

        let inner = Rc::new(Inner { window: window.clone(), list, entries: RefCell::new(initial.entries.clone()) });

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let add_file = gtk::Button::with_label("Add file...");
        let add_color = gtk::Button::with_label("Add color");
        let add_gradient = gtk::Button::with_label("Add gradient");
        let from_theme = gtk::Button::with_label("Add from theme...");
        from_theme.set_visible(false);
        for b in [&add_file, &add_color, &add_gradient, &from_theme] {
            buttons.append(b);
        }
        root.append(&buttons);

        let interval = gtk::SpinButton::with_range(INTERVAL_SECS_RANGE.0 as f64, INTERVAL_SECS_RANGE.1 as f64, 1.0);
        interval.set_value(initial.interval_secs.clamp(INTERVAL_SECS_RANGE.0, INTERVAL_SECS_RANGE.1) as f64);
        let fade = gtk::SpinButton::with_range(FADE_MS_RANGE.0 as f64, FADE_MS_RANGE.1 as f64, 50.0);
        fade.set_value(initial.fade_ms.clamp(FADE_MS_RANGE.0, FADE_MS_RANGE.1) as f64);
        let shuffle = gtk::CheckButton::with_label("Shuffle");
        shuffle.set_active(initial.shuffle);
        root.append(&_labelled("Interval (s)", &interval));
        root.append(&_labelled("Fade (ms)", &fade));
        root.append(&shuffle);

        {
            let inner = inner.clone();
            add_file.connect_clicked(move |_| inner.choose_files());
        }
        {
            let inner = inner.clone();
            add_color.connect_clicked(move |_| inner.add(vec![BackgroundEntry::default_solid()]));
        }
        {
            let inner = inner.clone();
            add_gradient.connect_clicked(move |_| inner.add(vec![BackgroundEntry::default_gradient()]));
        }
        inner.rebuild();
        Self { root, inner, interval, fade, shuffle, from_theme }
    }

    /// Show the "Add from theme..." button. `pictures` lists the pictures of
    /// the theme currently selected (called each time the button is pressed);
    /// the one the user picks is added to the list.
    pub fn connect_theme_pictures(&self, window: &gtk::ApplicationWindow, pictures: impl Fn() -> Vec<PathBuf> + 'static) {
        self.from_theme.set_visible(true);
        let window = window.clone();
        let inner = self.inner.clone();
        self.from_theme.connect_clicked(move |_| {
            let inner = inner.clone();
            choose_theme_picture(&window, pictures(), move |path| inner.add_paths(vec![path]));
        });
    }

    /// Read every control back into a `Background`.
    pub fn read(&self) -> Background {
        Background {
            entries: self.inner.entries.borrow().clone(),
            interval_secs: self.interval.value() as u32,
            fade_ms: self.fade.value() as u32,
            shuffle: self.shuffle.is_active(),
        }
    }
}

/// A row with a fixed-width name on the left and `widget` after it.
fn _labelled(text: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(text));
    name.set_xalign(0.0);
    name.set_size_request(90, -1);
    row.append(&name);
    row.append(widget);
    row
}

/// The dropdown index of `fit`, among `count` choices.
fn _fit_index(fit: ImageFit, count: usize) -> u32 {
    let idx = match fit {
        ImageFit::Fill => 0,
        ImageFit::Fit => 1,
        ImageFit::Center => 2,
        ImageFit::Tile => 3,
    };
    if idx < count { idx as u32 } else { 0 }
}

/// The fit a dropdown index stands for.
fn _fit_from_index(index: u32) -> ImageFit {
    match index {
        1 => ImageFit::Fit,
        2 => ImageFit::Center,
        3 => ImageFit::Tile,
        _ => ImageFit::Fill,
    }
}

impl Inner {
    /// Append `new` entries and show them.
    fn add(self: &Rc<Self>, new: Vec<BackgroundEntry>) {
        self.entries.borrow_mut().extend(new);
        self.rebuild();
    }

    /// Append the pictures and videos among `paths`, one entry each.
    fn add_paths(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let new = paths
            .into_iter()
            .filter_map(|p| {
                let entry = BackgroundEntry::from_path(p.clone());
                if entry.is_none() {
                    opusdm_ipc::err_log!("opusdm-settings", "{} is not a picture or a video; skipped", p.display());
                }
                entry
            })
            .collect();
        self.add(new);
    }

    /// Ask for picture and video files and add them.
    fn choose_files(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("Add backgrounds").build();
        let filter = gtk::FileFilter::new();
        filter.add_pixbuf_formats();
        filter.add_mime_type("video/*");
        filter.set_name(Some("Images and videos"));
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));
        let inner = self.clone();
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let Ok(files) = res else { return };
            let paths = (0..files.n_items())
                .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                .filter_map(|f| f.path())
                .collect();
            inner.add_paths(paths);
        });
    }

    /// Swap entry `index` with the one `delta` places away, when there is one.
    fn move_entry(self: &Rc<Self>, index: usize, delta: isize) {
        let Some(other) = index.checked_add_signed(delta) else { return };
        if other >= self.entries.borrow().len() {
            return;
        }
        self.entries.borrow_mut().swap(index, other);
        self.rebuild();
    }

    /// Drop entry `index`.
    fn remove(self: &Rc<Self>, index: usize) {
        if index < self.entries.borrow().len() {
            self.entries.borrow_mut().remove(index);
            self.rebuild();
        }
    }

    /// Write `entry` back into slot `index` (an in-place edit: no rebuild).
    fn set(&self, index: usize, entry: BackgroundEntry) {
        if let Some(slot) = self.entries.borrow_mut().get_mut(index) {
            *slot = entry;
        }
    }

    /// Throw the rows away and build them again from the model.
    fn rebuild(self: &Rc<Self>) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let entries = self.entries.borrow().clone();
        if entries.is_empty() {
            let note = gtk::Label::new(Some("No background: the desktop shows the default colour."));
            note.set_margin_top(12);
            note.set_margin_bottom(12);
            self.list.append(&note);
        }
        let last = entries.len().saturating_sub(1);
        for (i, entry) in entries.iter().enumerate() {
            self.list.append(&self._row(i, entry, i > 0, i < last));
        }
    }

    /// The row of entry `index`: its editor, then move up / down / remove.
    fn _row(self: &Rc<Self>, index: usize, entry: &BackgroundEntry, can_up: bool, can_down: bool) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_top(4);
        row.set_margin_bottom(4);
        row.set_margin_start(6);
        row.set_margin_end(6);
        let editor = match entry {
            BackgroundEntry::Solid { color } => self._solid_editor(index, color),
            BackgroundEntry::Gradient { start, end, angle } => self._gradient_editor(index, start, end, *angle),
            BackgroundEntry::Image { path, fit } => self._file_editor(index, path, *fit, false),
            BackgroundEntry::Video { path, fit } => self._file_editor(index, path, *fit, true),
        };
        editor.set_hexpand(true);
        row.append(&editor);

        let up = gtk::Button::from_icon_name("go-up-symbolic");
        up.set_sensitive(can_up);
        let down = gtk::Button::from_icon_name("go-down-symbolic");
        down.set_sensitive(can_down);
        let remove = gtk::Button::from_icon_name("list-remove-symbolic");
        up.set_tooltip_text(Some("Move up"));
        down.set_tooltip_text(Some("Move down"));
        remove.set_tooltip_text(Some("Remove"));
        {
            let inner = self.clone();
            up.connect_clicked(move |_| inner.move_entry(index, -1));
        }
        {
            let inner = self.clone();
            down.connect_clicked(move |_| inner.move_entry(index, 1));
        }
        {
            let inner = self.clone();
            remove.connect_clicked(move |_| inner.remove(index));
        }
        for b in [&up, &down, &remove] {
            b.set_valign(gtk::Align::Center);
            row.append(b);
        }
        row
    }

    /// Editor of a flat colour.
    fn _solid_editor(self: &Rc<Self>, index: usize, color: &str) -> gtk::Box {
        let editor = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let picker = Rc::new(ColorRow::new(&self.window, Some("Colour"), color));
        {
            let (inner, picker) = (self.clone(), picker.clone());
            picker.clone().connect_changed(move || inner.set(index, BackgroundEntry::Solid { color: picker.hex() }));
        }
        editor.append(&picker.root);
        editor
    }

    /// Editor of a gradient: both colours and the angle.
    fn _gradient_editor(self: &Rc<Self>, index: usize, start: &str, end: &str, angle: u16) -> gtk::Box {
        let editor = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let start = Rc::new(ColorRow::new(&self.window, Some("Start"), start));
        let end = Rc::new(ColorRow::new(&self.window, Some("End"), end));
        let angle_spin = gtk::SpinButton::with_range(0.0, GRADIENT_ANGLE_MAX as f64, 1.0);
        angle_spin.set_value(angle as f64);
        let write = {
            let (inner, start, end, angle_spin) = (self.clone(), start.clone(), end.clone(), angle_spin.clone());
            Rc::new(move || {
                inner.set(
                    index,
                    BackgroundEntry::Gradient { start: start.hex(), end: end.hex(), angle: angle_spin.value() as u16 },
                )
            })
        };
        for color in [&start, &end] {
            let write = write.clone();
            color.connect_changed(move || write());
        }
        {
            let write = write.clone();
            angle_spin.connect_value_changed(move |_| write());
        }
        editor.append(&start.root);
        editor.append(&end.root);
        editor.append(&_labelled("Angle", &angle_spin));
        editor
    }

    /// Editor of a picture or a video: thumbnail, file name, fit.
    fn _file_editor(self: &Rc<Self>, index: usize, path: &PathBuf, fit: ImageFit, video: bool) -> gtk::Box {
        let editor = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if video {
            editor.append(&gtk::Image::from_icon_name("video-x-generic"));
        } else {
            // Decoded at thumbnail size: a full-size picture would make the row as big as the picture.
            match gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, ROW_THUMB.0, ROW_THUMB.1, true) {
                Ok(pixbuf) => {
                    let thumb = gtk::Picture::for_pixbuf(&pixbuf);
                    thumb.set_can_shrink(false);
                    thumb.set_valign(gtk::Align::Center);
                    editor.append(&thumb);
                }
                Err(_) => editor.append(&gtk::Image::from_icon_name("image-x-generic")),
            }
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let label = gtk::Label::new(Some(&name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_tooltip_text(Some(&path.to_string_lossy()));
        editor.append(&label);
        if !path.is_file() {
            let warning = gtk::Label::new(Some("file not found"));
            warning.add_css_class("warning");
            editor.append(&warning);
        }
        let labels: &[&str] = if video { &VIDEO_FIT_LABELS } else { &FIT_LABELS };
        let fit_choice = gtk::DropDown::from_strings(labels);
        fit_choice.set_selected(_fit_index(fit, labels.len()));
        {
            let (inner, path) = (self.clone(), path.clone());
            fit_choice.connect_selected_notify(move |d| {
                let fit = _fit_from_index(d.selected());
                let path = path.clone();
                inner.set(index, if video { BackgroundEntry::Video { path, fit } } else { BackgroundEntry::Image { path, fit } });
            });
        }
        fit_choice.set_valign(gtk::Align::Center);
        editor.append(&fit_choice);
        editor
    }
}

/// A modal chooser showing `pictures` as thumbnails; `on_chosen` gets the
/// one the user clicks. An empty list shows a short note instead.
fn choose_theme_picture(window: &gtk::ApplicationWindow, pictures: Vec<PathBuf>, on_chosen: impl Fn(PathBuf) + 'static) {
    let dialog = gtk::Window::builder()
        .title("Wallpapers of the Theme")
        .transient_for(window)
        .modal(true)
        .default_width(560)
        .default_height(420)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    let close = gtk::Button::with_label("Close");
    close.set_halign(gtk::Align::End);
    {
        let dialog = dialog.clone();
        close.connect_clicked(move |_| dialog.close());
    }
    if pictures.is_empty() {
        let note = gtk::Label::new(Some("This theme has no wallpapers."));
        note.set_vexpand(true);
        content.append(&note);
        content.append(&close);
        dialog.set_child(Some(&content));
        dialog.present();
        return;
    }

    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_activate_on_single_click(true);
    flow.set_valign(gtk::Align::Start);
    for path in &pictures {
        let thumb = gtk::Picture::for_filename(path);
        thumb.set_size_request(THEME_PICTURE_THUMB.0, THEME_PICTURE_THUMB.1);
        thumb.set_content_fit(gtk::ContentFit::Cover);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        thumb.set_tooltip_text(Some(&name));
        flow.append(&thumb);
    }
    {
        let dialog = dialog.clone();
        flow.connect_child_activated(move |_, child| {
            let Some(path) = pictures.get(child.index() as usize) else { return };
            on_chosen(path.clone());
            dialog.close();
        });
    }
    content.append(&gtk::ScrolledWindow::builder().child(&flow).vexpand(true).build());
    content.append(&close);
    dialog.set_child(Some(&content));
    dialog.present();
}

