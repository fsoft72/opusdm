//! opusdm-lister: one file-manager window. Spawned by opusdm-hub.

mod actions;
mod archive;
mod batch_rename;
mod build;
mod client;
mod clip;
mod columns;
mod context_menu;
mod dnd;
mod filter;
mod glyphs;
mod icons;
mod keys;
mod model;
mod palette;
mod preview;
mod terminal;
mod theme;
mod tools;
mod watcher;
mod window;

use std::path::PathBuf;

/// Debug log gated on OPUSDM_DEBUG=1: `[lister:<pid>] <msg>` on stderr.
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => {
        if std::env::var_os("OPUSDM_DEBUG").is_some() {
            eprintln!("[lister:{}] {}", std::process::id(), format_args!($($arg)*));
        }
    };
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
    let (client, lister_id, roles, settings) =
        match client::Client::connect(&sock_path, pid, &start_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("opusdm-lister: cannot reach hub: {e}");
                std::process::exit(1);
            }
        };

    crate::dlog!("connected to hub, lister_id={lister_id:?}, start={}", start_path.display());
    let code = build::run(client, lister_id, roles, settings, start_path);
    crate::dlog!("event loop exited, code={code:?}");
    std::process::exit(if code == gtk4::glib::ExitCode::SUCCESS { 0 } else { 1 });
}
