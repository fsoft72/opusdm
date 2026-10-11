//! `opusdm-viewer`: shows one picture, PDF or video and the others of its folder.
//! See `docs/superpowers/specs/2026-10-05-viewer-gallery-design.md`.

mod args;
mod delete;
mod gallery;
mod image_view;
mod keys;
mod kind;
mod loader;
mod nearest;
mod pdf;
mod slideshow;
mod strip;
mod video_view;
mod window;
mod zoom;

use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;
use std::path::Path;
use std::process::ExitCode;

/// Stage and strip look, on top of the shared OpusDM style.
const VIEWER_CSS: &str = ".viewer-stage, scrolledwindow.viewer-stage, scrolledwindow.viewer-stage viewport, picture.viewer-stage { background: black; } \
    .viewer-error { color: #dddddd; padding: 24px; } \
    .viewer-info { padding: 4px 8px; } \
    .viewer-thumb-current { outline: 2px solid #3584e4; outline-offset: -2px; }";

/// Load the shared OpusDM look (theme mode, fonts).
fn apply_theme() {
    let settings = opusdm_ipc::settings::load_settings("opusdm-viewer");
    opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    let css = format!("{} {} {VIEWER_CSS}", opusdm_ui::chrome_css(settings.appearance.theme_mode), opusdm_ui::ui_look_css(&settings));
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

fn main() -> ExitCode {
    let target = match args::parse(std::env::args().skip(1)).and_then(|t| args::check_target(&t).map(|_| t)) {
        Ok(t) => t,
        Err(msg) => {
            opusdm_ipc::err_log!("viewer", "{msg}");
            return ExitCode::from(1);
        }
    };
    let gallery = match gallery::Gallery::build(Path::new(&target)) {
        Ok(g) => g,
        Err(e) => {
            opusdm_ipc::err_log!("viewer", "{target}: cannot read the folder: {e}");
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
