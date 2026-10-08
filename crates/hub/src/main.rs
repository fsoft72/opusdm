//! opusdm-hub: the always-on controller.

mod background;
mod desktop;
mod desktop_ops;
mod bank_windows;
mod group_window;
mod progress_window;
mod desktop_menu;
mod desktop_windows;
mod groups_ui;
mod input_apply;
mod toolbar;

use opusdm_hub::{children, server, settings};

use children::AcquireOutcome;
use opusdm_ipc::sockpath::hub_sock_path;

fn main() {
    children::add_own_dir_to_path();
    let sock_path = hub_sock_path();
    let listener = match children::acquire_socket(&sock_path) {
        AcquireOutcome::Bound(l) => l,
        AcquireOutcome::HandedOff => {
            opusdm_ipc::err_log!("opusdm-hub", "already running; asked it to raise its toolbar");
            std::process::exit(0);
        }
        AcquireOutcome::Failed(msg) => {
            opusdm_ipc::err_log!("opusdm-hub", "{msg}");
            std::process::exit(1);
        }
    };

    let sock_for_spawn = sock_path.clone();
    // First run: Alt+1 / Alt+2 switch the virtual desktop. Before the hub
    // loads the bindings, and never overwrites a file.
    if opusdm_ipc::bindings::seed_default_hotkeys() {
        opusdm_ipc::debug_log!("opusdm-hub", "wrote the default hotkeys");
    }

    let handle = server::spawn_server_with(
        listener,
        Box::new(move |start: Option<std::path::PathBuf>| {
            if let Err(e) = children::spawn_lister(&sock_for_spawn, start.as_deref()) {
                opusdm_ipc::err_log!("opusdm-hub", "cannot spawn lister: {e}");
            }
        }),
        server::ServerOptions::production(),
    );

    // Answer "show in folder" requests from browsers and other programs.
    opusdm_hub::file_manager::start(handle.cmd_tx.clone());

    // First run: start with the default file types (never overwrites a file).
    if opusdm_ipc::filetypes::seed_default_filetypes() {
        opusdm_ipc::debug_log!("opusdm-hub", "wrote the default file types");
    }

    // Keep the saved mouse, touchpad and keyboard settings applied.
    input_apply::start();

    // Launch the Startup Apps of Settings, each after its own wait time.
    let desktop_settings = opusdm_hub::desktop_settings::load();
    opusdm_hub::startup_apps::start(&desktop_settings.startup_apps);

    let _ = handle.cmd_tx.send(server::HubCmd::EmitScriptEvent(opusdm_ipc::bindings::ScriptEvent::Startup));

    children::install_sigchld(handle.cmd_tx.clone());

    // Open the first Lister at $HOME, unless Settings turned it off.
    if desktop_settings.open_lister_at_start {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        if let Err(e) = children::spawn_lister(&sock_path, home.as_deref()) {
            opusdm_ipc::err_log!("opusdm-hub", "cannot spawn initial lister: {e}");
        }
    }

    let code = toolbar::run(handle, sock_path);
    std::process::exit(if code == gtk4::glib::ExitCode::SUCCESS { 0 } else { 1 });
}
