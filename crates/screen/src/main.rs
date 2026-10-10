//! `opusdm-screen`: arrange the monitors, duplicate or extend the desktop,
//! pick resolution, refresh rate, rotation and scale. Apply asks to keep the
//! result and goes back after 15 seconds without an answer. See
//! `docs/superpowers/specs/2026-10-08-screen-manager-design.md`.
//!
//! `opusdm-screen --demo` runs on two made up monitors and never touches the
//! real screens.

mod ui;

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_screen::backend::{for_session, session_problem, ScreenBackend};
use opusdm_screen::fake::{two_monitor_system, FakeBackend};

/// The command line flag for the demo mode.
const DEMO_FLAG: &str = "--demo";

/// Load the shared OpusDM look (theme mode, fonts).
fn apply_theme() {
    let settings = opusdm_ipc::settings::load_settings("opusdm-screen");
    opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    let css = format!("{} {}", opusdm_ui::chrome_css(settings.appearance.theme_mode), opusdm_ui::ui_look_css(&settings));
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

fn main() -> gtk::glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let demo = args.iter().any(|a| a == DEMO_FLAG);
    let gtk_args: Vec<&String> = args.iter().filter(|a| *a != DEMO_FLAG).collect();
    let app = gtk::Application::builder().application_id("org.opusdm.Screen").build();
    app.connect_activate(move |app| {
        // A second launch only raises the open window.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        apply_theme();
        let problem = if demo { None } else { session_problem() };
        if let Some(problem) = problem {
            return ui::open_problem(app, &problem);
        }
        let backend: Box<dyn ScreenBackend> = if demo { Box::new(FakeBackend::new(two_monitor_system())) } else { for_session() };
        ui::open(app, backend);
    });
    app.run_with_args(&gtk_args)
}
