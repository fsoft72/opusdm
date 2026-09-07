//! opusdm-hub: the always-on controller.

mod desktop;
mod toolbar;

use opusdm_hub::{children, server, settings};

use children::AcquireOutcome;
use opusdm_ipc::sockpath::hub_sock_path;

fn main() {
    let sock_path = hub_sock_path();
    let listener = match children::acquire_socket(&sock_path) {
        AcquireOutcome::Bound(l) => l,
        AcquireOutcome::HandedOff => {
            eprintln!("opusdm-hub already running; asked it to raise its toolbar");
            std::process::exit(0);
        }
        AcquireOutcome::Failed(msg) => {
            eprintln!("opusdm-hub: {msg}");
            std::process::exit(1);
        }
    };

    let sock_for_spawn = sock_path.clone();
    let handle = server::spawn_server(
        listener,
        Box::new(move |start: Option<std::path::PathBuf>| {
            if let Err(e) = children::spawn_lister(&sock_for_spawn, start.as_deref()) {
                eprintln!("opusdm-hub: cannot spawn lister: {e}");
            }
        }),
    );

    children::install_sigchld(handle.cmd_tx.clone());

    // Open the first Lister at $HOME.
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if let Err(e) = children::spawn_lister(&sock_path, home.as_deref()) {
        eprintln!("opusdm-hub: cannot spawn initial lister: {e}");
    }

    let code = toolbar::run(handle, sock_path);
    std::process::exit(if code == gtk4::glib::ExitCode::SUCCESS { 0 } else { 1 });
}
