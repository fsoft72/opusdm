//! The page panel of a PDF: one thumbnail button per page, in a column on the
//! left. The thumbnails are rendered by a worker thread, only once the panel
//! is shown, and a new document stops the worker of the old one.

use crate::loader::RawImage;
use crate::pdf::{open, render_page_cached};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Width of a page thumbnail in pixels.
const THUMB_WIDTH: i32 = 110;
/// Height of the place kept for a thumbnail not rendered yet (A4 ratio).
const PLACEHOLDER_HEIGHT: i32 = 155;

/// A finished thumbnail: document generation, page index and picture.
type ThumbAnswer = (u64, usize, Result<RawImage, String>);

/// The panel widget and its pages.
pub struct PagePanel {
    pub root: gtk::ScrolledWindow,
    column: gtk::Box,
    buttons: RefCell<Vec<gtk::Button>>,
    /// The document shown and whether its thumbnails were asked for.
    doc: RefCell<Option<PathBuf>>,
    started: Cell<bool>,
    current: Cell<Option<usize>>,
    /// Bumped for every document; a worker of an older one stops.
    generation: Arc<AtomicU64>,
    picked: async_channel::Sender<usize>,
    answers: async_channel::Sender<ThumbAnswer>,
}

impl PagePanel {
    /// Build the empty panel; a click on a page sends its index through `picked`.
    pub fn new(picked: async_channel::Sender<usize>) -> Rc<Self> {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.set_margin_start(6);
        column.set_margin_end(6);
        column.set_margin_top(6);
        column.set_margin_bottom(6);
        let root = gtk::ScrolledWindow::new();
        root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        root.set_child(Some(&column));
        root.set_size_request(THUMB_WIDTH + 40, -1);
        root.add_css_class("viewer-pages");
        let (answers, results) = async_channel::unbounded::<ThumbAnswer>();
        let this = Rc::new(PagePanel { root, column, buttons: RefCell::new(Vec::new()), doc: RefCell::new(None), started: Cell::new(false), current: Cell::new(None), generation: Arc::new(AtomicU64::new(0)), picked, answers });
        let weak = Rc::downgrade(&this);
        glib::spawn_future_local(async move {
            while let Ok((generation, index, result)) = results.recv().await {
                let Some(this) = weak.upgrade() else { break };
                if generation == this.generation.load(Ordering::SeqCst) { this._set_thumb(index, result); }
            }
        });
        this
    }

    /// Show the pages of `path` (`count` of them); nothing changes when it is
    /// already the document shown.
    pub fn load(&self, path: &Path, count: usize) {
        if self.doc.borrow().as_deref() == Some(path) && self.buttons.borrow().len() == count { return; }
        self.clear();
        *self.doc.borrow_mut() = Some(path.to_path_buf());
        let mut buttons = self.buttons.borrow_mut();
        for index in 0..count {
            let button = self._page_button(index);
            self.column.append(&button);
            buttons.push(button);
        }
    }

    /// Drop the pages and stop the worker.
    pub fn clear(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        for button in self.buttons.borrow_mut().drain(..) {
            self.column.remove(&button);
        }
        *self.doc.borrow_mut() = None;
        self.started.set(false);
        self.current.set(None);
    }

    /// Start rendering the thumbnails, once per document (call it when the panel shows).
    pub fn start_thumbnails(&self) {
        if self.started.get() { return; }
        let Some(path) = self.doc.borrow().clone() else { return };
        self.started.set(true);
        let count = self.buttons.borrow().len();
        let generation = self.generation.load(Ordering::SeqCst);
        let (flag, answers) = (self.generation.clone(), self.answers.clone());
        let first = self.current.get().unwrap_or(0);
        std::thread::spawn(move || _render_thumbs(&path, count, first, generation, &flag, &answers));
    }

    /// Highlight page `index` and scroll it into view.
    pub fn set_current(&self, index: usize) {
        let buttons = self.buttons.borrow();
        if let Some(old) = self.current.replace(Some(index)) {
            if let Some(b) = buttons.get(old) { b.remove_css_class("viewer-thumb-current"); }
        }
        let Some(button) = buttons.get(index) else { return };
        button.add_css_class("viewer-thumb-current");
        let (root, button) = (self.root.clone(), button.clone());
        glib::idle_add_local_once(move || _scroll_into_view(&root, &button));
    }

    /// One page: a picture place and the page number under it.
    fn _page_button(&self, index: usize) -> gtk::Button {
        let picture = gtk::Picture::new();
        picture.set_size_request(THUMB_WIDTH, PLACEHOLDER_HEIGHT);
        picture.set_content_fit(gtk::ContentFit::Contain);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        column.append(&picture);
        column.append(&gtk::Label::new(Some(&(index + 1).to_string())));
        let button = gtk::Button::new();
        button.set_focusable(false);
        button.set_child(Some(&column));
        let picked = self.picked.clone();
        button.connect_clicked(move |_| { let _ = picked.try_send(index); });
        button
    }

    /// Put a finished thumbnail in its button; a failure leaves the empty place.
    fn _set_thumb(&self, index: usize, result: Result<RawImage, String>) {
        let buttons = self.buttons.borrow();
        let Some(button) = buttons.get(index) else { return };
        let raw = match result {
            Ok(raw) => raw,
            Err(e) => return opusdm_ipc::err_log!("viewer", "page {} thumbnail: {e}", index + 1),
        };
        let Some(picture) = button.child().and_then(|c| c.first_child()).and_then(|w| w.downcast::<gtk::Picture>().ok()) else { return };
        picture.set_size_request(THUMB_WIDTH, (THUMB_WIDTH as f64 * raw.height as f64 / raw.width.max(1) as f64).round() as i32);
        picture.set_paintable(Some(&raw.texture(0)));
    }
}

/// Order the pages are rendered in: from `first` to the end, then the ones before it.
fn _render_order(count: usize, first: usize) -> impl Iterator<Item = usize> {
    let first = first.min(count);
    (first..count).chain(0..first)
}

/// Worker body: render every page small, stop when the generation changes.
fn _render_thumbs(path: &Path, count: usize, first: usize, generation: u64, flag: &AtomicU64, answers: &async_channel::Sender<ThumbAnswer>) {
    let pdf = match open(path) {
        Ok(pdf) => pdf,
        Err(e) => return opusdm_ipc::err_log!("viewer", "page thumbnails of {}: {e}", path.display()),
    };
    let cache = hayro::RenderCache::new();
    for index in _render_order(count, first) {
        if flag.load(Ordering::SeqCst) != generation { return; }
        let Some(page) = pdf.pages().get(index) else { return };
        let scale = THUMB_WIDTH as f32 / page.render_dimensions().0.max(1.0);
        let result = render_page_cached(&pdf, index, scale, &cache).map(|p| p.raw);
        if answers.send_blocking((generation, index, result)).is_err() { return; }
    }
}

/// Scroll the panel so that `button` is fully visible.
fn _scroll_into_view(root: &gtk::ScrolledWindow, button: &gtk::Button) {
    let adj = root.vadjustment();
    let (top, height) = (button.allocation().y() as f64, button.height() as f64);
    if top < adj.value() {
        adj.set_value(top);
    } else if top + height > adj.value() + adj.page_size() {
        adj.set_value(top + height - adj.page_size());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_order_starts_at_the_current_page() {
        assert_eq!(_render_order(5, 2).collect::<Vec<_>>(), vec![2, 3, 4, 0, 1]);
        assert_eq!(_render_order(3, 0).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(_render_order(3, 9).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(_render_order(0, 0).count(), 0);
    }

    #[test]
    fn the_worker_stops_when_the_generation_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        std::fs::write(&path, crate::pdf::tests::pdf_with_contents(&["", "", ""])).unwrap();
        let (tx, rx) = async_channel::unbounded();
        let flag = AtomicU64::new(1);
        _render_thumbs(&path, 3, 1, 1, &flag, &tx);
        let pages: Vec<usize> = std::iter::from_fn(|| rx.try_recv().ok()).map(|(_, i, r)| { assert_eq!(r.unwrap().width, THUMB_WIDTH as u32); i }).collect();
        assert_eq!(pages, vec![1, 2, 0]);
        _render_thumbs(&path, 3, 0, 0, &flag, &tx);
        assert!(rx.try_recv().is_err(), "an old generation renders nothing");
    }
}
