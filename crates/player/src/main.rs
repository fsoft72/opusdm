//! `opusdm-player`: plays one audio or video file. The engine is the
//! `opusdm-media` crate; this binary is the window. See
//! `docs/superpowers/specs/2026-10-05-player-design.md`.

mod args;
mod audio_view;
mod view;
mod window;

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use std::process::ExitCode;
use std::sync::atomic::Ordering;

/// Load the shared OpusDM look (theme mode, fonts).
fn apply_theme() {
    opusdm_ui::install_app_css("opusdm-player", ".player-stage { background: black; }");
}

/// Fail early with a clear message when a local file is missing.
fn check_target(target: &str) -> Result<(), String> {
    if target.contains("://") || std::path::Path::new(target).exists() { return Ok(()); }
    Err(format!("{target}: no such file"))
}

fn main() -> ExitCode {
    let args = match args::parse(std::env::args().skip(1)).and_then(|a| check_target(&a.target).map(|_| a)) {
        Ok(a) => a,
        Err(msg) => {
            opusdm_ipc::err_log!("player", "{msg}");
            return ExitCode::from(1);
        }
    };
    // Each launch is its own process and window; the file is not a GApplication argument.
    let app = gtk::Application::builder().application_id("org.opusdm.Player").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_activate(move |app| {
        apply_theme();
        window::open(app, &args);
    });
    app.run_with_args::<&str>(&[]);
    if window::FAILED.load(Ordering::SeqCst) { ExitCode::from(1) } else { ExitCode::SUCCESS }
}
