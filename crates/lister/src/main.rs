//! opusdm-lister: one file-manager window. Spawned by opusdm-hub.

mod actions;
mod client;
mod dnd;
mod model;
mod window;

use std::path::PathBuf;

fn main() {
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
    let (client, lister_id, roles) = match client::Client::connect(&sock_path, pid, &start_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("opusdm-lister: cannot reach hub: {e}");
            std::process::exit(1);
        }
    };

    let code = window::run(client, lister_id, roles, start_path);
    std::process::exit(code as i32);
}
