//! The Markdown area: a read-only text view that shows the rendered file.

use crate::markdown::{indent_tag, parse};
use crate::text_doc::{create_match_tags, create_syntax_tags, set_doc_dark, TextDoc, TextSearch};
use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::pango;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Biggest file shown; a larger one is more likely not a text to read.
const MAX_BYTES: u64 = 8 * 1024 * 1024;
/// Margin around the text, in pixels.
const PAGE_MARGIN: i32 = 24;
/// Pixels of left margin per indent level.
const INDENT_PX: i32 = 24;
/// Text scale of the headings `h1`..`h6`.
const HEADING_SCALES: [f64; 6] = [2.0, 1.6, 1.35, 1.15, 1.0, 0.9];
/// Shade behind code, whatever the theme.
const CODE_BG: &str = "rgba(128,128,128,0.22)";
const LINK_COLOR: &str = "#4a90ff";
const RULE_COLOR: &str = "#888888";
/// Widest a picture is drawn, in pixels; a wider one is scaled down.
const MAX_IMAGE_WIDTH: i32 = 800;
/// Deepest indent tag created (matches `markdown::indent_tag`).
const MAX_INDENT: usize = 6;

/// A link of the text: character range and target.
struct LinkRange {
    start: i32,
    end: i32,
    url: String,
}

/// The scrolled text view and the links of the file on screen.
pub struct MarkdownView {
    /// Put this in the stack.
    pub root: gtk::ScrolledWindow,
    view: gtk::TextView,
    links: RefCell<Vec<LinkRange>>,
    search: TextSearch,
    /// Folder of the file on screen, to resolve relative links.
    base: RefCell<PathBuf>,
}

impl MarkdownView {
    /// An empty view with its tags; clicking a link opens it.
    pub fn new() -> Rc<MarkdownView> {
        let view = gtk::TextView::new();
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_left_margin(PAGE_MARGIN);
        view.set_right_margin(PAGE_MARGIN);
        view.set_top_margin(PAGE_MARGIN);
        view.set_bottom_margin(PAGE_MARGIN);
        _create_tags(&view.buffer());
        let root = gtk::ScrolledWindow::new();
        root.set_vexpand(true);
        root.set_child(Some(&view));
        let this = Rc::new(MarkdownView { root, view, links: RefCell::new(Vec::new()), search: TextSearch::default(), base: RefCell::new(PathBuf::new()) });
        this._wire_links();
        this
    }

    /// Dark or light colours for the document only; the window keeps its own.
    pub fn set_dark(&self, dark: bool) {
        set_doc_dark(&[self.root.upcast_ref(), self.view.upcast_ref()], &self.view.buffer(), dark);
    }

    /// Read and show `path`; the error is a message for the user.
    pub fn show(&self, path: &Path) -> Result<(), String> {
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > MAX_BYTES { return Err(format!("the file is too big ({} MiB, the limit is {} MiB)", size / (1024 * 1024), MAX_BYTES / (1024 * 1024))); }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        *self.base.borrow_mut() = path.parent().map(Path::to_path_buf).unwrap_or_default();
        self._fill(&String::from_utf8_lossy(&bytes));
        self.view.grab_focus();
        Ok(())
    }

    /// Replace the text with the rendering of `source`, scrolled to the top.
    fn _fill(&self, source: &str) {
        let buffer = self.view.buffer();
        buffer.set_text("");
        self.search.forget();
        let mut links = Vec::new();
        for run in parse(source) {
            let start = buffer.end_iter().offset();
            let mut at = buffer.end_iter();
            let picture = run.image.as_deref().and_then(|url| self._picture(url, &run.text));
            match &picture {
                Some(picture) => {
                    let anchor = buffer.create_child_anchor(&mut at);
                    self.view.add_child_at_anchor(picture, &anchor);
                }
                None if run.image.is_some() => buffer.insert(&mut at, &format!("[image: {}]", run.text)),
                None => buffer.insert(&mut at, &run.text),
            }
            let (from, to) = (buffer.iter_at_offset(start), buffer.end_iter());
            for name in &run.tags {
                if buffer.tag_table().lookup(name).is_none() { opusdm_ipc::err_log!("viewer", "unknown Markdown tag {name}"); continue; }
                buffer.apply_tag_by_name(name, &from, &to);
            }
            if let Some(url) = run.link { links.push(LinkRange { start, end: to.offset(), url }); }
        }
        *self.links.borrow_mut() = links;
        buffer.place_cursor(&buffer.start_iter());
        self.root.vadjustment().set_value(0.0);
    }

    /// The picture at `url` (a file, from the folder of the text) as a widget;
    /// `None` for an address on the web or a file that cannot be read.
    fn _picture(&self, url: &str, alt: &str) -> Option<gtk::Picture> {
        if url.contains("://") { return None; }
        let name = gtk::glib::Uri::unescape_string(url, None)?;
        let path = self.base.borrow().join(name.as_str());
        let texture = match gdk::Texture::from_filename(&path) {
            Ok(t) => t,
            Err(e) => {
                opusdm_ipc::err_log!("viewer", "cannot load picture {}: {e}", path.display());
                return None;
            }
        };
        let (w, h) = (texture.width(), texture.height());
        let (w, h) = if w > MAX_IMAGE_WIDTH { (MAX_IMAGE_WIDTH, (h as i64 * MAX_IMAGE_WIDTH as i64 / w as i64) as i32) } else { (w, h) };
        let picture = gtk::Picture::for_paintable(&texture);
        picture.set_can_shrink(true);
        picture.set_size_request(w, h);
        picture.set_tooltip_text(Some(alt).filter(|a| !a.is_empty()));
        Some(picture)
    }

    /// Target of the link under the widget point (`x`, `y`), if any.
    fn _link_at(&self, x: f64, y: f64) -> Option<String> {
        let (bx, by) = self.view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let iter = self.view.iter_at_location(bx, by)?;
        let at = iter.offset();
        self.links.borrow().iter().find(|l| l.start <= at && at < l.end).map(|l| l.url.clone())
    }

    /// Click opens a link; the pointer is a hand over one.
    fn _wire_links(self: &Rc<Self>) {
        let click = gtk::GestureClick::new();
        let this = self.clone();
        click.connect_released(move |gesture, n, x, y| {
            let selecting = this.view.buffer().has_selection();
            if n != 1 || selecting || gesture.current_button() != gdk::BUTTON_PRIMARY { return; }
            if let Some(url) = this._link_at(x, y) { this._open(&url); }
        });
        self.view.add_controller(click);
        let motion = gtk::EventControllerMotion::new();
        let this = self.clone();
        motion.connect_motion(move |_, x, y| {
            let name = if this._link_at(x, y).is_some() { "pointer" } else { "text" };
            this.view.set_cursor_from_name(Some(name));
        });
        self.view.add_controller(motion);
    }

    /// Open a link with the desktop: a web address as it is, a path from the
    /// folder of the file. A `#anchor` inside the file is ignored.
    fn _open(&self, url: &str) {
        if url.starts_with('#') { return; }
        let uri = if url.contains("://") || url.starts_with("mailto:") {
            url.to_string()
        } else {
            let target = self.base.borrow().join(url.split('#').next().unwrap_or(url));
            gio::File::for_path(&target).uri().to_string()
        };
        if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
            opusdm_ipc::err_log!("viewer", "cannot open {uri}: {e}");
        }
    }
}

impl TextDoc for MarkdownView {
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

/// The tags of `markdown::Run`, by name.
fn _create_tags(buffer: &gtk::TextBuffer) {
    for (i, scale) in HEADING_SCALES.iter().enumerate() {
        buffer.create_tag(Some(&format!("h{}", i + 1)), &[("weight", &700), ("scale", scale)]);
    }
    buffer.create_tag(Some("bold"), &[("weight", &700)]);
    buffer.create_tag(Some("italic"), &[("style", &pango::Style::Italic)]);
    buffer.create_tag(Some("strike"), &[("strikethrough", &true)]);
    buffer.create_tag(Some("code"), &[("family", &"monospace"), ("background", &CODE_BG)]);
    buffer.create_tag(Some("codeblock"), &[("family", &"monospace"), ("background", &CODE_BG), ("wrap-mode", &gtk::WrapMode::None)]);
    buffer.create_tag(Some("quote"), &[("style", &pango::Style::Italic), ("foreground", &RULE_COLOR)]);
    buffer.create_tag(Some("link"), &[("foreground", &LINK_COLOR), ("underline", &pango::Underline::Single)]);
    buffer.create_tag(Some("table"), &[("family", &"monospace"), ("wrap-mode", &gtk::WrapMode::None)]);
    buffer.create_tag(Some("rule"), &[("foreground", &RULE_COLOR)]);
    for level in 1..=MAX_INDENT {
        buffer.create_tag(Some(&indent_tag(level)), &[("left-margin", &(INDENT_PX * level as i32 + PAGE_MARGIN))]);
    }
    create_syntax_tags(buffer);
    // The match tags are created last: they win over every other tag.
    create_match_tags(buffer);
}
