//! The Viewer window (F-16 / F-17): Read, AnsiRead, HexRead, SmartRead and
//! Show. Loading and every decision live in the pure modules; this file only
//! builds widgets and wires keys.

use crate::ansi::{AnsiDoc, PALETTE};
use crate::slideshow::Marks;
use crate::viewer_load::{load, ViewData};
use crate::viewer_mode::{image_files, viewer_files, Mode};
use crate::viewer_search::{find_matches, step};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::{gdk, glib};
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

/// Highlight of every match.
const MATCH_TAG: &str = "match";
/// Highlight of the match the search is on.
const CURRENT_TAG: &str = "current";

/// What a key press does in the Viewer once the search entry is not typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewerKey {
    /// Next file.
    Next,
    /// Previous file.
    Prev,
    /// Switch the mode.
    SetMode(Mode),
    /// Mark or unmark the picture (Show only).
    ToggleMark,
}

/// Map a key to its Viewer action. Chords with Ctrl or Alt are never ours, so
/// select-all and copy keep working. `image` is true while a picture is shown
/// (arrows page then, and scroll text otherwise); mode keys are off in Show,
/// marking is Show only. Letters work in either case.
fn viewer_key(key: gdk::Key, modified: bool, show: bool, image: bool) -> Option<ViewerKey> {
    if modified {
        return None;
    }
    match key {
        gdk::Key::n | gdk::Key::N => Some(ViewerKey::Next),
        gdk::Key::p | gdk::Key::P => Some(ViewerKey::Prev),
        gdk::Key::Right if image => Some(ViewerKey::Next),
        gdk::Key::Left if image => Some(ViewerKey::Prev),
        gdk::Key::Delete | gdk::Key::KP_Delete | gdk::Key::m | gdk::Key::M if show => Some(ViewerKey::ToggleMark),
        gdk::Key::t | gdk::Key::T if !show => Some(ViewerKey::SetMode(Mode::Text)),
        gdk::Key::a | gdk::Key::A if !show => Some(ViewerKey::SetMode(Mode::Ansi)),
        gdk::Key::x | gdk::Key::X if !show => Some(ViewerKey::SetMode(Mode::Hex)),
        gdk::Key::s | gdk::Key::S if !show => Some(ViewerKey::SetMode(Mode::Smart)),
        _ => None,
    }
}

/// Extra options when opening the Viewer from code.
#[derive(Default)]
pub struct ViewerOptions {
    /// Search text to fill in and jump to once the first file is shown.
    pub initial_search: Option<String>,
}

/// One open Viewer.
struct Viewer {
    ui: Rc<Ui>,
    window: gtk::Window,
    files: Vec<PathBuf>,
    index: Cell<usize>,
    mode: Cell<Mode>,
    show: bool,
    generation: Cell<u64>,
    title: gtk::Label,
    meta: gtk::Label,
    content: gtk::Box,
    text_view: RefCell<Option<gtk::TextView>>,
    search_bar: gtk::Box,
    search_entry: gtk::Entry,
    search_count: gtk::Label,
    matches: RefCell<Vec<(usize, usize)>>,
    current: Cell<Option<usize>>,
    marks: RefCell<Marks>,
    /// Set once the delete question was answered, so the close goes through.
    closing: Cell<bool>,
    marked: gtk::Label,
}

/// Open the Viewer on the files selected in `ui` (or the cursor file). For
/// `show`, only the pictures among them.
pub fn open_from_selection(ui: &Rc<Ui>, mode: Mode, show: bool) {
    let cursor = ui.row_at(ui.view.cursor.get()).map(|r| ui.cwd_path().join(&r.name));
    let mut files = viewer_files(&ui.selected_paths(), cursor.as_deref());
    if show {
        files = image_files(&files);
        if files.is_empty() {
            crate::actions::flash(ui, "no images selected");
            return;
        }
    }
    if files.is_empty() {
        crate::actions::flash(ui, "no file selected");
        return;
    }
    open(ui, files, mode, show, ViewerOptions::default());
}

/// Open a Viewer on `files`.
pub fn open(ui: &Rc<Ui>, files: Vec<PathBuf>, mode: Mode, show: bool, opts: ViewerOptions) {
    if files.is_empty() {
        return;
    }
    let viewer = Viewer::new(ui, files, mode, show);
    viewer.install_keys();
    viewer.install_close_guard();
    if let Some(query) = opts.initial_search {
        viewer.search_entry.set_text(&query);
        viewer.search_bar.set_visible(true);
    }
    viewer.show_file();
    viewer.window.present();
}

impl Viewer {
    /// Build the window and its widgets.
    fn new(ui: &Rc<Ui>, files: Vec<PathBuf>, mode: Mode, show: bool) -> Rc<Self> {
        let window = gtk::Window::builder()
            .title(if show { "Show" } else { "Viewer" })
            .transient_for(&ui.window)
            .default_width(820)
            .default_height(620)
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_margin_start(12);
        header.set_margin_end(12);
        header.set_margin_top(8);
        header.set_margin_bottom(8);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.add_css_class("preview-title");
        let meta = gtk::Label::new(None);
        meta.set_xalign(1.0);
        meta.add_css_class("dim-label");
        let marked = gtk::Label::new(None);
        marked.add_css_class("dim-label");
        let close = gtk::Button::with_label("\u{2715}");
        close.add_css_class("flat");
        {
            let window = window.clone();
            close.connect_clicked(move |_| window.close());
        }
        header.append(&title);
        header.append(&meta);
        header.append(&marked);
        header.append(&close);
        root.append(&header);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let search_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        search_bar.set_margin_start(12);
        search_bar.set_margin_end(12);
        search_bar.set_margin_top(4);
        search_bar.set_margin_bottom(4);
        search_bar.set_visible(false);
        search_bar.append(&gtk::Label::new(Some("Find")));
        let search_entry = gtk::Entry::new();
        search_entry.set_hexpand(true);
        let prev_btn = gtk::Button::with_label("\u{25B2}");
        let next_btn = gtk::Button::with_label("\u{25BC}");
        let search_count = gtk::Label::new(None);
        search_count.set_width_chars(8);
        search_bar.append(&search_entry);
        search_bar.append(&prev_btn);
        search_bar.append(&next_btn);
        search_bar.append(&search_count);
        root.append(&search_bar);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_vexpand(true);
        content.set_hexpand(true);
        root.append(&content);
        window.set_child(Some(&root));

        let viewer = Rc::new(Self {
            ui: ui.clone(),
            window,
            files,
            index: Cell::new(0),
            mode: Cell::new(mode),
            show,
            generation: Cell::new(0),
            title,
            meta,
            content,
            text_view: RefCell::new(None),
            search_bar,
            search_entry: search_entry.clone(),
            search_count,
            matches: RefCell::new(Vec::new()),
            current: Cell::new(None),
            marks: RefCell::new(Marks::new()),
            closing: Cell::new(false),
            marked,
        });

        // The entry is owned by the Viewer, so its handlers hold only a weak
        // reference: a strong one would keep the Viewer (and its text buffer)
        // alive after the window is gone.
        {
            let v = Rc::downgrade(&viewer);
            search_entry.connect_changed(move |_| {
                if let Some(v) = v.upgrade() {
                    v.run_search();
                }
            });
        }
        {
            let v = Rc::downgrade(&viewer);
            search_entry.connect_activate(move |_| {
                if let Some(v) = v.upgrade() {
                    v.step_match(true);
                }
            });
        }
        {
            let v = viewer.clone();
            next_btn.connect_clicked(move |_| v.step_match(true));
        }
        {
            let v = viewer.clone();
            prev_btn.connect_clicked(move |_| v.step_match(false));
        }
        viewer
    }

    /// True while the focus is in a text entry, where letters are text.
    fn typing(&self) -> bool {
        GtkWindowExt::focus(&self.window).is_some_and(|w| w.is::<gtk::Editable>())
    }

    /// Keys of the window. Runs in the capture phase so Shift+Enter in the
    /// search entry can step backwards.
    fn install_keys(self: &Rc<Self>) {
        let v = self.clone();
        let ctrl = gtk::EventControllerKey::new();
        ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        ctrl.connect_key_pressed(move |_, key, _, state| {
            let with_ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            if key == gdk::Key::Escape {
                if v.search_bar.is_visible() {
                    v.search_bar.set_visible(false);
                    if let Some(view) = v.text_view.borrow().as_ref() {
                        view.grab_focus();
                    }
                } else {
                    v.window.close();
                }
                return glib::Propagation::Stop;
            }
            if with_ctrl && matches!(key, gdk::Key::f | gdk::Key::F) {
                v.search_bar.set_visible(true);
                v.search_entry.grab_focus();
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::F3 {
                v.step_match(!shift);
                return glib::Propagation::Stop;
            }
            if v.typing() {
                let backwards = shift && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter);
                if backwards {
                    v.step_match(false);
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }
            let modified = with_ctrl || state.contains(gdk::ModifierType::ALT_MASK);
            match viewer_key(key, modified, v.show, v.showing_image()) {
                Some(ViewerKey::Next) => v.go(1),
                Some(ViewerKey::Prev) => v.go(-1),
                Some(ViewerKey::SetMode(mode)) => v.set_mode(mode),
                Some(ViewerKey::ToggleMark) => v.toggle_mark(),
                None => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.window.add_controller(ctrl);
    }

    /// Refresh the `MARKED` label and the count in the header (Show only).
    fn update_marks(&self) {
        if !self.show {
            return;
        }
        let marks = self.marks.borrow();
        let path = &self.files[self.index.get()];
        let prefix = if marks.is_marked(path) { "MARKED  \u{2022}  " } else { "" };
        self.marked.set_text(&format!("{prefix}{} marked", marks.len()));
    }

    /// Mark or unmark the current picture.
    fn toggle_mark(&self) {
        if !self.show {
            return;
        }
        self.marks.borrow_mut().toggle(self.files[self.index.get()].clone());
        self.update_marks();
    }

    /// Ask before closing a slideshow that has marked pictures. Nothing is
    /// deleted unless the user picks Delete.
    fn install_close_guard(self: &Rc<Self>) {
        let v = self.clone();
        self.window.connect_close_request(move |_| {
            if v.closing.get() || v.marks.borrow().is_empty() {
                return glib::Propagation::Proceed;
            }
            let files = v.marks.borrow().list();
            let message = format!("Delete {} marked file(s)?", files.len());
            let (viewer, ui) = (v.clone(), v.ui.clone());
            opusdm_ui::requester(&v.window, "Show", &message, None, &[("Delete", true), ("Keep", false)], move |delete| {
                if delete {
                    crate::job_submit::submit_job(
                        &ui,
                        opusdm_ipc::Op::Delete,
                        files.clone(),
                        Some(ui.cwd_path()),
                        opusdm_ipc::JobOptions::default(),
                    );
                }
                viewer.closing.set(true);
                viewer.window.close();
            });
            glib::Propagation::Stop
        });
    }

    /// True when the current file is shown as a picture (no text view).
    fn showing_image(&self) -> bool {
        self.text_view.borrow().is_none()
    }

    /// Move `delta` files, wrapping around.
    fn go(self: &Rc<Self>, delta: isize) {
        let n = self.files.len() as isize;
        let next = (self.index.get() as isize + delta).rem_euclid(n);
        self.index.set(next as usize);
        self.show_file();
    }

    /// Switch the mode and reload the current file.
    fn set_mode(self: &Rc<Self>, mode: Mode) {
        self.mode.set(mode);
        self.show_file();
    }

    /// Load the current file on a thread and render it when it arrives.
    fn show_file(self: &Rc<Self>) {
        let path = self.files[self.index.get()].clone();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.title.set_text(&format!("{}/{}  {}", self.index.get() + 1, self.files.len(), name));
        self.window.set_title(Some(&format!("{}: {name}", if self.show { "Show" } else { "Viewer" })));
        self.meta.set_text("Loading\u{2026}");
        self.update_marks();
        self.clear_content();

        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        let mode = self.mode.get();
        let (tx, rx) = async_channel::bounded::<ViewData>(1);
        let path_for_thread = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(load(&path_for_thread, mode));
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Ok(data) = rx.recv().await else { return };
            if this.generation.get() != gen {
                return;
            }
            this.apply(data);
        });
    }

    /// Remove what the content area shows.
    fn clear_content(&self) {
        *self.text_view.borrow_mut() = None;
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
    }

    /// Render loaded data.
    fn apply(self: &Rc<Self>, data: ViewData) {
        self.clear_content();
        match data {
            ViewData::Text { meta, text } => {
                self.meta.set_text(&meta);
                self.render_text(&text, None, gtk::WrapMode::WordChar);
            }
            ViewData::Ansi { meta, doc } => {
                self.meta.set_text(&meta);
                self.render_text(&doc.text, Some(&doc), gtk::WrapMode::None);
            }
            ViewData::Hex { meta, text } => {
                self.meta.set_text(&meta);
                self.render_text(&text, None, gtk::WrapMode::None);
            }
            ViewData::Image { bytes, meta } => self.render_image(&bytes, &meta),
            ViewData::Error { message } => {
                self.meta.set_text("Error");
                let label = gtk::Label::new(Some(&message));
                label.set_margin_top(20);
                self.content.append(&label);
            }
        }
        self.run_search();
    }

    /// Show `text` in a read-only monospace view, colouring it with `ansi`.
    fn render_text(&self, text: &str, ansi: Option<&AnsiDoc>, wrap: gtk::WrapMode) {
        let view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(wrap)
            .left_margin(8)
            .right_margin(8)
            .top_margin(8)
            .bottom_margin(8)
            .build();
        let buffer = view.buffer();
        buffer.set_text(text);
        if let Some(doc) = ansi {
            apply_ansi_tags(&buffer, doc);
        }
        // Created last, so they win over the ANSI colours.
        let _ = buffer.create_tag(Some(MATCH_TAG), &[("background", &"#d4b106"), ("foreground", &"#000000")]);
        let _ = buffer.create_tag(Some(CURRENT_TAG), &[("background", &"#ff7a00"), ("foreground", &"#000000")]);
        let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hexpand(true).build();
        self.content.append(&scroller);
        if !self.search_bar.is_visible() {
            view.grab_focus();
        }
        *self.text_view.borrow_mut() = Some(view);
    }

    /// Show a picture, falling back to its name and size if it cannot decode.
    fn render_image(&self, bytes: &[u8], meta: &str) {
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_vexpand(true);
        picture.set_hexpand(true);
        let text = match gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
            Ok(texture) => {
                let t = format!("{} \u{D7} {} px  \u{2022}  {meta}", texture.width(), texture.height());
                picture.set_paintable(Some(&texture));
                t
            }
            Err(_) => meta.to_string(),
        };
        self.meta.set_text(&text);
        self.content.append(&picture);
    }

    /// Highlight every match of the search text and move to the first.
    fn run_search(&self) {
        self.matches.borrow_mut().clear();
        self.current.set(None);
        let Some(view) = self.text_view.borrow().clone() else {
            self.search_count.set_text("");
            return;
        };
        let buffer = view.buffer();
        let (start, end) = buffer.bounds();
        buffer.remove_tag_by_name(MATCH_TAG, &start, &end);
        buffer.remove_tag_by_name(CURRENT_TAG, &start, &end);
        let query = self.search_entry.text().to_string();
        let found = find_matches(buffer.text(&start, &end, false).as_str(), &query);
        for (s, e) in &found {
            buffer.apply_tag_by_name(MATCH_TAG, &buffer.iter_at_offset(*s as i32), &buffer.iter_at_offset(*e as i32));
        }
        let first = step(None, found.len(), true);
        *self.matches.borrow_mut() = found;
        self.goto_match(first);
        if query.is_empty() {
            self.search_count.set_text("");
        }
    }

    /// Move the search to the next or previous match.
    fn step_match(&self, forward: bool) {
        let count = self.matches.borrow().len();
        self.goto_match(step(self.current.get(), count, forward));
    }

    /// Make match `index` the current one and scroll to it.
    fn goto_match(&self, index: Option<usize>) {
        let Some(view) = self.text_view.borrow().clone() else { return };
        let buffer = view.buffer();
        let (start, end) = buffer.bounds();
        buffer.remove_tag_by_name(CURRENT_TAG, &start, &end);
        self.current.set(index);
        let count = self.matches.borrow().len();
        let Some(i) = index else {
            self.search_count.set_text(if self.search_entry.text().is_empty() { "" } else { "0/0" });
            return;
        };
        let (s, e) = self.matches.borrow()[i];
        buffer.apply_tag_by_name(CURRENT_TAG, &buffer.iter_at_offset(s as i32), &buffer.iter_at_offset(e as i32));
        let mut iter = buffer.iter_at_offset(s as i32);
        view.scroll_to_iter(&mut iter, 0.1, false, 0.0, 0.0);
        let more = if count >= crate::viewer_search::MAX_MATCHES { "+" } else { "" };
        self.search_count.set_text(&format!("{}/{count}{more}", i + 1));
    }
}

/// Colour `buffer` from the spans of `doc`: a dark base, then one tag per
/// distinct style, created lazily.
fn apply_ansi_tags(buffer: &gtk::TextBuffer, doc: &AnsiDoc) {
    let base = buffer.create_tag(Some("ansi-base"), &[("foreground", &PALETTE[7]), ("background", &PALETTE[0])]);
    let (start, end) = buffer.bounds();
    if let Some(base) = base {
        buffer.apply_tag(&base, &start, &end);
    }
    let mut tags: HashMap<(Option<u8>, Option<u8>, bool), gtk::TextTag> = HashMap::new();
    for span in &doc.spans {
        let key = (span.fg, span.bg, span.bold);
        let tag = tags.entry(key).or_insert_with(|| {
            let tag = gtk::TextTag::new(None);
            if let Some(fg) = span.fg {
                tag.set_foreground(Some(PALETTE[fg as usize]));
            }
            if let Some(bg) = span.bg {
                tag.set_background(Some(PALETTE[bg as usize]));
            }
            if span.bold {
                tag.set_weight(700);
            }
            buffer.tag_table().add(&tag);
            tag
        });
        buffer.apply_tag(tag, &buffer.iter_at_offset(span.start as i32), &buffer.iter_at_offset(span.end as i32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: gdk::Key) -> Option<ViewerKey> {
        viewer_key(k, false, false, false)
    }

    #[test]
    fn plain_letters_page_and_switch_modes_in_either_case() {
        assert_eq!(key(gdk::Key::n), Some(ViewerKey::Next));
        assert_eq!(key(gdk::Key::N), Some(ViewerKey::Next));
        assert_eq!(key(gdk::Key::P), Some(ViewerKey::Prev));
        assert_eq!(key(gdk::Key::a), Some(ViewerKey::SetMode(Mode::Ansi)));
        assert_eq!(key(gdk::Key::X), Some(ViewerKey::SetMode(Mode::Hex)));
        assert_eq!(key(gdk::Key::T), Some(ViewerKey::SetMode(Mode::Text)));
        assert_eq!(key(gdk::Key::S), Some(ViewerKey::SetMode(Mode::Smart)));
    }

    #[test]
    fn ctrl_or_alt_chords_are_left_alone_so_select_all_and_copy_work() {
        for k in [gdk::Key::a, gdk::Key::x, gdk::Key::s, gdk::Key::n, gdk::Key::p, gdk::Key::m] {
            assert_eq!(viewer_key(k, true, false, false), None, "{k:?} with a modifier");
        }
    }

    #[test]
    fn arrows_page_only_while_a_picture_is_shown() {
        assert_eq!(viewer_key(gdk::Key::Right, false, false, true), Some(ViewerKey::Next));
        assert_eq!(viewer_key(gdk::Key::Left, false, false, true), Some(ViewerKey::Prev));
        assert_eq!(viewer_key(gdk::Key::Right, false, false, false), None, "arrows scroll text");
    }

    #[test]
    fn mode_keys_are_ignored_in_show_and_marking_only_works_there() {
        assert_eq!(viewer_key(gdk::Key::a, false, true, true), None);
        assert_eq!(viewer_key(gdk::Key::m, false, true, true), Some(ViewerKey::ToggleMark));
        assert_eq!(viewer_key(gdk::Key::Delete, false, true, true), Some(ViewerKey::ToggleMark));
        assert_eq!(viewer_key(gdk::Key::m, false, false, false), None);
        assert_eq!(viewer_key(gdk::Key::Delete, false, false, false), None);
    }
}
