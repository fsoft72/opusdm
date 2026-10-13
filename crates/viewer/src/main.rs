//! `opusdm-viewer`: shows the pictures, PDF files, Markdown texts and videos given on the
//! command line, files and whole folders.
//! See `docs/superpowers/specs/2026-10-05-viewer-gallery-design.md`.

mod args;
mod delete;
mod gallery;
mod highlight;
mod image_view;
mod keys;
mod kind;
mod loader;
mod markdown;
mod markdown_view;
mod view_paintable;
mod pdf;
mod pdf_bar;
mod pdf_links;
mod pdf_pages;
mod pdf_search;
mod pdf_text;
mod slideshow;
mod strip;
mod text_doc;
mod video_view;
mod window;
mod zoom;

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use std::path::PathBuf;
use std::process::ExitCode;

/// Stage and strip look, on top of the shared OpusDM style.
const VIEWER_CSS: &str = ".viewer-stage, scrolledwindow.viewer-stage, scrolledwindow.viewer-stage viewport, picture.viewer-stage { background: black; } \
    .viewer-doc-dark, scrolledwindow.viewer-doc-dark, textview.viewer-doc-dark, textview.viewer-doc-dark text { background-color: #1e1f22; color: #e3e3e3; } \
    .viewer-doc-light, scrolledwindow.viewer-doc-light, textview.viewer-doc-light, textview.viewer-doc-light text { background-color: #ffffff; color: #1a1a1a; } \
    .viewer-error { color: #dddddd; padding: 24px; } \
    .viewer-info { padding: 4px 8px; } \
    .viewer-pdf-bar { padding: 4px 8px; } \
    .viewer-thumb-current { outline: 2px solid @opusdm_accent; outline-offset: -2px; }";

/// Load the shared OpusDM look (theme mode, fonts).
fn apply_theme() {
    opusdm_ui::install_app_css("opusdm-viewer", VIEWER_CSS);
}

fn main() -> ExitCode {
    let targets = match args::parse(std::env::args().skip(1)).and_then(|ts| ts.iter().try_for_each(|t| args::check_target(t)).map(|_| ts)) {
        Ok(ts) => ts,
        Err(msg) => {
            opusdm_ipc::err_log!("viewer", "{msg}");
            return ExitCode::from(1);
        }
    };
    let targets: Vec<PathBuf> = targets.iter().map(PathBuf::from).collect();
    let gallery = match gallery::Gallery::from_targets(&targets) {
        Ok(g) => g,
        Err(e) => {
            opusdm_ipc::err_log!("viewer", "{e}");
            return ExitCode::from(1);
        }
    };
    // Each launch is its own process and window; the file is not a GApplication argument.
    let app = gtk::Application::builder().application_id("org.opusdm.Viewer").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    let gallery = std::cell::RefCell::new(Some(gallery));
    app.connect_activate(move |app| {
        let Some(gallery) = gallery.borrow_mut().take() else { return };
        apply_theme();
        window::open(app, gallery);
    });
    app.run_with_args::<&str>(&[]);
    ExitCode::SUCCESS
}
