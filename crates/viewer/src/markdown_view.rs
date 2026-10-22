//! The Markdown area: a read-only text view that shows the rendered file, or
//! an EPUB book turned into Markdown.

use crate::font_scale::DOC_TEXT_CLASS;
use crate::epub::Book;
use crate::loader::{decode_within, load_within};
use crate::markdown::{indent_tag, parse};
use crate::text_doc::{create_match_tags, create_syntax_tags, set_doc_dark, TextDoc, TextSearch};
use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
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
/// Biggest side a picture is decoded at: room for a tall picture while a
/// huge photo never costs its full size in memory.
const MAX_DECODE_PX: i32 = 2 * MAX_IMAGE_WIDTH;
/// Seconds a picture on the web has to arrive.
const WEB_TIMEOUT_SECS: u32 = 20;
/// Biggest picture downloaded from the web, in bytes.
const MAX_WEB_BYTES: u64 = 16 * 1024 * 1024;
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
    /// Pictures of the book on screen, by image address; empty for a file.
    pictures: RefCell<HashMap<String, glib::Bytes>>,
}

impl MarkdownView {
    /// An empty view with its tags; clicking a link opens it.
    pub fn new() -> Rc<MarkdownView> {
        let view = gtk::TextView::new();
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.add_css_class(DOC_TEXT_CLASS);
        view.set_left_margin(PAGE_MARGIN);
        view.set_right_margin(PAGE_MARGIN);
        view.set_top_margin(PAGE_MARGIN);
        view.set_bottom_margin(PAGE_MARGIN);
        _create_tags(&view.buffer());
        let root = gtk::ScrolledWindow::new();
        root.set_vexpand(true);
        root.set_child(Some(&view));
        let this = Rc::new(MarkdownView { root, view, links: RefCell::new(Vec::new()), search: TextSearch::default(), base: RefCell::new(PathBuf::new()), pictures: RefCell::new(HashMap::new()) });
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
        self.pictures.borrow_mut().clear();
        self._fill(&String::from_utf8_lossy(&bytes));
        self.view.grab_focus();
        Ok(())
    }

    /// Show `book`, read from the file at `path`; its pictures come from memory.
    pub fn show_book(&self, path: &Path, book: Book) {
        *self.base.borrow_mut() = path.parent().map(Path::to_path_buf).unwrap_or_default();
        *self.pictures.borrow_mut() = book.pictures;
        self._fill(&book.markdown);
        self.view.grab_focus();
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
            let picture = run.image.as_deref().and_then(|url| self._picture(url, &run.text, run.image_width));
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

    /// The picture at `url` (a picture of the book, a file from the folder
    /// of the text, or an `http(s)` address) as a widget, drawn `width`
    /// pixels wide when it is given;
    /// `None` for another address or a name that is not a file. The
    /// picture is downloaded and decoded, scaled down, on a worker and
    /// appears when ready; one that cannot be had is replaced by its
    /// `[image: alt]` text.
    fn _picture(&self, url: &str, alt: &str, width: Option<i32>) -> Option<gtk::Box> {
        let web = _is_web(url).then(|| url.to_string());
        if web.is_none() && url.contains("://") { return None; }
        let name = match web {
            Some(_) => String::new(),
            None => gtk::glib::Uri::unescape_string(url, None)?.to_string(),
        };
        let in_book = self.pictures.borrow().get(name.as_str()).cloned();
        let path = self.base.borrow().join(name.as_str());
        if web.is_none() && in_book.is_none() && !path.is_file() {
            opusdm_ipc::err_log!("viewer", "cannot load picture {}: not a file", path.display());
            return None;
        }
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        holder.set_tooltip_text(Some(alt).filter(|a| !a.is_empty()));
        let handle = gio::spawn_blocking(move || {
            let shown = web.clone().unwrap_or_else(|| path.display().to_string());
            let bytes = match web.as_deref().map(_download) {
                Some(Ok(bytes)) => Some(bytes),
                Some(Err(e)) => return (shown, Err(e)),
                None => in_book,
            };
            let decoded = match &bytes {
                Some(bytes) => decode_within(bytes, MAX_DECODE_PX).map(Decoded::Raw),
                None => load_within(&path, MAX_DECODE_PX).map(Decoded::Raw),
            };
            // gdk-pixbuf cannot read it: GTK may have a loader of its own
            // (avif, jxl, heif through glycin). Decoded at full size, as the
            // viewer did before the scaled decode.
            let decoded = decoded.or_else(|e| _gtk_texture(bytes.as_ref(), &path).map(Decoded::Texture).ok_or(e));
            (shown, decoded)
        });
        let weak = holder.downgrade();
        let alt = alt.to_string();
        glib::spawn_future_local(async move {
            let decoded = handle.await;
            // The text was replaced meanwhile: nothing left to fill.
            let Some(holder) = weak.upgrade() else { return };
            let (shown, decoded) = match decoded {
                Ok(done) => done,
                Err(_) => return holder.append(&gtk::Label::new(Some(&format!("[image: {alt}]")))),
            };
            let texture = match decoded {
                Ok(Decoded::Raw(raw)) => raw.texture(0),
                Ok(Decoded::Texture(texture)) => texture,
                Err(e) => {
                    opusdm_ipc::err_log!("viewer", "cannot load picture {shown}: {e}");
                    return holder.append(&gtk::Label::new(Some(&format!("[image: {alt}]"))));
                }
            };
            let (w, h) = display_size(texture.width(), texture.height(), width);
            let picture = gtk::Picture::for_paintable(&texture);
            picture.set_can_shrink(true);
            picture.set_size_request(w, h);
            holder.append(&picture);
        });
        Some(holder)
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

/// A picture decoded on the worker: by gdk-pixbuf (scaled down), or by
/// GTK when gdk-pixbuf has no loader for its format.
enum Decoded {
    Raw(crate::loader::RawImage),
    Texture(gdk::Texture),
}

/// True for an address the viewer downloads pictures from.
fn _is_web(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The bytes at the `http(s)` address `url`, read by `curl` (which follows
/// redirects to `http(s)` only), within `WEB_TIMEOUT_SECS` and
/// `MAX_WEB_BYTES`. Blocking: run it on a worker.
fn _download(url: &str) -> Result<glib::Bytes, String> {
    let output = std::process::Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "--location", "--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--max-time", &WEB_TIMEOUT_SECS.to_string(), "--max-filesize", &MAX_WEB_BYTES.to_string()])
        .arg(url)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !output.status.success() { return Err(format!("curl failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr).trim())); }
    Ok(glib::Bytes::from_owned(output.stdout))
}

/// The picture decoded by GTK's own loaders, from the bytes of the book (or
/// downloaded) or from `path`; `None` when GTK cannot read it either. Safe on a worker.
fn _gtk_texture(in_book: Option<&glib::Bytes>, path: &Path) -> Option<gdk::Texture> {
    match in_book {
        Some(bytes) => gdk::Texture::from_bytes(bytes).ok(),
        None => gdk::Texture::from_filename(path).ok(),
    }
}

/// The size a `w` x `h` picture is drawn at: `wanted` pixels wide when it is
/// given (the `width` of an HTML `<img>`), else as it is; never wider than
/// `MAX_IMAGE_WIDTH`, keeping its proportions.
fn display_size(w: i32, h: i32, wanted: Option<i32>) -> (i32, i32) {
    let target = wanted.unwrap_or(w).min(MAX_IMAGE_WIDTH);
    if target == w || w <= 0 { return (w, h); }
    (target, (h as i64 * target as i64 / w as i64) as i32)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_pictures_keep_their_size_and_wide_ones_are_scaled_down() {
        assert_eq!(display_size(640, 480, None), (640, 480));
        assert_eq!(display_size(MAX_IMAGE_WIDTH, 10, None), (MAX_IMAGE_WIDTH, 10));
        assert_eq!(display_size(1600, 1200, None), (MAX_IMAGE_WIDTH, 600));
    }

    #[test]
    fn an_html_width_sets_the_size_up_to_the_widest() {
        assert_eq!(display_size(1200, 600, Some(600)), (600, 300));
        assert_eq!(display_size(100, 50, Some(200)), (200, 100));
        assert_eq!(display_size(1600, 1200, Some(5000)), (MAX_IMAGE_WIDTH, 600));
    }

    #[test]
    fn only_http_addresses_are_downloaded() {
        assert!(_is_web("https://example.org/a.png"));
        assert!(_is_web("HTTP://example.org/a.png"));
        assert!(!_is_web("file:///etc/passwd"));
        assert!(!_is_web("pics/a.png"));
    }

    #[test]
    fn a_failed_download_is_an_error_with_a_message() {
        let e = _download("http://127.0.0.1:9/no.png").expect_err("nothing listens on port 9");
        assert!(e.contains("curl"), "{e}");
    }

    #[test]
    fn gtk_decodes_a_picture_from_the_book_or_a_file_and_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pic.png");
        gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 6, 4).unwrap().savev(&path, "png", &[]).unwrap();
        let bytes = glib::Bytes::from_owned(std::fs::read(&path).unwrap());
        assert_eq!(_gtk_texture(None, &path).map(|t| (t.width(), t.height())), Some((6, 4)));
        assert_eq!(_gtk_texture(Some(&bytes), Path::new("/no/such/file")).map(|t| t.width()), Some(6));
        let garbage = dir.path().join("bad.avif");
        std::fs::write(&garbage, b"not a picture").unwrap();
        assert!(_gtk_texture(None, &garbage).is_none());
        assert!(_gtk_texture(Some(&glib::Bytes::from_static(b"nope")), &garbage).is_none());
    }
}
