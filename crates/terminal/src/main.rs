//! `opusdm-terminal`: a VTE terminal emulator with tabs and Terminator-style
//! split panes, its own colour themes, link recognition, export of the
//! scrollback and drop of files from a Lister or the desktop.

mod actions;
mod cli;
mod config;
mod layout;
mod links;
mod pane;
mod prefs;
mod quote;
mod shared;
mod theme;
mod theme_editor;
mod window;

use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;

/// Style of the panes: the focused one of a split tab gets an outline.
const TERMINAL_CSS: &str = "
.term-pane { border: 1px solid transparent; }
.term-pane.focused { border-color: #3daee9; }
window.opusdm-terminal notebook > header tab { padding: 2px 6px; min-height: 0; }
window.opusdm-terminal notebook > header tab button { min-height: 0; min-width: 0; padding: 0; }
";

/// Load the shared OpusDM look (theme mode, fonts) plus the terminal style.
fn apply_theme() {
    let settings = opusdm_ipc::settings::load_settings("opusdm-terminal");
    opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    let css = format!("{} {} {TERMINAL_CSS}", opusdm_ui::chrome_css(settings.appearance.theme_mode), opusdm_ui::ui_look_css(&settings));
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

fn main() -> gtk::glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match cli::parse_args(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("opusdm-terminal: {e}\n{}", cli::USAGE);
            return gtk::glib::ExitCode::FAILURE;
        }
    };
    if options.help {
        println!("{}", cli::USAGE);
        return gtk::glib::ExitCode::SUCCESS;
    }

    // Every launch is its own process with its own window, like the other
    // terminals started by `exec:` hotkeys.
    let app = gtk::Application::builder()
        .application_id("org.opusdm.Terminal")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| {
        apply_theme();
        let shared = shared::Shared::new(app);
        let cwd = options.working_directory.clone();
        window::open_window(&shared, cwd.as_deref(), options.command.as_deref());
    });
    // The arguments were parsed above; GApplication must not see them.
    app.run_with_args(&["opusdm-terminal"])
}
