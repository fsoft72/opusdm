//! The tabs of buttons that open the other OpusDM configuration tools (Hotkeys, File
//! Types, Functions, Apps, Input, Screens, Network), each as its own process, and the
//! Button Banks manager, which lives in the hub.

use gtk4 as gtk;
use gtk::prelude::*;
use std::path::PathBuf;

/// What a tool button does when clicked.
enum Action {
    /// Start a sibling binary with these arguments.
    Spawn(&'static str, &'static [&'static str]),
    /// Ask the running hub to open the button bank manager.
    OpenBankManager,
}

/// The buttons of the Advanced tab: label and action.
const ADVANCED_TOOLS: [(&str, Action); 6] = [
    ("Hotkeys", Action::Spawn("opusdm-lister", &["--edit-hotkeys"])),
    ("File Types", Action::Spawn("opusdm-lister", &["--edit-filetypes"])),
    ("Manage Functions", Action::Spawn("opusdm-lister", &["--manage-functions"])),
    ("Scripts (JavaScript)", Action::Spawn("opusdm-lister", &["--scripts"])),
    ("Button Banks", Action::OpenBankManager),
    ("Apps", Action::Spawn("opusdm-apps-editor", &[])),
];

/// The buttons of the System tab: label and action.
const SYSTEM_TOOLS: [(&str, Action); 3] = [
    ("Input", Action::Spawn("opusdm-input-config", &[])),
    ("Screens", Action::Spawn("opusdm-screen", &[])),
    ("Network", Action::Spawn("opusdm-network-manager", &[])),
];

/// Build the page of the Advanced tab.
pub fn build_advanced_page() -> gtk::Box {
    build_page(&ADVANCED_TOOLS)
}

/// Build the page of the System tab.
pub fn build_system_page() -> gtk::Box {
    build_page(&SYSTEM_TOOLS)
}

/// Build a tab page with one button per entry of `tools`.
fn build_page(tools: &'static [(&str, Action)]) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_margin_top(12);
    page.set_margin_bottom(12);
    page.set_margin_start(12);
    page.set_margin_end(12);
    for (label, action) in tools {
        let button = gtk::Button::with_label(label);
        button.set_halign(gtk::Align::Start);
        button.connect_clicked(move |_| run(action));
        page.append(&button);
    }
    page
}

/// Run the action of a tool button.
fn run(action: &Action) {
    match action {
        Action::Spawn(bin, args) => spawn(bin, args),
        Action::OpenBankManager => open_bank_manager(),
    }
}

/// Tell the running hub to open the button bank manager.
fn open_bank_manager() {
    let sock = opusdm_ipc::sockpath::hub_sock_path();
    if let Err(e) = opusdm_ipc::sockpath::send_oneshot(&sock, &[opusdm_ipc::ToHub::OpenBankManager]) {
        opusdm_ipc::err_log!("theme-config", "cannot reach the hub to open the button banks: {e}");
    }
}

/// Start `bin` with `args`, detached. The binary is looked up next to the
/// running executable first, then on `PATH`.
fn spawn(bin: &str, args: &[&str]) {
    let path = sibling_bin(bin);
    if let Err(e) = std::process::Command::new(&path).args(args).spawn() {
        opusdm_ipc::err_log!("theme-config", "cannot launch {}: {e}", path.display());
    }
}

/// Path of `name` in the folder of the running executable, or the bare name
/// when that folder is unknown.
fn sibling_bin(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}
