//! `opusdm-shot`: screenshot with area selection and a basic editor. A hotkey
//! bound to `exec:opusdm-shot` freezes the screen, the user drags the area to
//! keep, and an editor opens for lines, arrows and text before saving. See
//! `docs/superpowers/specs/2026-10-05-screenshot-design.md`.

mod annotations;
mod capture;
mod config;
mod editor;
mod export;
mod icons;
mod overlay;
mod render;
mod selection;

use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;

/// Time given to the compositor to unmap closed windows before a new capture.
const RESTART_SETTLE_MS: u64 = 250;

/// Load the shared OpusDM look (theme mode, fonts) plus the editor style.
fn apply_theme() {
    let settings = opusdm_ipc::settings::load_settings("opusdm-shot");
    opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    let css = format!("{} {} {}", opusdm_ui::chrome_css(settings.appearance.theme_mode), opusdm_ui::ui_look_css(&settings), editor::editor_css());
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

/// The primary monitor and its geometry (x, y, width, height).
fn primary_monitor() -> Result<(gdk::Monitor, opusdm_ui::MonitorRect), String> {
    let display = gdk::Display::default().ok_or("no display")?;
    let monitor = display
        .monitors()
        .item(0)
        .and_then(|o| o.downcast::<gdk::Monitor>().ok())
        .ok_or("no monitor found")?;
    let g = monitor.geometry();
    Ok((monitor, (g.x(), g.y(), g.width(), g.height())))
}

/// Wait the configured delay (none by default), then capture and start.
fn start_after_delay(app: &gtk::Application) {
    let delay = config::load().delay_secs;
    if delay == 0 { return start(app); }
    // No window is open while waiting: hold the app so it does not quit.
    let hold = app.hold();
    let app = app.clone();
    gtk::glib::timeout_add_seconds_local_once(delay, move || {
        start(&app);
        drop(hold);
    });
}

/// Capture the primary monitor and start the overlay, then the editor.
fn start(app: &gtk::Application) {
    let (monitor, (x, y, w, h)) = match primary_monitor() {
        Ok(m) => m,
        Err(e) => return fail(app, &e),
    };
    // The screen is captured before any window of ours is shown.
    let screen = match capture::capture(x, y, w as u32, h as u32) {
        Ok(s) => s,
        Err(e) => return fail(app, &format!("capture failed: {e}")),
    };
    let app2 = app.clone();
    overlay::open(app, &monitor, screen, move |cropped| editor::open(&app2, cropped));
}

/// Close every open window and start a fresh screenshot. The app is held while
/// the windows go away, and the capture waits so they are off screen first.
fn restart(app: &gtk::Application) {
    let hold = app.hold();
    for window in app.windows() { window.close(); }
    let app = app.clone();
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(RESTART_SETTLE_MS), move || {
        start_after_delay(&app);
        drop(hold);
    });
}

/// Log `message` and quit with a failure.
fn fail(app: &gtk::Application, message: &str) {
    opusdm_ipc::err_log!("opusdm-shot", "{message}");
    app.quit();
    std::process::exit(1);
}

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.Shot").build();
    app.connect_activate(|app| {
        apply_theme();
        // A second launch while a window is open replaces it with a new shot.
        if !app.windows().is_empty() { return restart(app); }
        start_after_delay(app);
    });
    app.run()
}
