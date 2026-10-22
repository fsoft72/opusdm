//! Library of `opusdm-lister`: the file window and the tool editors (Hotkeys,
//! File Types, Functions, Scripts), reused by `opusdm-settings`. The binary
//! in `main.rs` is spawned by opusdm-hub.
//!
//! The `terminal` feature (on by default) builds the VTE terminal drawer;
//! without it `terminal` is a stub, so a user of the editors does not link
//! VTE.

pub mod actions;
pub mod advanced_select;
pub mod ansi;
pub mod pathcomplete;
pub mod archive;
pub mod attr_dialogs;
pub mod batch_rename;
pub mod size_format;
pub mod start_target;
pub mod bookmarks_ui;
pub mod background;
pub mod build;
pub mod busy;
pub mod cache_list;
pub mod checkfit_ui;
pub mod client;
pub mod copy_as;
pub mod compress_dialog;
pub mod extract_dialog;
pub mod crypt_dialog;
pub mod cli_dialog;
pub mod clip;
pub mod close_guard;
pub mod commands;
pub mod config_cache;
pub mod columns;
pub mod viewpos;
pub mod context_menu;
pub mod dircache;
pub mod dnd;
pub mod filetype_form;
pub mod remote;
pub mod remote_ui;
pub mod remote_xfer;
pub mod file_kind;
pub mod filetypes_ui;
pub mod filter;
pub mod format_apply;
pub mod format_editor;
pub mod find_dialog;
pub mod find_search;
pub mod font_viewer;
pub mod function_editor;
pub mod gauge;
pub mod glyphs;
pub mod history;
pub mod hotkey_editor;
pub mod hidden;
pub mod hotkeys;
pub mod icons;
pub mod inbound;
pub mod job_submit;
pub mod keys;
pub mod link_dialogs;
pub mod menu_editor;
pub mod model;
pub mod nav_cache;
pub mod navpath;
pub mod open_default;
pub mod open_with;
pub mod output_window;
pub mod palette;
pub mod progress_panel;
pub mod passwd;
pub mod reselect;
pub mod scripts_editor;
pub mod script_editor;
pub mod scripts_ui;
pub mod select_criteria;
pub mod sizecache;
pub mod status_format;
#[cfg(feature = "terminal")]
pub mod terminal;
#[cfg(not(feature = "terminal"))]
#[path = "terminal_stub.rs"]
pub mod terminal;
pub mod theme;
pub mod typeahead;
pub mod tools;
pub mod trash_ui;
pub mod user_menu;
pub mod watcher;
pub mod image_viewer;
pub mod sidecar;
pub mod viewer;
pub mod viewer_load;
pub mod viewer_mode;
pub mod viewer_search;
pub mod window;

/// Debug log gated on OPUSDM_DEBUG=1: `[lister:<pid>] <msg>` on stderr.
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => { opusdm_ipc::debug_log!("lister", $($arg)*) };
}
