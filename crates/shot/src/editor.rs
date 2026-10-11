//! The editor window: the cropped screenshot on a canvas, a tool bar for lines,
//! arrows and text, and the Save, Copy and Quick save actions.

use crate::annotations::{self, Annotation, Color, Document, Point, Shape, Size, DEFAULT_COLOR, DEFAULT_SIZE, PALETTE};
use crate::config;
use crate::export::{self, Format, Options};
use crate::icons::{self, Icon};
use crate::render;
use gtk4 as gtk;
use gtk::cairo;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use image::RgbaImage;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// A drag shorter than this many pixels does not create a line or arrow.
const MIN_DRAG: f64 = 3.0;

/// Largest default size of the window, in pixels.
const MAX_WINDOW_WIDTH: i32 = 1400;
const MAX_WINDOW_HEIGHT: i32 = 900;

/// Space around the canvas that the window needs besides the image.
const CHROME_WIDTH: i32 = 24;
const CHROME_HEIGHT: i32 = 150;

/// Width of the quality slider.
const QUALITY_WIDTH: i32 = 160;

/// Padding and spacing of the bars.
const BAR_SPACING: i32 = 6;

/// Dash pattern of the selection box on the canvas.
const SELECTION_DASH: [f64; 2] = [5.0, 3.0];

/// Gap between an annotation and its selection box.
const SELECTION_GAP: f64 = 3.0;

/// Style of the editor, to add to the OpusDM theme: the palette swatches are
/// generated from [`PALETTE`].
pub fn editor_css() -> String {
    let mut css = String::from(
        ".shot-error { color: #e5484d; } .shot-info { opacity: 0.8; } \
         .shot-canvas { background: #404040; } \
         button.shot-swatch { min-width: 20px; min-height: 20px; padding: 0; background-image: none; } \
         button.toggle:checked:not(.shot-swatch) { background-image: none; background-color: alpha(@opusdm_accent, 0.55); } \
         button.shot-swatch:checked { outline: 2px solid @opusdm_accent; outline-offset: 1px; }",
    );
    for (i, (_, c)) in PALETTE.iter().enumerate() {
        let to_byte = |v: f64| (v * 255.0).round() as u8;
        css.push_str(&format!(
            " button.shot-color-{i} {{ background-color: rgb({},{},{}); }}",
            to_byte(c.r), to_byte(c.g), to_byte(c.b)
        ));
    }
    css
}

/// The tool picked in the tool bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Line,
    Arrow,
    Text,
}

/// A mouse drag in progress.
enum Drag {
    None,
    /// Drawing a line or arrow from `start`.
    Draw { start: Point },
    /// Moving the selected annotation; `last` is the offset already applied.
    Move { last: Point },
}

/// A text being typed.
struct Edit {
    index: usize,
    /// Created by this edit: an empty result is dropped instead of kept.
    is_new: bool,
}

/// What the callbacks change.
struct State {
    base: RgbaImage,
    surface: cairo::ImageSurface,
    doc: Document,
    tool: Tool,
    color: Color,
    size: Size,
    selected: Option<usize>,
    drag: Drag,
    edit: Option<Edit>,
}

/// The widgets the callbacks use, with the state.
struct Ui {
    window: gtk::ApplicationWindow,
    canvas: gtk::DrawingArea,
    status: gtk::Label,
    im: gtk::IMMulticontext,
    format: gtk::DropDown,
    quality: gtk::Scale,
    lossless: gtk::CheckButton,
    delay: gtk::SpinButton,
    state: RefCell<State>,
}

/// Open the editor for `base`.
pub fn open(app: &gtk::Application, base: RgbaImage) {
    let surface = match render::surface_from(&base) {
        Ok(s) => s,
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-shot", "{e}");
            app.quit();
            return;
        }
    };
    let (iw, ih) = (base.width() as i32, base.height() as i32);
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Screenshot")
        .default_width((iw + CHROME_WIDTH).min(MAX_WINDOW_WIDTH))
        .default_height((ih + CHROME_HEIGHT).min(MAX_WINDOW_HEIGHT))
        .build();

    let canvas = gtk::DrawingArea::new();
    canvas.add_css_class("shot-canvas");
    canvas.set_content_width(iw);
    canvas.set_content_height(ih);
    canvas.set_halign(gtk::Align::Start);
    canvas.set_valign(gtk::Align::Start);

    let format = gtk::DropDown::from_strings(&Format::ALL.map(Format::label));
    let quality = gtk::Scale::with_range(gtk::Orientation::Horizontal, export::MIN_QUALITY as f64, export::MAX_QUALITY as f64, 1.0);
    quality.set_value(export::DEFAULT_QUALITY as f64);
    quality.set_draw_value(true);
    quality.set_width_request(QUALITY_WIDTH);
    let lossless = gtk::CheckButton::with_label("Lossless");
    let delay = gtk::SpinButton::with_range(0.0, config::MAX_DELAY_SECS as f64, 1.0);
    delay.set_value(config::load().delay_secs as f64);
    delay.set_tooltip_text(Some("Seconds to wait before the screen is captured, from the next launch"));

    let ui = Rc::new(Ui {
        window: window.clone(),
        canvas: canvas.clone(),
        status: gtk::Label::new(None),
        im: gtk::IMMulticontext::new(),
        format,
        quality,
        lossless,
        delay,
        state: RefCell::new(State {
            base,
            surface,
            doc: Document::default(),
            tool: Tool::Arrow,
            color: DEFAULT_COLOR,
            size: DEFAULT_SIZE,
            selected: None,
            drag: Drag::None,
            edit: None,
        }),
    });
    ui.status.set_halign(gtk::Align::Start);
    ui.status.set_margin_start(BAR_SPACING);
    ui.status.set_visible(false);

    wire_canvas(&ui);
    wire_keys(&ui);

    let root = gtk::Box::new(gtk::Orientation::Vertical, BAR_SPACING);
    root.append(&top_bar(&ui));
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&canvas));
    root.append(&scroll);
    root.append(&ui.status);
    root.append(&bottom_bar(&ui));
    window.set_child(Some(&root));
    refresh_save_options(&ui);
    window.present();
}

/// The tool bar: tools, colours, sizes, undo and redo.
fn top_bar(ui: &Rc<Ui>) -> gtk::Box {
    let bar = bar();

    let tools = [
        ("Select and move", Icon::Select, Tool::Select),
        ("Line", Icon::Line, Tool::Line),
        ("Arrow", Icon::Arrow, Tool::Arrow),
        ("Text", Icon::Text, Tool::Text),
    ];
    let mut first: Option<gtk::ToggleButton> = None;
    for (name, icon, tool) in tools {
        let button = toggle(first.as_ref());
        button.set_child(Some(&icons::widget(icon)));
        button.set_tooltip_text(Some(name));
        button.set_active(tool == Tool::Arrow);
        let ui = ui.clone();
        button.connect_toggled(move |b| {
            if !b.is_active() { return; }
            commit_text(&ui);
            ui.state.borrow_mut().tool = tool;
        });
        bar.append(&button);
        first.get_or_insert(button);
    }
    bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

    let mut first: Option<gtk::ToggleButton> = None;
    for (i, (name, color)) in PALETTE.iter().enumerate() {
        let button = toggle(first.as_ref());
        button.add_css_class("shot-swatch");
        button.add_css_class(&format!("shot-color-{i}"));
        button.set_tooltip_text(Some(name));
        button.set_active(*color == DEFAULT_COLOR);
        let (ui, color) = (ui.clone(), *color);
        button.connect_toggled(move |b| {
            if !b.is_active() { return; }
            ui.state.borrow_mut().color = color;
            restyle_selected(&ui);
        });
        bar.append(&button);
        first.get_or_insert(button);
    }
    bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

    let mut first: Option<gtk::ToggleButton> = None;
    for (name, size) in [("Small", Size::Small), ("Medium", Size::Medium), ("Large", Size::Large)] {
        let button = toggle(first.as_ref());
        button.set_child(Some(&icons::widget(Icon::Width(size))));
        button.set_tooltip_text(Some(&format!("{name}: stroke {} px, text {} px", size.line_width(), size.font_size())));
        button.set_active(size == DEFAULT_SIZE);
        let ui = ui.clone();
        button.connect_toggled(move |b| {
            if !b.is_active() { return; }
            ui.state.borrow_mut().size = size;
            restyle_selected(&ui);
        });
        bar.append(&button);
        first.get_or_insert(button);
    }
    bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

    let undo = gtk::Button::new();
    undo.set_child(Some(&icons::widget(Icon::Undo)));
    undo.set_focus_on_click(false);
    undo.set_tooltip_text(Some("Undo (Ctrl+Z)"));
    {
        let ui = ui.clone();
        undo.connect_clicked(move |_| undo_step(&ui));
    }
    let redo = gtk::Button::new();
    redo.set_child(Some(&icons::widget(Icon::Redo)));
    redo.set_focus_on_click(false);
    redo.set_tooltip_text(Some("Redo (Ctrl+Shift+Z)"));
    {
        let ui = ui.clone();
        redo.connect_clicked(move |_| redo_step(&ui));
    }
    bar.append(&undo);
    bar.append(&redo);
    bar
}

/// The bottom bar: format options on the left, the three save actions on the
/// right.
fn bottom_bar(ui: &Rc<Ui>) -> gtk::Box {
    let bar = bar();
    bar.append(&gtk::Label::new(Some("Format")));
    bar.append(&ui.format);
    bar.append(&ui.quality);
    bar.append(&ui.lossless);
    bar.append(&gtk::Label::new(Some("Delay (s)")));
    bar.append(&ui.delay);
    ui.delay.connect_value_changed(|spin| config::save(&config::ShotConfig { delay_secs: spin.value_as_int().max(0) as u32 }));
    {
        let ui2 = ui.clone();
        ui.format.connect_selected_notify(move |_| refresh_save_options(&ui2));
        let ui2 = ui.clone();
        ui.lossless.connect_toggled(move |_| refresh_save_options(&ui2));
    }

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);

    let copy = gtk::Button::with_label("Copy");
    copy.set_tooltip_text(Some("Copy the image to the clipboard"));
    let quick = gtk::Button::with_label("Quick save");
    quick.set_tooltip_text(Some("Save a PNG in ~/Pictures/Screenshots"));
    let save = gtk::Button::with_label("Save as...");
    save.add_css_class("suggested-action");
    for b in [&copy, &quick, &save] {
        b.set_focus_on_click(false);
        bar.append(b);
    }
    { let ui = ui.clone(); copy.connect_clicked(move |_| copy_to_clipboard(&ui)); }
    { let ui = ui.clone(); quick.connect_clicked(move |_| quick_save(&ui)); }
    { let ui = ui.clone(); save.connect_clicked(move |_| save_as(&ui)); }
    bar
}

/// A horizontal bar with the shared spacing.
fn bar() -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, BAR_SPACING);
    bar.set_margin_start(BAR_SPACING);
    bar.set_margin_end(BAR_SPACING);
    bar.set_margin_top(BAR_SPACING);
    bar.set_margin_bottom(BAR_SPACING);
    bar
}

/// A toggle button of a radio group (`group` is the first button of it).
fn toggle(group: Option<&gtk::ToggleButton>) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::new();
    button.set_focus_on_click(false);
    if let Some(g) = group { button.set_group(Some(g)); }
    button
}

/// Show the quality slider for JPG and lossy WebP, the lossless switch for WebP.
fn refresh_save_options(ui: &Ui) {
    let format = selected_format(ui);
    ui.quality.set_visible(format.has_quality() && !(format == Format::WebP && ui.lossless.is_active()));
    ui.lossless.set_visible(format == Format::WebP);
}

/// The format picked in the bottom bar.
fn selected_format(ui: &Ui) -> Format {
    Format::ALL.get(ui.format.selected() as usize).copied().unwrap_or(Format::Png)
}

/// The encoding options as set in the bottom bar, for `format`.
fn options_for(ui: &Ui, format: Format) -> Options {
    Options { format, quality: ui.quality.value().round() as u8, lossless: ui.lossless.is_active() }
}

// ----- canvas ---------------------------------------------------------------

/// Draw the canvas and handle the mouse on it.
fn wire_canvas(ui: &Rc<Ui>) {
    {
        let ui = ui.clone();
        ui.canvas.clone().set_draw_func(move |_, cr, _, _| {
            let _ = paint(&ui, cr);
        });
    }

    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let start = Rc::new(RefCell::new((0.0, 0.0)));
    {
        let (ui, start) = (ui.clone(), start.clone());
        drag.connect_drag_begin(move |_, x, y| {
            *start.borrow_mut() = (x, y);
            begin_drag(&ui, (x, y));
        });
    }
    {
        let (ui, start) = (ui.clone(), start.clone());
        drag.connect_drag_update(move |g, dx, dy| {
            let shift = g.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
            update_drag(&ui, *start.borrow(), (dx, dy), shift);
        });
    }
    {
        let ui = ui.clone();
        drag.connect_drag_end(move |_, dx, dy| end_drag(&ui, (dx, dy)));
    }
    ui.canvas.add_controller(drag);
}

/// Draw the image, the annotations and the box of the selected one.
fn paint(ui: &Ui, cr: &cairo::Context) -> Result<(), cairo::Error> {
    let st = ui.state.borrow();
    cr.set_source_surface(&st.surface, 0.0, 0.0)?;
    cr.paint()?;
    render::draw(cr, st.doc.items())?;
    let Some(item) = st.selected.and_then(|i| st.doc.items().get(i)) else { return Ok(()) };
    let (l, t, r, b) = item.bounds();
    cr.set_source_rgb(0.25, 0.55, 1.0);
    cr.set_line_width(1.0);
    cr.set_dash(&SELECTION_DASH, 0.0);
    cr.rectangle(l - SELECTION_GAP, t - SELECTION_GAP, r - l + 2.0 * SELECTION_GAP, b - t + 2.0 * SELECTION_GAP);
    cr.stroke()
}

/// The mouse went down at `p`: select, start drawing or start a text.
fn begin_drag(ui: &Ui, p: Point) {
    commit_text(ui);
    let mut st = ui.state.borrow_mut();
    match st.tool {
        Tool::Select => {
            st.selected = st.doc.hit_test(p);
            if st.selected.is_some() {
                st.doc.checkpoint();
                st.drag = Drag::Move { last: (0.0, 0.0) };
            }
        }
        Tool::Line | Tool::Arrow => {
            let shape = if st.tool == Tool::Line { Shape::Line { from: p, to: p } } else { Shape::Arrow { from: p, to: p } };
            let annotation = styled(&st, shape);
            let index = st.doc.add(annotation);
            st.selected = Some(index);
            st.drag = Drag::Draw { start: p };
        }
        Tool::Text => {
            let existing = st.doc.hit_test(p).filter(|&i| matches!(st.doc.items()[i].shape, Shape::Text { .. }));
            let edit = match existing {
                Some(index) => {
                    st.doc.checkpoint();
                    Edit { index, is_new: false }
                }
                None => {
                    let annotation = styled(&st, Shape::Text { at: p, text: String::new() });
                    Edit { index: st.doc.add(annotation), is_new: true }
                }
            };
            st.selected = Some(edit.index);
            st.edit = Some(edit);
            ui.im.focus_in();
        }
    }
    drop(st);
    ui.canvas.queue_draw();
}

/// A new annotation of `shape` with the colour and size of the tool bar.
fn styled(st: &State, shape: Shape) -> Annotation {
    Annotation { shape, color: st.color, size: st.size }
}

/// The mouse moved by `delta` since `start`.
fn update_drag(ui: &Ui, start: Point, delta: Point, shift: bool) {
    let mut st = ui.state.borrow_mut();
    let Some(index) = st.selected else { return };
    match st.drag {
        Drag::Draw { start: from } => {
            let mut to = (start.0 + delta.0, start.1 + delta.1);
            if shift { to = annotations::snap_angle(from, to); }
            st.doc.modify(index, |a| match &mut a.shape {
                Shape::Line { to: t, .. } | Shape::Arrow { to: t, .. } => *t = to,
                Shape::Text { .. } => {}
            });
        }
        Drag::Move { last } => {
            st.doc.modify(index, |a| a.translate(delta.0 - last.0, delta.1 - last.1));
            st.drag = Drag::Move { last: delta };
        }
        Drag::None => return,
    }
    drop(st);
    ui.canvas.queue_draw();
}

/// The mouse was released after moving by `delta`: a drawing that is just a
/// click is dropped.
fn end_drag(ui: &Ui, delta: Point) {
    let mut st = ui.state.borrow_mut();
    if let Drag::Draw { .. } = st.drag {
        if delta.0.hypot(delta.1) < MIN_DRAG {
            st.doc.undo();
            st.selected = None;
        }
    }
    st.drag = Drag::None;
    drop(st);
    ui.canvas.queue_draw();
}

/// Give the selected annotation the colour and size of the tool bar.
fn restyle_selected(ui: &Ui) {
    let mut st = ui.state.borrow_mut();
    let Some(index) = st.selected else { return };
    let (color, size) = (st.color, st.size);
    st.doc.checkpoint();
    st.doc.modify(index, |a| {
        a.color = color;
        a.size = size;
    });
    drop(st);
    ui.canvas.queue_draw();
}

// ----- text and keys --------------------------------------------------------

/// Route the keyboard: typed text while editing a text, shortcuts otherwise.
/// The controller runs in the capture phase so a focused button never eats the
/// space or Enter of a text.
fn wire_keys(ui: &Rc<Ui>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    ui.im.set_client_widget(Some(&ui.window));
    keys.set_im_context(Some(&ui.im));
    {
        let ui = ui.clone();
        ui.im.clone().connect_commit(move |_, text| type_text(&ui, text));
    }
    {
        let ui = ui.clone();
        keys.connect_key_pressed(move |_, key, _, modifiers| on_key(&ui, key, modifiers));
    }
    ui.window.add_controller(keys);
}

/// Handle a key press that the input method did not take.
fn on_key(ui: &Ui, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
    let editing = ui.state.borrow().edit.is_some();
    let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    if editing {
        match key {
            gdk::Key::Escape => commit_text(ui),
            gdk::Key::BackSpace => erase_char(ui),
            gdk::Key::Return | gdk::Key::KP_Enter => type_text(ui, "\n"),
            _ => return glib::Propagation::Proceed,
        }
        return glib::Propagation::Stop;
    }
    match key {
        gdk::Key::z | gdk::Key::Z if ctrl && modifiers.contains(gdk::ModifierType::SHIFT_MASK) => redo_step(ui),
        gdk::Key::z if ctrl => undo_step(ui),
        gdk::Key::y if ctrl => redo_step(ui),
        gdk::Key::Delete => delete_selected(ui),
        gdk::Key::Escape => ui.window.close(),
        _ => return glib::Propagation::Proceed,
    }
    glib::Propagation::Stop
}

/// Append `text` to the text being edited.
fn type_text(ui: &Ui, text: &str) {
    let mut st = ui.state.borrow_mut();
    let Some(index) = st.edit.as_ref().map(|e| e.index) else { return };
    st.doc.modify(index, |a| {
        if let Shape::Text { text: t, .. } = &mut a.shape { t.push_str(text); }
    });
    drop(st);
    ui.canvas.queue_draw();
}

/// Remove the last character of the text being edited.
fn erase_char(ui: &Ui) {
    let mut st = ui.state.borrow_mut();
    let Some(index) = st.edit.as_ref().map(|e| e.index) else { return };
    st.doc.modify(index, |a| {
        if let Shape::Text { text: t, .. } = &mut a.shape { t.pop(); }
    });
    drop(st);
    ui.canvas.queue_draw();
}

/// Finish the text being edited; an empty one is dropped.
fn commit_text(ui: &Ui) {
    let mut st = ui.state.borrow_mut();
    let Some(edit) = st.edit.take() else { return };
    ui.im.focus_out();
    let empty = matches!(&st.doc.items().get(edit.index).map(|a| &a.shape), Some(Shape::Text { text, .. }) if text.is_empty());
    if empty {
        if edit.is_new { st.doc.undo(); } else { st.doc.remove(edit.index); }
        st.selected = None;
    }
    drop(st);
    ui.canvas.queue_draw();
}

/// Undo the last change.
fn undo_step(ui: &Ui) {
    commit_text(ui);
    let mut st = ui.state.borrow_mut();
    st.doc.undo();
    st.selected = None;
    drop(st);
    ui.canvas.queue_draw();
}

/// Redo the last undone change.
fn redo_step(ui: &Ui) {
    commit_text(ui);
    let mut st = ui.state.borrow_mut();
    st.doc.redo();
    st.selected = None;
    drop(st);
    ui.canvas.queue_draw();
}

/// Delete the selected annotation.
fn delete_selected(ui: &Ui) {
    let mut st = ui.state.borrow_mut();
    if let Some(index) = st.selected.take() { st.doc.remove(index); }
    drop(st);
    ui.canvas.queue_draw();
}

// ----- output ---------------------------------------------------------------

/// Show an error under the canvas and log it; the image stays open.
fn show_error(ui: &Ui, message: &str) {
    opusdm_ipc::err_log!("opusdm-shot", "{message}");
    ui.status.remove_css_class("shot-info");
    ui.status.add_css_class("shot-error");
    ui.status.set_text(message);
    ui.status.set_visible(true);
}

/// Show a note under the canvas.
fn show_info(ui: &Ui, message: &str) {
    ui.status.remove_css_class("shot-error");
    ui.status.add_css_class("shot-info");
    ui.status.set_text(message);
    ui.status.set_visible(true);
}

/// The image with its annotations drawn on it, or `None` after showing why not.
fn final_image(ui: &Ui) -> Option<RgbaImage> {
    commit_text(ui);
    let st = ui.state.borrow();
    match render::render(&st.base, st.doc.items()) {
        Ok(img) => Some(img),
        Err(e) => {
            drop(st);
            show_error(ui, &e);
            None
        }
    }
}

/// Put the image on the clipboard. The window stays open: without a clipboard
/// manager the content would vanish with the process.
fn copy_to_clipboard(ui: &Ui) {
    let Some(img) = final_image(ui) else { return };
    let (w, h) = img.dimensions();
    let bytes = glib::Bytes::from_owned(img.into_raw());
    let texture = gdk::MemoryTexture::new(w as i32, h as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, w as usize * 4);
    WidgetExt::display(&ui.window).clipboard().set_texture(&texture);
    show_info(ui, "Copied to the clipboard (it stays available while this window is open)");
}

/// Save a PNG in `~/Pictures/Screenshots` and close.
fn quick_save(ui: &Ui) {
    let Some(img) = final_image(ui) else { return };
    let Some(home) = dirs::home_dir() else {
        show_error(ui, "cannot find the home directory for the quick save");
        return;
    };
    let now = match glib::DateTime::now_local() {
        Ok(n) => n,
        Err(e) => {
            show_error(ui, &format!("cannot read the clock: {e}"));
            return;
        }
    };
    let stem = export::quick_stem(now.year(), now.month() as u32, now.day_of_month() as u32, now.hour() as u32, now.minute() as u32, now.second() as u32);
    match export::quick_save(&img, &home.join(export::QUICK_SAVE_DIR), &stem) {
        Ok(path) => {
            opusdm_ipc::err_log!("opusdm-shot", "saved {}", path.display());
            ui.window.close();
        }
        Err(e) => show_error(ui, &e),
    }
}

/// Ask for a file name and save in the chosen format, then close.
fn save_as(ui: &Rc<Ui>) {
    let Some(img) = final_image(ui) else { return };
    let format = selected_format(ui);
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    let mut default_filter = None;
    for f in Format::ALL {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(f.label()));
        filter.add_suffix(f.extension());
        if f == Format::Jpg { filter.add_suffix("jpeg"); }
        if f == format { default_filter = Some(filter.clone()); }
        filters.append(&filter);
    }
    let dialog = gtk::FileDialog::builder().title("Save screenshot").modal(true).build();
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(default_filter.as_ref());
    dialog.set_initial_name(Some(&format!("screenshot.{}", format.extension())));
    if let Some(dir) = dirs::picture_dir().filter(|d| d.is_dir()) {
        dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
    }
    let ui2 = ui.clone();
    dialog.save(Some(&ui.window), gio::Cancellable::NONE, move |result| {
        let file = match result {
            Ok(f) => f,
            Err(e) if e.matches(gtk::DialogError::Dismissed) => return,
            Err(e) => return show_error(&ui2, &format!("save dialog failed: {e}")),
        };
        let Some(path) = file.path() else {
            return show_error(&ui2, "the chosen location is not a local file");
        };
        let (path, chosen) = resolve_target(path, format);
        let options = options_for(&ui2, chosen);
        match export::save(&img, &path, &options) {
            Ok(()) => {
                opusdm_ipc::err_log!("opusdm-shot", "saved {}", path.display());
                ui2.window.close();
            }
            Err(e) => show_error(&ui2, &e),
        }
    });
}

/// The path to write and its format: the extension of `path` decides, and a
/// missing or unknown one gets the extension of `fallback` appended.
fn resolve_target(path: PathBuf, fallback: Format) -> (PathBuf, Format) {
    if let Some(format) = Format::from_path(&path) { return (path, format); }
    let mut name = path.into_os_string();
    name.push(format!(".{}", fallback.extension()));
    (PathBuf::from(name), fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_decides_the_format() {
        let (p, f) = resolve_target(PathBuf::from("/tmp/a.JPG"), Format::Png);
        assert_eq!((p, f), (PathBuf::from("/tmp/a.JPG"), Format::Jpg));
    }

    #[test]
    fn a_missing_or_unknown_extension_gets_the_fallback() {
        let (p, f) = resolve_target(PathBuf::from("/tmp/a"), Format::WebP);
        assert_eq!((p, f), (PathBuf::from("/tmp/a.webp"), Format::WebP));
        let (p, f) = resolve_target(PathBuf::from("/tmp/a.txt"), Format::Png);
        assert_eq!((p, f), (PathBuf::from("/tmp/a.txt.png"), Format::Png));
    }

    #[test]
    fn css_has_a_rule_for_every_palette_colour() {
        let css = editor_css();
        for i in 0..PALETTE.len() {
            assert!(css.contains(&format!("shot-color-{i}")));
        }
    }
}
