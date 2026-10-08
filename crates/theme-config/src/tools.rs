//! The buttons that open the other OpusDM configuration tools (Hotkeys, File
//! Types, Functions, Apps, Input, Screens), each as its own process, and the
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

/// The tool buttons: label and action.
const TOOLS: [(&str, Action); 7] = [
    ("Hotkeys", Action::Spawn("opusdm-lister", &["--edit-hotkeys"])),
    ("File Types", Action::Spawn("opusdm-lister", &["--edit-filetypes"])),
    ("Manage Functions", Action::Spawn("opusdm-lister", &["--manage-functions"])),
    ("Button Banks", Action::OpenBankManager),
    ("Apps", Action::Spawn("opusdm-apps-editor", &[])),
    ("Input", Action::Spawn("opusdm-input-config", &[])),
    ("Screens", Action::Spawn("opusdm-screen", &[])),
];

/// Build the wrapping grid of buttons, one per entry in `TOOLS`.
pub fn build_row() -> gtk::FlowBox {
    let row = gtk::FlowBox::new();
    row.set_selection_mode(gtk::SelectionMode::None);
    row.set_homogeneous(true);
    row.set_max_children_per_line(4);
    row.set_row_spacing(6);
    row.set_column_spacing(6);
    for (label, action) in &TOOLS {
        let button = gtk::Button::with_label(label);
        button.connect_clicked(move |_| run(action));
        row.append(&button);
    }
    row
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
    let sent = std::os::unix::net::UnixStream::connect(&sock)
        .and_then(|mut stream| opusdm_ipc::frame::send(&mut stream, &opusdm_ipc::ToHub::OpenBankManager));
    if let Err(e) = sent {
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
