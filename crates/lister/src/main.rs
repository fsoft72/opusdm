//! opusdm-lister: one file-manager window. Spawned by opusdm-hub.

mod actions;
mod advanced_select;
mod ansi;
mod archive;
mod attr_dialogs;
mod batch_rename;
mod size_format;
mod binding_ui;
mod bookmarks_ui;
mod build;
mod busy;
mod cache_list;
mod checkfit_ui;
mod client;
mod copy_as;
mod crypt_dialog;
mod cli_dialog;
mod clip;
mod commands;
mod columns;
mod context_menu;
mod dircache;
mod dnd;
mod filetype_form;
mod remote;
mod remote_ui;
mod remote_xfer;
mod filetypes_ui;
mod filter;
mod format_apply;
mod format_editor;
mod find_dialog;
mod find_search;
mod font_viewer;
mod function_editor;
mod gauge;
mod glyphs;
mod history;
mod hotkey_editor;
mod hidden;
mod hotkeys;
mod icons;
mod job_submit;
mod keys;
mod link_dialogs;
mod menu_editor;
mod model;
mod nav_cache;
mod navpath;
mod output_window;
mod palette;
mod passwd;
mod preview;
mod reselect;
mod scripts_editor;
mod select_criteria;
mod sizecache;
mod slideshow;
mod status_format;
mod terminal;
mod theme;
mod typeahead;
mod tools;
mod user_menu;
mod watcher;
mod viewer;
mod viewer_load;
mod viewer_mode;
mod viewer_search;
mod window;

use std::path::PathBuf;

/// Debug log gated on OPUSDM_DEBUG=1: `[lister:<pid>] <msg>` on stderr.
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => { opusdm_ipc::debug_log!("lister", $($arg)*) };
}

/// Command line flag that makes the Lister open only the File Types editor.
const EDIT_FILETYPES_FLAG: &str = "--edit-filetypes";

/// Run only the File Types editor, with no hub connection and no file window.
/// The application exits when the editor is closed.
fn run_filetypes_editor() -> gtk4::glib::ExitCode {
    use gtk4::prelude::*;
    let app = gtk4::Application::builder().application_id("org.opusdm.FileTypes").build();
    app.connect_activate(|app| {
        let settings = opusdm_ipc::settings::load_settings("opusdm-lister");
        opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(&theme::generate_css(&settings));
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("no default GDK display: cannot run without one"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        filetypes_ui::open_standalone(app);
    });
    app.run_with_args::<&str>(&[])
}

fn main() {
    if std::env::args().any(|a| a == EDIT_FILETYPES_FLAG) {
        let code = run_filetypes_editor();
        std::process::exit(if code == gtk4::glib::ExitCode::SUCCESS { 0 } else { 1 });
    }
    crate::dlog!("starting");
    let sock_path = match std::env::var_os(opusdm_ipc::sockpath::ENV_SOCK) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => {
            opusdm_ipc::err_log!("opusdm-lister", "run opusdm-hub (this process is launched by the hub)");
            std::process::exit(1);
        }
    };

    let start_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));

    let pid = std::process::id();
    let (client, lister_id, roles, settings, bindings, formats, functions) =
        match client::Client::connect(&sock_path, pid, &start_path) {
            Ok(t) => t,
            Err(e) => {
                opusdm_ipc::err_log!("opusdm-lister", "cannot reach hub: {e}");
                std::process::exit(1);
            }
        };

    crate::dlog!("connected to hub, lister_id={lister_id:?}, start={}", start_path.display());
    let code = build::run(client, lister_id, roles, settings, bindings, formats, functions, start_path);
    crate::dlog!("event loop exited, code={code:?}");
    archive::cleanup_temp_dirs();
    std::process::exit(if code == gtk4::glib::ExitCode::SUCCESS { 0 } else { 1 });
}
