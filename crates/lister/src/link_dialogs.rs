//! "Make Link...", "Join Files..." and "Split File..." dialogs.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::glob::expand_all;
use opusdm_ipc::{JobOptions, Op};
use std::path::PathBuf;
use std::rc::Rc;

/// Bytes in a KiB.
const KIB: u64 = 1024;

/// Part size the Split dialog starts with.
const DEFAULT_SPLIT_SIZE: &str = "4M";

/// Parse a size such as `4096`, `512K`, `4M`, `1G`, `4MiB` (powers of 1024,
/// case-insensitive, whole numbers only). Zero is rejected.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let bad = || format!("not a size (use e.g. 4096, 512K, 4M, 1G): {text:?}");
    let t = text.trim().to_ascii_lowercase();
    let t = t.strip_suffix("ib").or_else(|| t.strip_suffix('b')).unwrap_or(&t);
    let (digits, factor) = match t.chars().last() {
        Some('k') => (&t[..t.len() - 1], KIB),
        Some('m') => (&t[..t.len() - 1], KIB * KIB),
        Some('g') => (&t[..t.len() - 1], KIB * KIB * KIB),
        _ => (t, 1),
    };
    let n: u64 = digits.trim().parse().map_err(|_| bad())?;
    match n.checked_mul(factor) {
        Some(0) | None => Err(bad()),
        Some(bytes) => Ok(bytes),
    }
}

/// The target name the Join dialog proposes: `movie.mkv.001` becomes
/// `movie.mkv`, anything else gets `.joined` appended.
pub fn default_join_name(first: &str) -> String {
    if let Some((stem, ext)) = first.rsplit_once('.') {
        if ext.len() >= 2 && ext.chars().all(|c| c.is_ascii_digit()) && !stem.is_empty() {
            return stem.to_string();
        }
    }
    format!("{first}.joined")
}

/// `None` (the hub uses the DEST Lister) when this Lister is the SOURCE with
/// a DEST set, otherwise the current directory.
fn dest_or_cwd(ui: &Rc<Ui>) -> Option<PathBuf> {
    match crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
        Ok(()) => None,
        Err(_) => Some(ui.cwd_path()),
    }
}

/// Open the link dialog: symbolic or hard, with a naming pattern (`*` = the
/// original name), into the DEST Lister.
pub fn open_link(ui: &Rc<Ui>) {
    if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
        crate::actions::flash(ui, &msg);
        return;
    }
    let originals = ui.selected_names();
    if originals.is_empty() {
        crate::actions::flash(ui, "nothing selected");
        return;
    }
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Make Link").size(460, 320);
    let kind = gtk::DropDown::from_strings(&["Symbolic link", "Hard link"]);
    shell.content().append(&kind);
    shell.content().append(&gtk::Label::new(Some("Link name (* = original name)")));
    let entry = gtk::Entry::new();
    entry.set_text("*");
    shell.content().append(&entry);
    let preview = gtk::Label::new(None);
    preview.set_xalign(0.0);
    shell.content().append(&preview);

    let ok_btn = shell.button("Create");
    shell.close_button("Cancel");
    let refresh = {
        let (entry, preview, ok_btn, originals) =
            (entry.clone(), preview.clone(), ok_btn.clone(), originals.clone());
        move || match crate::copy_as::preview_text(entry.text().as_str(), &originals) {
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
        let Ok(names) = expand_all(entry.text().as_str(), &originals) else { return };
        let op = Op::Link { symbolic: kind.selected() == 0, new_names: Some(names) };
        crate::job_submit::submit_job(&ui, op, ui.selected_paths(), None, JobOptions::default());
        window.close();
    });
    shell.present();
}

/// Open the join dialog: the selection, in listed order, into one new file.
pub fn open_join(ui: &Rc<Ui>) {
    let names = ui.selected_names();
    if names.len() < 2 {
        crate::actions::flash(ui, "select at least two files to join");
        return;
    }
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Join Files").size(460, 300);
    shell.content().append(&gtk::Label::new(Some("Joined in this order:")));
    let list = gtk::Label::new(Some(&names.join("\n")));
    list.set_xalign(0.0);
    shell.content().append(&list);
    shell.content().append(&gtk::Label::new(Some("Name of the joined file")));
    let entry = gtk::Entry::new();
    entry.set_text(&default_join_name(&names[0]));
    entry.set_activates_default(true);
    shell.content().append(&entry);

    let ok_btn = shell.button("Join");
    shell.close_button("Cancel");
    let window = shell.window().clone();
    let ui = ui.clone();
    ok_btn.connect_clicked(move |_| {
        let target = entry.text().trim().to_string();
        if let Err(msg) = opusdm_ipc::glob::validate_new_name(&target) {
            crate::actions::flash(&ui, &msg);
            return;
        }
        let dest = dest_or_cwd(&ui);
        crate::job_submit::submit_job(&ui, Op::Join { target }, ui.selected_paths(), dest, JobOptions::default());
        window.close();
    });
    shell.present();
}

/// Open the split dialog for the single selected file.
pub fn open_split(ui: &Rc<Ui>) {
    let srcs = ui.selected_paths();
    if srcs.len() != 1 {
        crate::actions::flash(ui, "select exactly one file to split");
        return;
    }
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Split File").size(400, 200);
    shell.content().append(&gtk::Label::new(Some("Part size (e.g. 512K, 4M, 1G)")));
    let entry = gtk::Entry::new();
    entry.set_text(DEFAULT_SPLIT_SIZE);
    entry.set_activates_default(true);
    shell.content().append(&entry);
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.add_css_class("error");
    shell.content().append(&error);

    let ok_btn = shell.button("Split");
    shell.close_button("Cancel");
    let window = shell.window().clone();
    let ui = ui.clone();
    ok_btn.connect_clicked(move |_| match parse_size(entry.text().as_str()) {
        Ok(chunk_bytes) => {
            let dest = dest_or_cwd(&ui);
            crate::job_submit::submit_job(&ui, Op::Split { chunk_bytes }, srcs.clone(), dest, JobOptions::default());
            window.close();
        }
        Err(msg) => error.set_text(&msg),
    });
    shell.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes_with_binary_suffixes() {
        assert_eq!(parse_size("4096"), Ok(4096));
        assert_eq!(parse_size("512K"), Ok(512 * 1024));
        assert_eq!(parse_size("4m"), Ok(4 * 1024 * 1024));
        assert_eq!(parse_size("1GiB"), Ok(1024 * 1024 * 1024));
        assert_eq!(parse_size(" 2 KB "), Ok(2048));
    }

    #[test]
    fn rejects_bad_sizes() {
        for bad in ["", "0", "0K", "abc", "-1", "1.5M", "M", "99999999999999999999G"] {
            assert!(parse_size(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn proposes_the_joined_name() {
        assert_eq!(default_join_name("movie.mkv.001"), "movie.mkv");
        assert_eq!(default_join_name("part.01"), "part");
        assert_eq!(default_join_name("notes.txt"), "notes.txt.joined");
        assert_eq!(default_join_name(".5"), ".5.joined");
        assert_eq!(default_join_name("v1.2"), "v1.2.joined");
    }
}
