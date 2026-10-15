//! The source area: a read-only text view with the line numbers on its left.
//! The text shows at once; a thread colours it a block of lines at a time and
//! the colours are applied as they arrive, so a big file can be read while
//! the rest is still being coloured.

use crate::font_scale::DOC_TEXT_CLASS;
use crate::highlight::{highlight_in_batches, syntax_for, CharSpan};
use crate::line_numbers::LineNumbers;
use crate::text_doc::{create_match_tags, create_syntax_tags, set_doc_dark, TextDoc, TextSearch};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Biggest file shown; a larger one is more likely not a text to read.
const MAX_BYTES: u64 = 8 * 1024 * 1024;
/// Margin around the text, in pixels.
const PAGE_MARGIN: i32 = 12;
/// Lines coloured between two updates of the view.
const BATCH_LINES: usize = 500;

/// Spans of the colouring thread for the file of a generation.
type Coloured = (u64, Vec<CharSpan>);

/// The scrolled source text, its line numbers and its colouring.
pub struct SourceView {
    /// Put this in the stack.
    pub root: gtk::ScrolledWindow,
    view: gtk::TextView,
    numbers: LineNumbers,
    search: TextSearch,
    /// Bumped for every shown file; the colouring thread of an older one stops.
    generation: Arc<AtomicU64>,
    coloured_tx: async_channel::Sender<Coloured>,
    /// Syntax name (or `plain text`) and line count of the file on screen.
    syntax_name: RefCell<String>,
    lines: Cell<i32>,
}

impl SourceView {
    /// An empty view; the colours of the files arrive on the main loop.
    pub fn new() -> Rc<SourceView> {
        let view = _text_view();
        view.set_left_margin(PAGE_MARGIN);
        view.set_right_margin(PAGE_MARGIN);
        create_syntax_tags(&view.buffer());
        create_match_tags(&view.buffer());
        let root = gtk::ScrolledWindow::new();
        root.set_vexpand(true);
        root.set_child(Some(&view));
        let numbers = LineNumbers::attach(&view);
        let (coloured_tx, coloured_rx) = async_channel::unbounded();
        let this = Rc::new(SourceView { root, view, numbers, search: TextSearch::default(), generation: Arc::new(AtomicU64::new(0)), coloured_tx, syntax_name: RefCell::new(String::new()), lines: Cell::new(0) });
        let s = this.clone();
        glib::spawn_future_local(async move {
            while let Ok((generation, spans)) = coloured_rx.recv().await {
                if generation == s.generation.load(Ordering::Relaxed) { s._apply(&spans); }
            }
        });
        this
    }

    /// Dark or light colours for the document only; the window keeps its own.
    pub fn set_dark(&self, dark: bool) {
        set_doc_dark(&[self.root.upcast_ref(), self.view.upcast_ref()], &self.view.buffer(), dark);
    }

    /// Show or hide the line numbers.
    pub fn toggle_numbers(&self) {
        self.numbers.set_visible(!self.numbers.is_visible());
    }

    /// `Rust  -  120 lines`, for the info bar.
    pub fn detail(&self) -> String {
        let lines = self.lines.get();
        format!("{}  -  {lines} line{}", self.syntax_name.borrow(), if lines == 1 { "" } else { "s" })
    }

    /// Read and show `path` and start its colouring; the error is a message for the user.
    pub fn show(&self, path: &Path) -> Result<(), String> {
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > MAX_BYTES { return Err(format!("the file is too big ({} MiB, the limit is {} MiB)", size / (1024 * 1024), MAX_BYTES / (1024 * 1024))); }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self._fill(&text);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let syntax = syntax_for(&name, text.lines().next().unwrap_or(""));
        *self.syntax_name.borrow_mut() = syntax.map_or_else(|| "plain text".to_string(), |s| s.name.clone());
        if let Some(syntax) = syntax { self._start_colouring(generation, text, syntax); }
        self.view.grab_focus();
        Ok(())
    }

    /// Replace the text and the line numbers, scrolled to the top.
    fn _fill(&self, text: &str) {
        let buffer = self.view.buffer();
        buffer.set_text(text);
        self.search.forget();
        buffer.place_cursor(&buffer.start_iter());
        self.lines.set(buffer.line_count() - i32::from(text.ends_with('\n')));
        self.numbers.set_line_count(buffer.line_count());
        self.root.vadjustment().set_value(0.0);
        self.root.hadjustment().set_value(0.0);
    }

    /// Colour `text` on a thread; it stops as soon as another file is shown.
    fn _start_colouring(&self, generation: u64, text: String, syntax: &'static crate::highlight::syntect::parsing::SyntaxReference) {
        let (current, tx) = (self.generation.clone(), self.coloured_tx.clone());
        std::thread::spawn(move || {
            highlight_in_batches(&text, syntax, BATCH_LINES, |spans| {
                current.load(Ordering::Relaxed) == generation && tx.send_blocking((generation, spans)).is_ok()
            });
        });
    }

    /// Put the colours of a block of lines on the text.
    fn _apply(&self, spans: &[CharSpan]) {
        let buffer = self.view.buffer();
        for span in spans {
            let (start, end) = (buffer.iter_at_offset(span.start as i32), buffer.iter_at_offset(span.end as i32));
            buffer.apply_tag_by_name(span.category.tag_name(), &start, &end);
        }
    }
}

impl TextDoc for SourceView {
    fn search(&self, query: &str) -> usize {
        self.search.run(&self.view, query)
    }

    fn step(&self, forward: bool) -> Option<()> {
        self.search.step(&self.view, forward)
    }

    fn label(&self) -> String {
        self.search.label()
    }

    /// The arrows scroll the text once it has the focus.
    fn focus(&self) {
        self.view.grab_focus();
    }
}

/// A read-only monospace text view that does not wrap.
fn _text_view() -> gtk::TextView {
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.add_css_class(DOC_TEXT_CLASS);
    view.set_wrap_mode(gtk::WrapMode::None);
    view.set_top_margin(PAGE_MARGIN);
    view.set_bottom_margin(PAGE_MARGIN);
    view
}
