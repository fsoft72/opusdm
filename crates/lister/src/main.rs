//! opusdm-lister: one file-manager window. Spawned by opusdm-hub.

mod actions;
mod advanced_select;
mod ansi;
mod archive;
mod attr_dialogs;
mod batch_rename;
mod binding_ui;
mod build;
mod cache_list;
mod checkfit_ui;
mod client;
mod copy_as;
mod crypt_dialog;
mod clip;
mod columns;
mod context_menu;
mod dircache;
mod dnd;
mod filetype_form;
mod remote;
mod remote_ui;
mod filetypes_ui;
mod filter;
mod format_apply;
mod format_editor;
mod find_dialog;
mod find_search;
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
mod palette;
mod passwd;
mod preview;
mod reselect;
mod scripts_editor;
mod select_criteria;
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

fn main() {
    crate::dlog!("starting");
    let sock_path = match std::env::var_os(opusdm_ipc::sockpath::ENV_SOCK) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => {
            eprintln!("opusdm-lister: run opusdm-hub (this process is launched by the hub)");
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
    let (client, lister_id, roles, settings, bindings, formats) =
        match client::Client::connect(&sock_path, pid, &start_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("opusdm-lister: cannot reach hub: {e}");
                std::process::exit(1);
            }
        };

    crate::dlog!("connected to hub, lister_id={lister_id:?}, start={}", start_path.display());
    let code = build::run(client, lister_id, roles, settings, bindings, formats, start_path);
    crate::dlog!("event loop exited, code={code:?}");
    std::process::exit(if code == gtk4::glib::ExitCode::SUCCESS { 0 } else { 1 });
}
