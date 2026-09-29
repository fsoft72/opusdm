//! "Check Fit" command: ask the hub whether the selection fits at DEST and
//! show the numbers.

use crate::model::format_size;
use crate::window::Ui;
use opusdm_ipc::ToHub;
use std::path::Path;
use std::rc::Rc;

/// Ask the hub to check the selection against the DEST Lister.
pub fn start(ui: &Rc<Ui>) {
    if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
        crate::actions::flash(ui, &msg);
        return;
    }
    let srcs = ui.selected_paths();
    if srcs.is_empty() {
        crate::actions::flash(ui, "nothing selected");
        return;
    }
    crate::actions::flash(ui, "checking free space...");
    let _ = ui.hub.client.send(&ToHub::CheckFit { srcs, dest_dir: None });
}

/// The requester text for a hub answer.
pub fn fit_message(dest_dir: &Path, needed: u64, free: u64, fits: bool, percent: u64) -> String {
    let verdict = if fits { "Everything fits.".to_string() } else { format!("Only {percent}% fits.") };
    format!(
        "{verdict}\nNeeded: {}\nFree at {}: {}",
        format_size(false, needed),
        dest_dir.display(),
        format_size(false, free)
    )
}

/// Show the answer the hub sent.
pub fn show_result(ui: &Rc<Ui>, dest_dir: &Path, needed: u64, free: u64, fits: bool, percent: u64) {
    crate::actions::flash(ui, "");
    let text = fit_message(dest_dir, needed, free, fits, percent);
    opusdm_ui::requester(&ui.window, "Check Fit", &text, None, &[("OK", true)], |_| {});
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_states_the_verdict_and_both_sizes() {
        let ok = fit_message(Path::new("/d"), 1024, 4096, true, 100);
        assert!(ok.starts_with("Everything fits."));
        assert!(ok.contains("Needed: 1.0K") && ok.contains("Free at /d: 4.0K"));
        let bad = fit_message(Path::new("/d"), 2048, 1024, false, 50);
        assert!(bad.starts_with("Only 50% fits."));
    }
}
