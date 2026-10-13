//! The viewer window: stage, thumbnail strip and info bar; keys, full screen,
//! slideshow and delete. The PDF bar, page panel, search and links are in `pdf`.

mod pdf;

use crate::delete::{remove_permanently, try_trash, SystemRemover, TrashStep};
use crate::gallery::Gallery;
use crate::image_view::ImageView;
use crate::keys::{map, Action};
use crate::kind::{kind_or_image, MediaKind};
use crate::loader::{load_raw, RawImage};
use crate::markdown_view::MarkdownView;
use crate::pdf::{start_worker, PageAnswer, PageRequest};
use pdf::PdfUi;
use crate::slideshow::{Slideshow, SLIDESHOW_SECS};
use crate::strip::Strip;
use crate::video_view::{VideoNotice, VideoView};
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

/// Seconds a Left / Right key moves a video.
const SKIP_SECS: i64 = 5;
const DEFAULT_WIDTH: i32 = 1000;
const DEFAULT_HEIGHT: i32 = 720;

/// Answer of a decoding thread: the generation it was started for and the picture.
type Loaded = (u64, Result<RawImage, String>);

/// Everything the window needs, shared by the event handlers.
struct State {
    window: gtk::ApplicationWindow,
    gallery: RefCell<Gallery>,
    stack: gtk::Stack,
    image: Rc<ImageView>,
    video: Rc<VideoView>,
    markdown: Rc<MarkdownView>,
    error: gtk::Label,
    info: gtk::Label,
    strip: Rc<Strip>,
    slideshow: RefCell<Slideshow>,
    slide_timer: RefCell<Option<glib::SourceId>>,
    /// Bumped for every shown file; an answer for an older one is dropped.
    generation: Cell<u64>,
    load_tx: async_channel::Sender<Loaded>,
    /// Requests to the PDF page worker.
    page_tx: mpsc::Sender<PageRequest>,
    /// Page on screen (0-based) and page count of a PDF; the count is 0 until
    /// the first page arrives.
    page: Cell<usize>,
    pages: Cell<usize>,
    /// The next picture is another page of the same PDF: keep zoom and rotation.
    keep_zoom: Cell<bool>,
    pdf: PdfUi,
}

/// Open the window on `gallery` and show its current file.
pub fn open(app: &gtk::Application, gallery: Gallery) {
    let window = gtk::ApplicationWindow::builder().application(app).default_width(DEFAULT_WIDTH).default_height(DEFAULT_HEIGHT).build();
    let (load_tx, load_rx) = async_channel::unbounded();
    let (video_tx, video_rx) = async_channel::unbounded();
    let (pick_tx, pick_rx) = async_channel::unbounded();
    let (answer_tx, answer_rx) = async_channel::unbounded();
    let page_tx = start_worker(answer_tx);
    let (pdf_ui, pdf_channels) = pdf::build();

    let image = ImageView::new();
    let video = VideoView::new(video_tx);
    let markdown = MarkdownView::new();
    let error = gtk::Label::new(None);
    error.set_wrap(true);
    error.set_vexpand(true);
    error.add_css_class("viewer-stage");
    error.add_css_class("viewer-error");
    let video_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    video_page.append(&video.stage);
    video_page.append(&video.bar);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&image.root, Some("image"));
    stack.add_named(&video_page, Some("video"));
    stack.add_named(&markdown.root, Some("markdown"));
    stack.add_named(&error, Some("error"));
    let strip = Strip::new(gallery.items(), pick_tx);
    let info = gtk::Label::new(None);
    info.set_xalign(0.0);
    info.add_css_class("viewer-info");

    let middle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    middle.append(&pdf_ui.panel.root);
    middle.append(&stack);
    stack.set_hexpand(true);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&pdf_ui.bar.root);
    vbox.append(&middle);
    vbox.append(&strip.root);
    vbox.append(&info);
    window.set_child(Some(&vbox));

    let state = Rc::new(State {
        window: window.clone(),
        gallery: RefCell::new(gallery),
        stack,
        image,
        video,
        markdown,
        error,
        info,
        strip,
        slideshow: RefCell::new(Slideshow::default()),
        slide_timer: RefCell::new(None),
        generation: Cell::new(0),
        load_tx,
        page_tx,
        page: Cell::new(0),
        pages: Cell::new(0),
        keep_zoom: Cell::new(false),
        pdf: pdf_ui,
    });
    state._wire_keys();
    state._wire_pdf(pdf_channels);
    state._sync_strip();
    state._wire_fullscreen();
    _spawn_loops(&state, load_rx, answer_rx, video_rx, pick_rx);
    window.present();
    state.show_current();
}

/// Handle what the worker threads and the other widgets send, on the main thread.
fn _spawn_loops(state: &Rc<State>, load_rx: async_channel::Receiver<Loaded>, answer_rx: async_channel::Receiver<PageAnswer>, video_rx: async_channel::Receiver<VideoNotice>, pick_rx: async_channel::Receiver<PathBuf>) {
    let s = state.clone();
    glib::spawn_future_local(async move {
        while let Ok((generation, result)) = load_rx.recv().await {
            if generation == s.generation.get() { s._picture_loaded(result); }
        }
    });
    let s = state.clone();
    glib::spawn_future_local(async move {
        while let Ok((generation, result)) = answer_rx.recv().await {
            if generation == s.generation.get() { s._pdf_page_loaded(result); }
        }
    });
    let s = state.clone();
    glib::spawn_future_local(async move {
        while let Ok(notice) = video_rx.recv().await {
            s._video_notice(notice);
        }
    });
    let s = state.clone();
    glib::spawn_future_local(async move {
        while let Ok(path) = pick_rx.recv().await {
            let jumped = {
                let mut g = s.gallery.borrow_mut();
                g.position(&path).is_some_and(|i| g.jump_to(i))
            };
            if jumped { s.show_current(); }
        }
    });
}

/// Name of a file for titles and messages.
fn _name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string())
}

impl State {
    /// Path, kind and position text of the file on screen.
    fn _current(&self) -> Option<(PathBuf, MediaKind)> {
        let g = self.gallery.borrow();
        let path = g.current()?.to_path_buf();
        let kind = kind_or_image(&path);
        Some((path, kind))
    }

    /// Show the gallery's current file: start the decoding of a picture, the
    /// rendering of the first page of a PDF or the playback of a video.
    fn show_current(self: &Rc<Self>) {
        let Some((path, kind)) = self._current() else { return };
        self.generation.set(self.generation.get() + 1);
        self._cancel_timer();
        self.window.set_title(Some(&_name(&path)));
        self.strip.set_current(&path);
        self.page.set(0);
        self.pages.set(0);
        self.keep_zoom.set(false);
        self._pdf_file_changed();
        match kind {
            MediaKind::Image => {
                self.video.stop();
                let (tx, generation) = (self.load_tx.clone(), self.generation.get());
                std::thread::spawn(move || {
                    let _ = tx.send_blocking((generation, load_raw(&path)));
                });
            }
            MediaKind::Pdf => {
                self.video.stop();
                self._request_page(&path);
            }
            MediaKind::Markdown => {
                self.video.stop();
                match self.markdown.show(&path) {
                    Ok(()) => {
                        self.stack.set_visible_child_name("markdown");
                        self._schedule_slideshow();
                    }
                    Err(e) => {
                        opusdm_ipc::err_log!("viewer", "cannot show {}: {e}", path.display());
                        self._show_error(&format!("Cannot show {}: {e}", _name(&path)));
                    }
                }
            }
            MediaKind::Video => match self.video.open(&path) {
                Ok(()) => self.stack.set_visible_child_name("video"),
                Err(e) => self._show_error(&format!("Cannot play {}: {e}", _name(&path))),
            },
        }
        self._update_info();
    }

    /// Ask the page worker for the current page of the PDF on screen.
    fn _request_page(&self, path: &Path) {
        let request = PageRequest { generation: self.generation.get(), path: path.to_path_buf(), index: self.page.get() };
        if self.page_tx.send(request).is_err() { self._show_error("The PDF renderer stopped"); }
    }

    /// A decoding thread (or the page worker) finished for the file on screen.
    fn _picture_loaded(self: &Rc<Self>, result: Result<RawImage, String>) {
        match result {
            Ok(raw) => {
                if self.keep_zoom.get() { self.image.replace_image(raw, true); } else { self.image.set_image(raw); }
                self.stack.set_visible_child_name("image");
            }
            Err(e) => {
                let name = self._current().map(|(p, _)| _name(&p)).unwrap_or_default();
                opusdm_ipc::err_log!("viewer", "cannot load {name}: {e}");
                self._show_error(&format!("Cannot load {name}: {e}"));
            }
        }
        self._update_info();
        self._schedule_slideshow();
    }

    /// The engine said something about the video on screen.
    fn _video_notice(self: &Rc<Self>, notice: VideoNotice) {
        match notice {
            VideoNotice::Ended => {
                if self.slideshow.borrow().advance_on_end() { self._step(true); }
            }
            VideoNotice::Failed(message) => {
                let name = self._current().map(|(p, _)| _name(&p)).unwrap_or_default();
                self._show_error(&format!("Cannot play {name}: {message}"));
            }
        }
    }

    /// Replace the stage with a message; the keys keep working.
    fn _show_error(&self, message: &str) {
        self.error.set_text(message);
        self.stack.set_visible_child_name("error");
        self._update_info();
    }

    /// Move to the next (or previous) file.
    fn _step(self: &Rc<Self>, forward: bool) {
        {
            let mut g = self.gallery.borrow_mut();
            if forward { g.next(); } else { g.prev(); }
        }
        self.show_current();
    }

    /// `name - 3/12 - 1920x1080 - 125%` (with `page 2/10` for a PDF), plus a slideshow mark.
    fn _update_info(&self) {
        let Some((path, kind)) = self._current() else { return };
        let (index, len) = { let g = self.gallery.borrow(); (g.index() + 1, g.len()) };
        let position = if len > 1 { format!("{index}/{len}  -  ") } else { String::new() };
        let detail = match (kind, self.stack.visible_child_name().as_deref()) {
            (_, Some("error")) => "error".to_string(),
            (MediaKind::Video, _) => "video".to_string(),
            (MediaKind::Markdown, _) => "markdown".to_string(),
            (MediaKind::Pdf, _) if self.pages.get() == 0 => "loading".to_string(),
            (MediaKind::Image | MediaKind::Pdf, _) => match self.image.size() {
                Some((w, h)) => format!("{}{w}x{h}  -  {}{}", self._page_label(kind), self.image.zoom_label(), if self.image.is_smooth() { "" } else { "  -  pixels" }),
                None => "loading".to_string(),
            },
        };
        let mark = if self.slideshow.borrow().is_on() { "  -  slideshow" } else { "" };
        self.info.set_text(&format!("{}  -  {position}{detail}{mark}", _name(&path)));
    }

    /// `page 2/10  -  ` for a PDF, nothing for a picture.
    fn _page_label(&self, kind: MediaKind) -> String {
        if kind != MediaKind::Pdf { return String::new(); }
        format!("page {}/{}  -  ", self.page.get() + 1, self.pages.get())
    }

    /// The thumbnail strip shows when there is more than one file, out of full screen.
    fn _sync_strip(&self) {
        let several = self.gallery.borrow().len() > 1;
        self.strip.root.set_visible(several && !self.window.is_fullscreen());
    }

    /// Stop the slideshow timer, if one is running.
    fn _cancel_timer(&self) {
        if let Some(id) = self.slide_timer.borrow_mut().take() { id.remove(); }
    }

    /// Start the one shot timer that advances to the next file, when the
    /// slideshow wants one for the file on screen.
    fn _schedule_slideshow(self: &Rc<Self>) {
        self._cancel_timer();
        let Some((_, kind)) = self._current() else { return };
        if !self.slideshow.borrow().wants_timer(kind) { return; }
        let this = self.clone();
        let id = glib::timeout_add_local_once(Duration::from_secs(SLIDESHOW_SECS), move || {
            this.slide_timer.borrow_mut().take();
            this._step(true);
        });
        *self.slide_timer.borrow_mut() = Some(id);
    }

    /// Key controller on the window, capture phase: no focused widget takes the keys.
    fn _wire_keys(self: &Rc<Self>) {
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = self.clone();
        key.connect_key_pressed(move |_, k, _, mods| {
            if let Some(propagation) = this._pdf_key(k, mods) { return propagation; }
            if mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) { return glib::Propagation::Proceed; }
            let Some((_, kind)) = this._current() else { return glib::Propagation::Proceed };
            let Some(action) = map(k, kind) else { return glib::Propagation::Proceed };
            this._run(action);
            glib::Propagation::Stop
        });
        self.window.add_controller(key);
    }

    /// Do what a key asked for.
    fn _run(self: &Rc<Self>, action: Action) {
        match action {
            Action::ZoomIn => self.image.zoom_in(),
            Action::ZoomOut => self.image.zoom_out(),
            Action::Fit => self.image.fit(),
            Action::Actual => self.image.actual(),
            Action::RotateCw => self.image.rotate_cw(),
            Action::RotateCcw => self.image.rotate_ccw(),
            Action::Interpolation => self.image.toggle_smooth(),
            Action::NextPage | Action::PrevPage | Action::FirstPage | Action::LastPage => return self._turn_page(action),
            Action::TogglePages => self._toggle_pages(),
            Action::Next => return self._step(true),
            Action::Prev => return self._step(false),
            Action::SeekBack => self.video.skip(-SKIP_SECS),
            Action::SeekForward => self.video.skip(SKIP_SECS),
            Action::TogglePlay => self.video.toggle_play(),
            Action::VolumeUp => self.video.change_volume(true),
            Action::VolumeDown => self.video.change_volume(false),
            Action::Mute => self.video.toggle_mute(),
            Action::Slideshow => {
                self.slideshow.borrow_mut().toggle();
                self._schedule_slideshow();
            }
            Action::Fullscreen => self.window.set_fullscreened(!self.window.is_fullscreen()),
            Action::Delete => return self._ask_delete(),
            Action::Escape if self.window.is_fullscreen() => self.window.set_fullscreened(false),
            Action::Escape | Action::Quit => self.window.close(),
        }
        self._update_info();
    }

    /// Double click toggles full screen; in full screen the bars are hidden.
    fn _wire_fullscreen(self: &Rc<Self>) {
        let click = gtk::GestureClick::new();
        let w = self.window.clone();
        click.connect_pressed(move |_, n, _, _| {
            if n == 2 { w.set_fullscreened(!w.is_fullscreen()); }
        });
        self.stack.add_controller(click);
        let this = self.clone();
        self.window.connect_fullscreened_notify(move |w| {
            let visible = !w.is_fullscreen();
            this._sync_strip();
            this.info.set_visible(visible);
            this.video.bar.set_visible(visible);
            this._sync_pdf_panels();
        });
    }

    /// Ask before moving the file on screen to the trash.
    fn _ask_delete(self: &Rc<Self>) {
        let Some((path, _)) = self._current() else { return };
        let this = self.clone();
        opusdm_ui::requester(&self.window, "Delete", &format!("Move \"{}\" to the trash?", _name(&path)), None, &[("Trash", true), ("Cancel", false)], move |yes| {
            if yes { this._trash(&path); }
        });
    }

    /// Trash the file; when there is no trash ask for a permanent delete.
    fn _trash(self: &Rc<Self>, path: &Path) {
        self.video.stop();
        let reason = match try_trash(&SystemRemover, path) {
            TrashStep::Trashed => return self._removed(path),
            TrashStep::AskPermanent(reason) => reason,
        };
        let (this, target) = (self.clone(), path.to_path_buf());
        opusdm_ui::requester(&self.window, "Delete", "The file cannot be moved to the trash. Delete it permanently?", Some(&reason), &[("Delete", true), ("Cancel", false)], move |yes| {
            if !yes { return this.show_current(); }
            match remove_permanently(&SystemRemover, &target) {
                Ok(()) => this._removed(&target),
                Err(e) => this._delete_failed(&target, &e),
            }
        });
    }

    /// Tell the user the file is still there.
    fn _delete_failed(self: &Rc<Self>, path: &Path, reason: &str) {
        opusdm_ipc::err_log!("viewer", "cannot delete {}: {reason}", path.display());
        let this = self.clone();
        opusdm_ui::requester(&self.window, "Delete", &format!("Cannot delete {}", _name(path)), Some(reason), &[("Close", ())], move |_| this.show_current());
    }

    /// The file is gone: leave the gallery and show the next one.
    fn _removed(self: &Rc<Self>, path: &Path) {
        self.strip.remove(path);
        let empty = {
            let mut g = self.gallery.borrow_mut();
            if g.current() == Some(path) { g.remove_current(); }
            g.is_empty()
        };
        if empty { return self.window.close(); }
        self._sync_strip();
        self.show_current();
    }
}
