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
    opusdm_ui::install_app_css("opusdm-screen", "");
}

fn main() -> gtk::glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let demo = args.iter().any(|a| a == DEMO_FLAG);
    let gtk_args: Vec<&String> = args.iter().filter(|a| *a != DEMO_FLAG).collect();
    let app = gtk::Application::builder().application_id("org.opusdm.Screen").build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
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
        let backend: Box<dyn ScreenBackend + Send> = if demo { Box::new(FakeBackend::new(two_monitor_system())) } else { for_session() };
        ui::open(app, backend);
    });
    app.run_with_args(&gtk_args)
}
