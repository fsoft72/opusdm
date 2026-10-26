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
mod screen_hotplug;
mod toolbar;

use opusdm_hub::{children, server};

use children::AcquireOutcome;
use opusdm_ipc::sockpath::hub_sock_path;

fn main() {
    // `opusdm-hub --run FUNCTION`: a client of the running hub, not a hub.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = opusdm_hub::remote::run_from_args(&args) {
        std::process::exit(code);
    }
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

    // Read once, here: the server, the desktop and the startup code below
    // all start from the same copy.
    let config = opusdm_hub::startup_config::StartupConfig::load();

    let handle = server::spawn_server_with(
        listener,
        Box::new(move |start: Option<std::path::PathBuf>| {
            if let Err(e) = children::spawn_lister(&sock_for_spawn, start.as_deref()) {
                opusdm_ipc::err_log!("opusdm-hub", "cannot spawn lister: {e}");
            }
        }),
        server::ServerOptions::production_with(config.clone()),
    );

    // Answer "show in folder" requests from browsers and other programs.
    opusdm_hub::file_manager::start(handle.cmd_tx.clone());
    // Folders opened through xdg (xdg-open, file choosers) go to a Lister.
    opusdm_hub::folder_handler::ensure_default();

    // First run: start with the default file types (never overwrites a file).
    if opusdm_ipc::filetypes::seed_default_filetypes() {
        opusdm_ipc::debug_log!("opusdm-hub", "wrote the default file types");
    }

    // Keep the saved mouse, touchpad and keyboard settings applied.
    input_apply::start();

    // Put the saved screen layout back when monitors are plugged or unplugged.
    screen_hotplug::start();

    // A program asking to bring its window forward (a browser tab opened for
    // a login, an app clicked in the tray) gets it, even on another desktop.
    opusdm_hub::activation_follow::start();

    // The polkit agent: password dialogs for pkexec and the other programs
    // that need administrator rights. A second one (restarted hub) exits.
    if let Err(e) = children::spawn_tool("opusdm-polkit") {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start opusdm-polkit: {e}");
    }

    // The notification server: popups for browsers, mail and notify-send,
    // and the history of the panel's bell. A second one (restarted hub)
    // exits; it also leaves an already running server (dunst...) alone.
    if let Err(e) = children::spawn_tool("opusdm-notify") {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start opusdm-notify: {e}");
    }

    // The desktop applets (X11 only): a supervisor thread starts
    // opusdm-applets and restarts it when it exits. A second one (restarted
    // hub) exits and the supervisor waits for the old one.
    opusdm_hub::applet_host::start();

    // Launch the Startup Apps of Settings, each after its own wait time,
    // unless the hub was restarted with --no-startup-apps.
    let desktop_settings = &config.desktop;
    if opusdm_hub::startup_apps::skipped_by_args(&args) {
        opusdm_ipc::debug_log!("opusdm-hub", "startup apps skipped ({})", opusdm_hub::startup_apps::NO_STARTUP_APPS_ARG);
    } else {
        opusdm_hub::startup_apps::start(&desktop_settings.startup_apps);
    }

    opusdm_hub::channels::send_or_log(&handle.cmd_tx, server::HubCmd::EmitScriptEvent(opusdm_ipc::bindings::ScriptEvent::Startup), "hub command");

    if let Err(e) = children::install_sigchld(handle.cmd_tx.clone()) {
        opusdm_ipc::err_log!("opusdm-hub", "cannot watch for exited children (SIGCHLD), they are not reaped: {e}");
    }

    // Open the first Lister at $HOME, unless Settings turned it off.
    if desktop_settings.open_lister_at_start {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        if let Err(e) = children::spawn_lister(&sock_path, home.as_deref()) {
            opusdm_ipc::err_log!("opusdm-hub", "cannot spawn initial lister: {e}");
        }
    }

    let code = toolbar::run(handle, sock_path, config);
    std::process::exit(if code == gtk4::glib::ExitCode::SUCCESS { 0 } else { 1 });
}
