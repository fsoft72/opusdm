//! Quick Look file previewer (F3 / Space).

use crate::model::format_size;
use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
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

/// Detect the kind of file based on extension and content.
pub fn detect_file_kind(path: &Path) -> FileKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "tiff" => FileKind::Image,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "tbz2" | "xz" | "txz" | "zst" | "7z" | "rar" => {
            FileKind::Archive
        }
        "rs" | "c" | "h" | "cpp" | "hpp" | "cc" | "py" | "js" | "ts" | "jsx" | "tsx" | "html"
        | "css" | "scss" | "json" | "toml" | "yaml" | "yml" | "md" | "txt" | "sh" | "bash"
        | "xml" | "ini" | "conf" | "log" | "diff" | "patch" | "sql" | "csv" | "tsv" | "lock" => {
            FileKind::Text
        }
        _ => {
            // Probe first 512 bytes for null byte / binary characters
            if let Ok(mut f) = File::open(path) {
                let mut buf = [0u8; 512];
                if let Ok(n) = f.read(&mut buf) {
                    if n > 0 && !buf[..n].contains(&0) && std::str::from_utf8(&buf[..n]).is_ok() {
                        return FileKind::Text;
                    }
                }
            }
            FileKind::Binary
        }
    }
}

/// Generate a formatted hex dump string for a buffer.
pub fn generate_hex_dump(buf: &[u8]) -> String {
    let mut out = String::new();
    for (chunk_idx, chunk) in buf.chunks(16).enumerate() {
        let offset = chunk_idx * 16;
        out.push_str(&format!("{offset:08x}:  "));
        // Hex bytes
        for i in 0..16 {
            if i < chunk.len() {
                out.push_str(&format!("{:02x} ", chunk[i]));
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

pub struct PreviewWindow {
    pub window: gtk::Window,
    pub title_label: gtk::Label,
    pub meta_label: gtk::Label,
    pub content_box: gtk::Box,
    pub current_path: RefCell<PathBuf>,
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

    /// Update preview content for the given file path.
    pub fn load_file(self: &Rc<Self>, path: &Path) {
        *self.current_path.borrow_mut() = path.to_path_buf();
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        self.title_label.set_text(&filename);
        self.window.set_title(Some(&format!("Quick Look: {filename}")));

        // Clear previous content
        while let Some(child) = self.content_box.first_child() {
            self.content_box.remove(&child);
        }

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

        let file_size = metadata.len();
        let size_str = format_size(metadata.is_dir(), file_size);

        if metadata.is_dir() {
            self.meta_label.set_text(&format!("Directory  •  {size_str}"));
            self.render_directory(path);
            return;
        }

        let kind = detect_file_kind(path);

        match kind {
            FileKind::Image => {
                self.render_image(path, file_size);
            }
            FileKind::Text => {
                self.render_text(path, file_size);
            }
            FileKind::Archive => {
                self.render_archive(path, file_size);
            }
            FileKind::Binary => {
                self.render_binary(path, file_size);
            }
        }
    }

    fn render_image(&self, path: &Path, file_size: u64) {
        let file = gio::File::for_path(path);
        let picture = gtk::Picture::for_file(&file);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_vexpand(true);
        picture.set_hexpand(true);
        picture.set_margin_start(16);
        picture.set_margin_end(16);
        picture.set_margin_top(16);
        picture.set_margin_bottom(16);

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("image");
        let size_str = format_size(false, file_size);

        // Probe dimensions via gdk::Texture if possible
        let meta_text = match gdk::Texture::from_file(&file) {
            Ok(texture) => {
                format!("{} × {} px  •  {}  •  {}", texture.width(), texture.height(), size_str, ext.to_uppercase())
            }
            Err(_) => format!("{}  •  {}", size_str, ext.to_uppercase()),
        };
        self.meta_label.set_text(&meta_text);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&picture)
            .vexpand(true)
            .hexpand(true)
            .build();
        self.content_box.append(&scroller);
    }

    fn render_text(&self, path: &Path, file_size: u64) {
        let max_read = 256 * 1024; // 256 KB max for preview
        let f = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                let err_label = gtk::Label::new(Some(&format!("Cannot open: {e}")));
                self.content_box.append(&err_label);
                return;
            }
        };

        let mut buf = Vec::new();
        let _ = f.take(max_read).read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf);

        let line_count = text.lines().count();
        let size_str = format_size(false, file_size);
        let truncated = if file_size > max_read { " (preview truncated)" } else { "" };
        self.meta_label.set_text(&format!("{line_count} lines  •  {size_str}{truncated}"));

        // Format lines with line numbers
        let mut numbered_text = String::with_capacity(text.len() + line_count * 8);
        for (i, line) in text.lines().enumerate() {
            numbered_text.push_str(&format!("{:4} │ {}\n", i + 1, line));
        }

        let text_view = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        text_view.buffer().set_text(&numbered_text);
        text_view.set_margin_start(8);
        text_view.set_margin_end(8);
        text_view.set_margin_top(8);
        text_view.set_margin_bottom(8);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&text_view)
            .vexpand(true)
            .hexpand(true)
            .build();
        self.content_box.append(&scroller);
    }

    fn render_archive(&self, path: &Path, file_size: u64) {
        let size_str = format_size(false, file_size);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        self.meta_label.set_text(&format!("Archive ({})  •  {}", ext.to_uppercase(), size_str));

        let listing = if ext == "zip" {
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
        };

        let text_view = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::None)
            .build();
        text_view.buffer().set_text(&listing);
        text_view.set_margin_start(8);
        text_view.set_margin_end(8);
        text_view.set_margin_top(8);
        text_view.set_margin_bottom(8);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&text_view)
            .vexpand(true)
            .hexpand(true)
            .build();
        self.content_box.append(&scroller);
    }

    fn render_binary(&self, path: &Path, file_size: u64) {
        let size_str = format_size(false, file_size);
        self.meta_label.set_text(&format!("Binary  •  {size_str}"));

        let mut buf = [0u8; 256];
        let bytes_read = match File::open(path).and_then(|mut f| f.read(&mut buf)) {
            Ok(n) => n,
            Err(_) => 0,
        };

        let hex_view = generate_hex_dump(&buf[..bytes_read]);

        let text_view = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::None)
            .build();
        text_view.buffer().set_text(&hex_view);
        text_view.set_margin_start(8);
        text_view.set_margin_end(8);
        text_view.set_margin_top(8);
        text_view.set_margin_bottom(8);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&text_view)
            .vexpand(true)
            .hexpand(true)
            .build();
        self.content_box.append(&scroller);
    }

    fn render_directory(&self, path: &Path) {
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

        let text_view = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::None)
            .build();
        text_view.buffer().set_text(&listing);
        text_view.set_margin_start(8);
        text_view.set_margin_end(8);
        text_view.set_margin_top(8);
        text_view.set_margin_bottom(8);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&text_view)
            .vexpand(true)
            .hexpand(true)
            .build();
        self.content_box.append(&scroller);
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
}
