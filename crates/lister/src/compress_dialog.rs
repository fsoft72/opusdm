//! The Compress requester: archive name plus format.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_fsops::compress::CompressFormat;
use opusdm_ipc::{JobOptions, Op};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Name proposed for the archive of `srcs`: the item's own name for a single
/// item, otherwise the name of the folder holding them.
pub fn default_stem(srcs: &[PathBuf], cwd: &Path) -> String {
    let from = if let [one] = srcs { one.as_path() } else { cwd };
    from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "archive".to_string())
}

/// `name` with any known archive suffix swapped for that of `format`.
pub fn with_format(name: &str, format: CompressFormat) -> String {
    let stem = match CompressFormat::from_name(name) {
        Some(old) => &name[..name.len() - old.suffix().len()],
        None => name,
    };
    format!("{stem}{}", format.suffix())
}

/// Whether the selection is one regular file (the only case for `.gz` / `.bz2`).
fn is_single_file(srcs: &[PathBuf]) -> bool {
    matches!(srcs, [one] if std::fs::symlink_metadata(one).is_ok_and(|m| m.is_file()))
}

/// Ask for the archive name and format, then compress `srcs` into the DEST
/// Lister, or into this folder when there is no DEST.
pub fn open_compress(ui: &Rc<Ui>, srcs: Vec<PathBuf>) {
    if srcs.is_empty() {
        crate::actions::flash(ui, "nothing selected");
        return;
    }
    let formats = CompressFormat::available(is_single_file(&srcs));
    if formats.is_empty() {
        crate::actions::flash(ui, "no compressor is installed");
        return;
    }
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Compress").size(480, 170);
    let root = shell.content();
    root.append(&gtk::Label::builder().label("Archive name").xalign(0.0).build());
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    entry.set_activates_default(true);
    let labels: Vec<&str> = formats.iter().map(|f| f.suffix()).collect();
    let kind = gtk::DropDown::from_strings(&labels);
    row.append(&entry);
    row.append(&kind);
    root.append(&row);
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.add_css_class("error");
    root.append(&error);

    entry.set_text(&with_format(&default_stem(&srcs, &ui.cwd_path()), formats[0]));
    let formats = Rc::new(formats);
    {
        let entry = entry.clone();
        let formats = formats.clone();
        kind.connect_selected_notify(move |dd| {
            let Some(format) = formats.get(dd.selected() as usize) else { return };
            entry.set_text(&with_format(entry.text().as_str(), *format));
        });
    }

    let ok_btn = shell.button("Compress");
    shell.button_spacer();
    shell.close_button("Cancel");
    let window = shell.present();
    window.set_default_widget(Some(&ok_btn));
    entry.grab_focus();

    let ui = ui.clone();
    ok_btn.connect_clicked(move |_| {
        let archive = entry.text().to_string();
        if let Err(msg) = opusdm_ipc::glob::validate_new_name(&archive) {
            error.set_text(&msg);
            return;
        }
        if CompressFormat::from_name(&archive).is_none() {
            error.set_text("the name must end with the chosen extension");
            return;
        }
        let dest = crate::link_dialogs::dest_or_cwd(&ui);
        crate::job_submit::submit_job(&ui, Op::Compress { archive }, srcs.clone(), dest, JobOptions::default());
        window.close();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proposed_name_follows_the_selection() {
        let cwd = Path::new("/home/me/photos");
        assert_eq!(default_stem(&[PathBuf::from("/home/me/photos/a.txt")], cwd), "a.txt");
        assert_eq!(default_stem(&[PathBuf::from("/x/a"), PathBuf::from("/x/b")], cwd), "photos");
        assert_eq!(default_stem(&[], Path::new("/")), "archive");
    }

    #[test]
    fn changing_the_format_swaps_only_the_archive_suffix() {
        assert_eq!(with_format("docs", CompressFormat::Zip), "docs.zip");
        assert_eq!(with_format("docs.zip", CompressFormat::TarGz), "docs.tar.gz");
        assert_eq!(with_format("docs.tar.gz", CompressFormat::SevenZ), "docs.7z");
        assert_eq!(with_format("a.txt", CompressFormat::Gz), "a.txt.gz");
    }
}
