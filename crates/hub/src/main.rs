//! opusdm-hub: the always-on controller.

mod children;
mod jobs;
mod roles;
mod server;

use children::AcquireOutcome;
use opusdm_ipc::sockpath::hub_sock_path;
use server::{HubCmd, HubEvent};

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

    // Headless event drain. Task 10 replaces this block with the GTK toolbar
    // main loop.
    for ev in handle.evt_rx.iter() {
        match ev {
            HubEvent::ListerRegistered { id, pid, path } => {
                eprintln!("[hub] lister {id:?} pid={pid} at {}", path.display());
            }
            HubEvent::ListerGone(id) => eprintln!("[hub] lister {id:?} gone"),
            HubEvent::AllListersGone => eprintln!("[hub] no listers open (toolbar still up in v10)"),
            HubEvent::Roles { source, dest, .. } => {
                eprintln!("[hub] roles: source={source:?} dest={dest:?}");
            }
            HubEvent::JobUpdate { job_id, label, done, total } => {
                eprintln!("[hub] job {job_id}: {label} {done}/{total}");
            }
            HubEvent::JobFinished { job_id, ok, errors } => {
                eprintln!("[hub] job {job_id} done ok={ok} errors={errors}");
            }
            HubEvent::RaiseToolbar => eprintln!("[hub] raise-toolbar request"),
        }
    }
    let _ = handle.cmd_tx.send(HubCmd::Shutdown);
}
