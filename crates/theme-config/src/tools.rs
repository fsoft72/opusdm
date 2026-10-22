//! The buttons that open the other OpusDM configuration tools (Functions,
//! Scripts, Apps, Screens), each as its own process, and the Button Banks
//! manager, which lives in the hub. They sit under the list of the Advanced tab.

use gtk4 as gtk;
use gtk::prelude::*;
use std::path::PathBuf;

/// What a tool button does when clicked.
pub enum Action {
    /// Start a sibling binary with these arguments.
    Spawn(&'static str, &'static [&'static str]),
    /// Ask the running hub to open the button bank manager.
    OpenBankManager,
}

/// The buttons under the list of the Advanced tab: label and action.
const TOOL_BUTTONS: [(&str, Action); 5] = [
    ("Manage Functions", Action::Spawn("opusdm-lister", &["--manage-functions"])),
    ("Scripts (JavaScript)", Action::Spawn("opusdm-lister", &["--scripts"])),
    ("Button Banks", Action::OpenBankManager),
    ("Apps", Action::Spawn("opusdm-apps-editor", &[])),
    ("Screens", Action::Spawn("opusdm-screen", &[])),
];

/// Build the box with one button per tool that has its own window.
pub fn build_tool_buttons() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    for (label, action) in &TOOL_BUTTONS {
        let button = gtk::Button::with_label(label);
        button.set_halign(gtk::Align::Fill);
        button.connect_clicked(move |_| run(action));
        page.append(&button);
    }
    page
}

/// Run the action of a tool button.
pub fn run(action: &Action) {
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
