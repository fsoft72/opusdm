//! "Copy As..." / "Move As..." dialog: one naming pattern (`*` stands for
//! the original name) applied to every selected entry, with a live preview
//! of the resulting names.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::glob::expand_all;
use opusdm_ipc::{JobOptions, Op};
use std::rc::Rc;

/// Pattern the dialog starts with: keep the original names.
const DEFAULT_PATTERN: &str = "*";

/// How many "old -> new" lines the preview shows before summarising.
const PREVIEW_LINES: usize = 8;

/// The preview text for `pattern` over `originals`, or the error that
/// blocks the job.
pub fn preview_text(pattern: &str, originals: &[String]) -> Result<String, String> {
    let names = expand_all(pattern, originals)?;
    let mut lines: Vec<String> = originals
        .iter()
        .zip(&names)
        .take(PREVIEW_LINES)
        .map(|(old, new)| format!("{old}  ->  {new}"))
        .collect();
    if originals.len() > PREVIEW_LINES {
        lines.push(format!("... and {} more", originals.len() - PREVIEW_LINES));
    }
    Ok(lines.join("\n"))
}

/// Open the dialog for the current selection. `moving` picks `MoveAs` over
/// `CopyAs`. The destination is the DEST Lister, resolved by the hub.
pub fn open(ui: &Rc<Ui>, moving: bool) {
    if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
        crate::actions::flash(ui, &msg);
        return;
    }
    let originals = ui.selected_names();
    if originals.is_empty() {
        crate::actions::flash(ui, "nothing selected");
        return;
    }
    let title = if moving { "Move As" } else { "Copy As" };
    let shell = opusdm_ui::DialogShell::new(&ui.window, title).size(460, 300);

    let entry = gtk::Entry::new();
    entry.set_text(DEFAULT_PATTERN);
    entry.set_activates_default(true);
    shell.content().append(&gtk::Label::new(Some("New name (* = original name)")));
    shell.content().append(&entry);
    let preview = gtk::Label::new(None);
    preview.set_xalign(0.0);
    preview.set_selectable(true);
    shell.content().append(&preview);

    let ok_btn = shell.button("OK");
    shell.close_button("Cancel");

    let refresh = {
        let (entry, preview, ok_btn, originals) =
            (entry.clone(), preview.clone(), ok_btn.clone(), originals.clone());
        move || match preview_text(entry.text().as_str(), &originals) {
            Ok(text) => {
                preview.set_text(&text);
                ok_btn.set_sensitive(true);
            }
            Err(msg) => {
                preview.set_text(&msg);
                ok_btn.set_sensitive(false);
            }
        }
    };
    refresh();
    entry.connect_changed(move |_| refresh());

    let window = shell.window().clone();
    let ui = ui.clone();
    ok_btn.connect_clicked(move |_| {
        let Ok(new_names) = expand_all(entry.text().as_str(), &originals) else { return };
        let op = if moving { Op::MoveAs { new_names } } else { Op::CopyAs { new_names } };
        crate::job_submit::submit_job(&ui, op, ui.selected_paths(), None, JobOptions::default());
        window.close();
    });
    shell.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_lists_old_and_new_names() {
        let orig = vec!["a.txt".to_string(), "b.txt".to_string()];
        let text = preview_text("*.bak", &orig).unwrap();
        assert_eq!(text, "a.txt  ->  a.txt.bak\nb.txt  ->  b.txt.bak");
    }

    #[test]
    fn preview_summarises_long_selections() {
        let orig: Vec<String> = (0..12).map(|i| format!("f{i}")).collect();
        let text = preview_text("*", &orig).unwrap();
        assert!(text.ends_with("... and 4 more"));
        assert_eq!(text.lines().count(), PREVIEW_LINES + 1);
    }

    #[test]
    fn preview_reports_colliding_names() {
        let orig = vec!["a".to_string(), "b".to_string()];
        assert!(preview_text("same", &orig).is_err());
    }
}
