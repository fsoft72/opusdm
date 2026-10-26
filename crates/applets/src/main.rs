//! `opusdm-applets` binary. Placeholder host: owns the socket and answers the
//! protocol with placeholders until the real host is wired in.

use opusdm_applets::instance::{self, Exit};
use opusdm_applets::{protocol, server};

/// Acquire the single-instance socket, start the accept thread and wait for it.
fn main() {
    let listener = match instance::acquire() {
        Ok(l) => l,
        Err(Exit::AlreadyRunning) => {
            opusdm_ipc::err_log!(
                "applets",
                "opusdm-applets already running at {}",
                protocol::socket_path().display()
            );
            std::process::exit(opusdm_ipc::instance::EXIT_ALREADY_RUNNING);
        }
        Err(Exit::Failed(msg)) => {
            opusdm_ipc::err_log!("applets", "cannot start: {msg}");
            std::process::exit(1);
        }
    };
    let accept = match server::spawn_accept(listener) {
        Ok(h) => h,
        Err(e) => {
            opusdm_ipc::err_log!("applets", "cannot start the accept thread: {e}");
            std::process::exit(1);
        }
    };
    // The process lives as long as the accept thread; a panic there is fatal and visible.
    if accept.join().is_err() {
        opusdm_ipc::err_log!("applets", "accept thread panicked");
        std::process::exit(1);
    }
}
