//! The thumbnail strip: one button per file, thumbnails made by a worker thread.

use crate::kind::{kind_or_image, MediaKind};
use crate::loader::{load_thumb, RawImage};
use crate::pdf::render_thumb;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;

/// Edge of a thumbnail in pixels.
const THUMB_PX: i32 = 96;
/// Files on each side of the current one whose thumbnails are requested.
const THUMB_RADIUS: usize = 12;
/// Longest extension written on the placeholder of a source text.
const SOURCE_LABEL_CHARS: usize = 5;

type ThumbResult = (PathBuf, Result<RawImage, String>);

/// The strip widget and its thumbnail bookkeeping.
pub struct Strip {
    pub root: gtk::ScrolledWindow,
    row: gtk::Box,
    buttons: RefCell<Vec<(PathBuf, gtk::Button)>>,
    requested: RefCell<HashSet<PathBuf>>,
    current: RefCell<Option<PathBuf>>,
    requests: mpsc::Sender<PathBuf>,
    /// Where a click on a thumbnail sends its path.
    picked: async_channel::Sender<PathBuf>,
}

impl Strip {
    /// Build the strip for `items`; a click sends the path through `picked`.
    pub fn new(items: &[PathBuf], picked: async_channel::Sender<PathBuf>) -> Rc<Self> {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.set_margin_start(4);
        row.set_margin_end(4);
        row.set_margin_top(4);
        row.set_margin_bottom(4);
        let root = gtk::ScrolledWindow::new();
        root.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
        root.set_child(Some(&row));
        let (requests, results) = _start_worker();
        let this = Rc::new(Strip { root, row, buttons: RefCell::new(Vec::new()), requested: RefCell::new(HashSet::new()), current: RefCell::new(None), requests, picked });
        this.set_items(items);
        let weak = Rc::downgrade(&this);
        glib::spawn_future_local(async move {
            while let Ok((path, result)) = results.recv().await {
                let Some(this) = weak.upgrade() else { break };
                this._set_thumb(&path, result);
            }
        });
        this
    }

    /// Highlight `path`, scroll it to the centre and ask for the thumbnails around it.
    pub fn set_current(&self, path: &Path) {
        let old = self.current.borrow_mut().replace(path.to_path_buf());
        let buttons = self.buttons.borrow();
        if let Some(old) = old {
            if let Some((_, b)) = buttons.iter().find(|(p, _)| *p == old) { b.remove_css_class("viewer-thumb-current"); }
        }
        let Some(index) = buttons.iter().position(|(p, _)| p == path) else { return };
        buttons[index].1.add_css_class("viewer-thumb-current");
        let first = index.saturating_sub(THUMB_RADIUS);
        let last = (index + THUMB_RADIUS).min(buttons.len() - 1);
        for (p, _) in &buttons[first..=last] {
            self._request(p);
        }
        let (root, button) = (self.root.clone(), buttons[index].1.clone());
        // The layout is not final yet when the file just changed: centre on idle.
        glib::idle_add_local_once(move || _center(&root, &button));
    }

    /// Show `items` in place of the files of before.
    pub fn set_items(&self, items: &[PathBuf]) {
        for (_, button) in self.buttons.borrow_mut().drain(..) {
            self.row.remove(&button);
        }
        // A file shown again needs its thumbnail asked for again.
        self.requested.borrow_mut().clear();
        self.current.borrow_mut().take();
        for path in items {
            self._add_button(path, self.picked.clone());
        }
    }

    /// Drop the button of a deleted file.
    pub fn remove(&self, path: &Path) {
        let mut buttons = self.buttons.borrow_mut();
        let Some(index) = buttons.iter().position(|(p, _)| p == path) else { return };
        let (_, button) = buttons.remove(index);
        self.row.remove(&button);
    }

    /// One button; videos get a text placeholder, pictures wait for their thumbnail.
    fn _add_button(&self, path: &Path, picked: async_channel::Sender<PathBuf>) {
        let button = gtk::Button::new();
        button.set_focusable(false);
        button.set_size_request(THUMB_PX + 8, THUMB_PX * 3 / 4 + 8);
        let text = match kind_or_image(path) {
            MediaKind::Video => "VIDEO".to_string(),
            MediaKind::Markdown => "MD".to_string(),
            MediaKind::Source => _source_label(path),
            _ => "...".to_string(),
        };
        button.set_child(Some(&gtk::Label::new(Some(&text))));
        button.set_tooltip_text(path.file_name().map(|n| n.to_string_lossy()).as_deref());
        let target = path.to_path_buf();
        button.connect_clicked(move |_| { let _ = picked.try_send(target.clone()); });
        self.row.append(&button);
        self.buttons.borrow_mut().push((path.to_path_buf(), button));
    }

    /// Ask the worker for a thumbnail, once per picture.
    fn _request(&self, path: &Path) {
        if matches!(kind_or_image(path), MediaKind::Video | MediaKind::Markdown | MediaKind::Source) { return; }
        if !self.requested.borrow_mut().insert(path.to_path_buf()) { return; }
        if self.requests.send(path.to_path_buf()).is_err() { opusdm_ipc::err_log!("viewer", "the thumbnail worker stopped"); }
    }

    /// Show a finished thumbnail; a failure leaves a `?`.
    fn _set_thumb(&self, path: &Path, result: Result<RawImage, String>) {
        let buttons = self.buttons.borrow();
        let Some((_, button)) = buttons.iter().find(|(p, _)| p == path) else { return };
        match result {
            Ok(raw) => {
                let picture = gtk::Picture::for_paintable(&raw.texture(0));
                picture.set_content_fit(gtk::ContentFit::Contain);
                button.set_child(Some(&picture));
            }
            Err(e) => {
                opusdm_ipc::err_log!("viewer", "thumbnail of {}: {e}", path.display());
                button.set_child(Some(&gtk::Label::new(Some("?"))));
            }
        }
    }
}

/// Placeholder of a source text: its extension in capitals (`RS`), or `TEXT`.
fn _source_label(path: &Path) -> String {
    let ext = path.extension().map(|e| e.to_string_lossy().to_uppercase()).filter(|e| !e.is_empty() && e.chars().count() <= SOURCE_LABEL_CHARS);
    ext.unwrap_or_else(|| "TEXT".to_string())
}

/// The worker thread: makes the thumbnails in the order they are asked and stops
/// when the strip (the sender) is dropped.
fn _start_worker() -> (mpsc::Sender<PathBuf>, async_channel::Receiver<ThumbResult>) {
    let (req_tx, req_rx) = mpsc::channel::<PathBuf>();
    let (res_tx, res_rx) = async_channel::unbounded::<ThumbResult>();
    std::thread::spawn(move || {
        for path in req_rx {
            let result = match kind_or_image(&path) {
                MediaKind::Pdf => render_thumb(&path, THUMB_PX),
                _ => load_thumb(&path, THUMB_PX),
            };
            if res_tx.send_blocking((path, result)).is_err() { break; }
        }
    });
    (req_tx, res_rx)
}

/// Scroll so that `button` is in the middle of the strip.
fn _center(root: &gtk::ScrolledWindow, button: &gtk::Button) {
    let adj = root.hadjustment();
    let target = button.allocation().x() as f64 - (adj.page_size() - button.width() as f64) / 2.0;
    adj.set_value(target.clamp(adj.lower(), (adj.upper() - adj.page_size()).max(adj.lower())));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_placeholder_is_its_short_extension() {
        assert_eq!(_source_label(Path::new("a/main.rs")), "RS");
        assert_eq!(_source_label(Path::new("Makefile")), "TEXT");
        assert_eq!(_source_label(Path::new("x.properties")), "TEXT");
    }
}
