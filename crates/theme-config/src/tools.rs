//! The row of buttons that open the other OpusDM configuration tools
//! (Hotkeys, File Types, Apps, Input, Screens), each as its own process.

use gtk4 as gtk;
use gtk::prelude::*;
use std::path::PathBuf;

/// One tool button: its label, the sibling binary to start and its arguments.
const TOOLS: [(&str, &str, &[&str]); 5] = [
    ("Hotkeys", "opusdm-lister", &["--edit-hotkeys"]),
    ("File Types", "opusdm-lister", &["--edit-filetypes"]),
    ("Apps", "opusdm-apps-editor", &[]),
    ("Input", "opusdm-input-config", &[]),
    ("Screens", "opusdm-screen", &[]),
];

/// Build the row of buttons, one per tool in `TOOLS`.
pub fn build_row() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_homogeneous(true);
    for (label, bin, args) in TOOLS {
        let button = gtk::Button::with_label(label);
        button.connect_clicked(move |_| spawn(bin, args));
        row.append(&button);
    }
    row
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
