//! Quick Look file previewer (F3 / Space).

use crate::model::format_size;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Image,
    Text,
    Archive,
    Binary,
}

/// File extensions treated as text.
const TEXT_EXTS: &[&str] = &[
    "rs", "c", "h", "cpp", "hpp", "cc", "py", "js", "ts", "jsx", "tsx", "html", "css", "scss", "json", "toml",
    "yaml", "yml", "md", "txt", "sh", "bash", "xml", "ini", "conf", "log", "diff", "patch", "sql", "csv", "tsv",
    "lock",
];


/// File extensions treated as pictures.
pub const IMAGE_EXTS: [&str; 9] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "tiff"];

/// Classify a file from its lower-case extension and its first bytes: the
/// extension decides for archives, pictures and known text types; otherwise
/// the first 512 bytes decide between text (no NUL, valid UTF-8) and binary.
pub fn kind_of(ext: &str, head: &[u8]) -> FileKind {
    if crate::archive::ARCHIVE_EXTS.contains(&ext) {
        return FileKind::Archive;
    }
    if IMAGE_EXTS.contains(&ext) {
        return FileKind::Image;
    }
    if TEXT_EXTS.contains(&ext) {
        return FileKind::Text;
    }
    let probe = &head[..head.len().min(512)];
    if !probe.is_empty() && !probe.contains(&0) && std::str::from_utf8(probe).is_ok() {
        return FileKind::Text;
    }
    FileKind::Binary
}

/// Detect the kind of file based on extension and content.
pub fn detect_file_kind(path: &Path) -> FileKind {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    // Only an unknown extension needs the content probe.
    let mut head = [0u8; 512];
    let n = if kind_of(&ext, &[]) == FileKind::Binary {
        File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0)
    } else {
        0
    };
    kind_of(&ext, &head[..n])
}

/// Generate a formatted hex dump string for a buffer.
pub fn generate_hex_dump(buf: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (chunk_idx, chunk) in buf.chunks(16).enumerate() {
        let offset = chunk_idx * 16;
        let _ = write!(out, "{offset:08x}:  ");
        // Hex bytes
        for i in 0..16 {
            if i < chunk.len() {
                let _ = write!(out, "{:02x} ", chunk[i]);
            } else {
                out.push_str("   ");
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        // ASCII printable
        for &b in chunk {
            if (32..=126).contains(&b) {
                out.push(b as char);
            } else {
                out.push('.');
            }
        }
        out.push_str("|\n");
    }
    out
}

/// What `load_off_thread` computed, before any GTK object is built. Crossing
/// back to the main thread as plain data keeps every blocking read (and the
/// archive-listing subprocess) off the GTK thread, since GTK/GDK objects
/// can't safely be built off it.
#[derive(Debug)]
enum Loaded {
    Image { bytes: Vec<u8>, ext: String, size_str: String },
    Text { meta: String, numbered_text: String },
    Archive { meta: String, listing: String },
    Binary { meta: String, hex: String },
    Directory { meta: String, listing: String },
    Error { message: String },
}

pub struct PreviewWindow {
    pub window: gtk::Window,
    pub title_label: gtk::Label,
    pub meta_label: gtk::Label,
    pub content_box: gtk::Box,
    pub current_path: RefCell<PathBuf>,
    /// Bumped on every `load_file`; an in-flight background load whose
    /// generation no longer matches was superseded by a later navigation and
    /// its result is dropped instead of rendered.
    generation: Cell<u64>,
}

impl PreviewWindow {
    pub fn new(parent: &gtk::ApplicationWindow) -> Rc<Self> {
        let window = gtk::Window::builder()
            .title("Quick Look Preview")
            .transient_for(parent)
            .default_width(720)
            .default_height(540)
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("preview-root");

        // Top header bar
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_margin_start(12);
        header.set_margin_end(12);
        header.set_margin_top(8);
        header.set_margin_bottom(8);
        header.add_css_class("preview-header");

        let title_label = gtk::Label::new(None);
        title_label.set_xalign(0.0);
        title_label.set_hexpand(true);
        title_label.add_css_class("preview-title");

        let meta_label = gtk::Label::new(None);
        meta_label.set_xalign(1.0);
        meta_label.add_css_class("dim-label");

        let close_btn = gtk::Button::with_label("✕");
        close_btn.add_css_class("flat");
        let win_clone = window.clone();
        close_btn.connect_clicked(move |_| {
            win_clone.close();
        });

        header.append(&title_label);
        header.append(&meta_label);
        header.append(&close_btn);
        root.append(&header);

        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content_box.set_vexpand(true);
        content_box.set_hexpand(true);
        root.append(&content_box);

        window.set_child(Some(&root));

        let preview = Rc::new(Self {
            window,
            title_label,
            meta_label,
            content_box,
            current_path: RefCell::new(PathBuf::new()),
            generation: Cell::new(0),
        });

        // Key handler to close on Esc or Space or F3
        {
            let win = preview.window.clone();
            let key_ctrl = gtk::EventControllerKey::new();
            key_ctrl.connect_key_pressed(move |_, keyval, _, _| match keyval {
                gdk::Key::Escape | gdk::Key::F3 | gdk::Key::space => {
                    win.close();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            });
            preview.window.add_controller(key_ctrl);
        }

        preview
    }

    /// Update preview content for the given file path. Every kind of read -
    /// the directory listing, the binary-vs-text probe, the text/binary byte
    /// reads, and the archive-listing subprocess - runs on a background
    /// thread so a slow (e.g. network-mounted) directory or file can't stall
    /// the window; only the resulting `Loaded` data crosses back to build
    /// GTK objects on the main thread.
    pub fn load_file(self: &Rc<Self>, path: &Path) {
        *self.current_path.borrow_mut() = path.to_path_buf();
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        self.title_label.set_text(&filename);
        self.window.set_title(Some(&format!("Quick Look: {filename}")));
        self.clear_content();

        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                self.meta_label.set_text("Error reading file");
                let err_label = gtk::Label::new(Some(&format!("Failed to open file: {e}")));
                err_label.set_margin_top(20);
                self.content_box.append(&err_label);
                return;
            }
        };

        let is_dir = metadata.is_dir();
        let file_size = metadata.len();
        self.meta_label.set_text("Loading…");
        let placeholder = gtk::Label::new(Some("Loading preview…"));
        placeholder.set_margin_top(20);
        self.content_box.append(&placeholder);

        let gen = self.generation.get() + 1;
        self.generation.set(gen);

        let (tx, rx) = async_channel::bounded::<Loaded>(1);
        let path_owned = path.to_path_buf();
        std::thread::spawn(move || {
            let loaded = if is_dir {
                load_directory_off_thread(&path_owned, file_size)
            } else {
                load_off_thread(&path_owned, file_size)
            };
            let _ = tx.send_blocking(loaded);
        });

        let this = self.clone();
        let want_path = path.to_path_buf();
        glib::spawn_future_local(async move {
            let Ok(loaded) = rx.recv().await else { return };
            // The user may have navigated to another file while this loaded;
            // drop a superseded result instead of rendering it over the
            // current one.
            if this.generation.get() != gen || *this.current_path.borrow() != want_path {
                return;
            }
            this.apply_loaded(loaded);
        });
    }

    fn clear_content(&self) {
        while let Some(child) = self.content_box.first_child() {
            self.content_box.remove(&child);
        }
    }

    fn apply_loaded(&self, loaded: Loaded) {
        self.clear_content();
        match loaded {
            Loaded::Image { bytes, ext, size_str } => self.render_image(&bytes, &ext, &size_str),
            Loaded::Text { meta, numbered_text } => {
                self.meta_label.set_text(&meta);
                self.content_box.append(&text_view_scroller(&numbered_text, gtk::WrapMode::WordChar));
            }
            Loaded::Archive { meta, listing } => {
                self.meta_label.set_text(&meta);
                self.content_box.append(&text_view_scroller(&listing, gtk::WrapMode::None));
            }
            Loaded::Binary { meta, hex } => {
                self.meta_label.set_text(&meta);
                self.content_box.append(&text_view_scroller(&hex, gtk::WrapMode::None));
            }
            Loaded::Directory { meta, listing } => {
                self.meta_label.set_text(&meta);
                self.content_box.append(&text_view_scroller(&listing, gtk::WrapMode::None));
            }
            Loaded::Error { message } => {
                let err_label = gtk::Label::new(Some(&message));
                err_label.set_margin_top(20);
                self.content_box.append(&err_label);
            }
        }
    }

    fn render_image(&self, bytes: &[u8], ext: &str, size_str: &str) {
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_vexpand(true);
        picture.set_hexpand(true);
        picture.set_margin_start(16);
        picture.set_margin_end(16);
        picture.set_margin_top(16);
        picture.set_margin_bottom(16);

        let gbytes = glib::Bytes::from(bytes);
        let meta_text = match gdk::Texture::from_bytes(&gbytes) {
            Ok(texture) => {
                let t = format!(
                    "{} × {} px  •  {}  •  {}",
                    texture.width(),
                    texture.height(),
                    size_str,
                    ext.to_uppercase()
                );
                picture.set_paintable(Some(&texture));
                t
            }
            Err(_) => format!("{size_str}  •  {}", ext.to_uppercase()),
        };
        self.meta_label.set_text(&meta_text);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&picture)
            .vexpand(true)
            .hexpand(true)
            .build();
        self.content_box.append(&scroller);
    }

}

/// A read-only monospace text view in its own scroller, as used for the
/// text/archive/binary previews (and the directory listing).
fn text_view_scroller(text: &str, wrap_mode: gtk::WrapMode) -> gtk::ScrolledWindow {
    let text_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(wrap_mode)
        .build();
    text_view.buffer().set_text(text);
    text_view.set_margin_start(8);
    text_view.set_margin_end(8);
    text_view.set_margin_top(8);
    text_view.set_margin_bottom(8);

    gtk::ScrolledWindow::builder().child(&text_view).vexpand(true).hexpand(true).build()
}

/// List an archive's contents via an external tool. Pure function (no I/O
/// beyond the subprocess itself), safe to call off the GTK thread.
fn archive_listing(path: &Path, ext: &str) -> String {
    if ext == "zip" {
        Command::new("unzip")
            .arg("-l")
            .arg(path)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_else(|_| "Cannot run unzip to inspect archive".into())
    } else {
        Command::new("tar")
            .arg("-tf")
            .arg(path)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_else(|_| "Cannot run tar to inspect archive".into())
    }
}

/// Everything `load_file` needs that involves blocking I/O or a subprocess:
/// the binary-vs-text probe (inside `detect_file_kind`), the file read for
/// text/binary/image previews, and the archive-listing subprocess. Runs on a
/// background thread; returns plain data so the caller can build GTK objects
/// from it on the main thread.
/// Directory listing, off the GTK thread like every other preview kind (a
/// slow, e.g. network-mounted, directory must not freeze the window).
fn load_directory_off_thread(path: &Path, size: u64) -> Loaded {
    let mut listing = String::new();
    if let Ok(entries) = std::fs::read_dir(path) {
        let mut items = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            items.push((name, is_dir));
        }
        items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        for (name, is_dir) in items {
            if is_dir {
                listing.push_str(&format!("📁  {name}/\n"));
            } else {
                listing.push_str(&format!("📄  {name}\n"));
            }
        }
    }
    let meta = format!("Directory  •  {}", format_size(true, size));
    Loaded::Directory { meta, listing }
}

fn load_off_thread(path: &Path, file_size: u64) -> Loaded {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let size_str = format_size(false, file_size);

    match detect_file_kind(path) {
        FileKind::Image => match std::fs::read(path) {
            Ok(bytes) => Loaded::Image { bytes, ext, size_str },
            Err(e) => Loaded::Error { message: format!("Cannot open: {e}") },
        },
        FileKind::Text => {
            const MAX_READ: u64 = 256 * 1024;
            let f = match File::open(path) {
                Ok(f) => f,
                Err(e) => return Loaded::Error { message: format!("Cannot open: {e}") },
            };
            let mut buf = Vec::new();
            let _ = f.take(MAX_READ).read_to_end(&mut buf);
            let text = String::from_utf8_lossy(&buf);

            let line_count = text.lines().count();
            let truncated = if file_size > MAX_READ { " (preview truncated)" } else { "" };
            let meta = format!("{line_count} lines  •  {size_str}{truncated}");

            let mut numbered_text = String::with_capacity(text.len() + line_count * 8);
            for (i, line) in text.lines().enumerate() {
                numbered_text.push_str(&format!("{:4} │ {}\n", i + 1, line));
            }
            Loaded::Text { meta, numbered_text }
        }
        FileKind::Archive => {
            let listing = archive_listing(path, &ext);
            let meta = format!("Archive ({})  •  {size_str}", ext.to_uppercase());
            Loaded::Archive { meta, listing }
        }
        FileKind::Binary => {
            let mut buf = [0u8; 256];
            let bytes_read = File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
            let hex = generate_hex_dump(&buf[..bytes_read]);
            let meta = format!("Binary  •  {size_str}");
            Loaded::Binary { meta, hex }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_file_kind() {
        assert_eq!(detect_file_kind(Path::new("image.png")), FileKind::Image);
        assert_eq!(detect_file_kind(Path::new("photo.jpg")), FileKind::Image);
        assert_eq!(detect_file_kind(Path::new("vector.svg")), FileKind::Image);
        assert_eq!(detect_file_kind(Path::new("archive.zip")), FileKind::Archive);
        assert_eq!(detect_file_kind(Path::new("bundle.tar.gz")), FileKind::Archive);
        assert_eq!(detect_file_kind(Path::new("main.rs")), FileKind::Text);
        assert_eq!(detect_file_kind(Path::new("config.toml")), FileKind::Text);
    }

    #[test]
    fn test_generate_hex_dump() {
        let data = b"Hello, World!\n";
        let dump = generate_hex_dump(data);
        assert!(dump.contains("48 65 6c 6c 6f"));
        assert!(dump.contains("|Hello, World!.|"));
    }

    #[test]
    fn load_off_thread_reads_text_with_line_numbers() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("notes.txt");
        std::fs::write(&path, "first\nsecond\n").unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        match load_off_thread(&path, size) {
            Loaded::Text { meta, numbered_text } => {
                assert!(meta.contains("2 lines"));
                assert!(numbered_text.contains("1 │ first"));
                assert!(numbered_text.contains("2 │ second"));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn load_off_thread_reads_binary_as_hex() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("blob.bin");
        std::fs::write(&path, [0u8, 1, 2, 3, 255]).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        match load_off_thread(&path, size) {
            Loaded::Binary { hex, .. } => assert!(hex.contains("00 01 02 03 ff")),
            other => panic!("expected Binary, got {other:?}"),
        }
    }

    #[test]
    fn load_off_thread_reports_error_for_missing_file() {
        let missing = Path::new("/nonexistent/opusdm-preview-test/file.txt");
        match load_off_thread(missing, 0) {
            Loaded::Error { message } => assert!(message.contains("Cannot open")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn load_directory_off_thread_lists_dirs_before_files_case_insensitively() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("zeta.txt"), b"x").unwrap();
        std::fs::write(td.path().join("Apple.txt"), b"x").unwrap();
        std::fs::create_dir(td.path().join("banana_dir")).unwrap();

        match load_directory_off_thread(td.path(), 4096) {
            Loaded::Directory { meta, listing } => {
                assert!(meta.contains("Directory"));
                let dir_pos = listing.find("banana_dir").unwrap();
                let apple_pos = listing.find("Apple.txt").unwrap();
                let zeta_pos = listing.find("zeta.txt").unwrap();
                assert!(dir_pos < apple_pos, "directories should sort before files");
                assert!(apple_pos < zeta_pos, "files should sort case-insensitively");
            }
            other => panic!("expected Directory, got {other:?}"),
        }
    }

    #[test]
    fn kind_of_classifies_by_extension_first_then_by_content() {
        assert_eq!(kind_of("png", b"\x89PNG"), FileKind::Image);
        assert_eq!(kind_of("rs", b"\x00\x01"), FileKind::Text, "a known text extension wins");
        assert_eq!(kind_of("zip", b"PK"), FileKind::Archive);
        assert_eq!(kind_of("weird", b"plain words\n"), FileKind::Text);
        assert_eq!(kind_of("weird", b"a\x00b"), FileKind::Binary);
        assert_eq!(kind_of("weird", b""), FileKind::Binary);
        assert_eq!(kind_of("weird", &[0xC9, 0xCD, 0xBB]), FileKind::Binary, "CP437 bytes are not UTF-8");
    }
}
