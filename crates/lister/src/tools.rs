//! Visual directory compare, checksum calculation, and fast folder size calculation.

use crate::model::{format_size, Row};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib::{self, BoxedAnyObject};
use gtk::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Recursively calculate total size in bytes of a directory.
pub fn compute_dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(dir)
        .min_depth(1)
        .into_iter()
        .filter_map(Result::ok)
    {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

/// Calculate sizes for selected directories (or all directories if none selected),
/// updating the list store rows and the status bar asynchronously.
pub fn calculate_dir_sizes_async(ui: &Rc<Ui>) {
    let cwd = ui.cwd_path();
    let mut target_dirs = Vec::new();

    let sel_names = ui.selected_names();
    let n_items = ui.store.n_items();

    for i in 0..n_items {
        if let Some(item) = ui.store.item(i) {
            if let Some(boxed) = item.downcast_ref::<BoxedAnyObject>() {
                let row = boxed.borrow::<Rc<Row>>().clone();
                if row.is_dir {
                    let should_calc = if sel_names.is_empty() {
                        true
                    } else {
                        sel_names.contains(&row.name)
                    };
                    if should_calc {
                        target_dirs.push((i, row.name.clone(), cwd.join(&row.name)));
                    }
                }
            }
        }
    }

    if target_dirs.is_empty() {
        crate::actions::flash(ui, "no directories to calculate size for");
        return;
    }

    crate::actions::flash(
        ui,
        &format!("calculating sizes for {} director(ies)...", target_dirs.len()),
    );

    let ui_clone = ui.clone();
    let (tx, rx) = async_channel::unbounded::<(u32, String, u64)>();

    std::thread::spawn(move || {
        for (idx, name, path) in target_dirs {
            let size = compute_dir_size(&path);
            let _ = tx.send_blocking((idx, name, size));
        }
    });

    glib::spawn_future_local(async move {
        let mut total_calculated = 0u64;
        let mut count = 0;
        while let Ok((_idx, name, size)) = rx.recv().await {
            total_calculated += size;
            count += 1;

            // Find matching row in store by name
            for i in 0..ui_clone.store.n_items() {
                if let Some(item) = ui_clone.store.item(i) {
                    if let Some(boxed) = item.downcast_ref::<BoxedAnyObject>() {
                        let row = boxed.borrow::<Rc<Row>>().clone();
                        if row.name == name && row.is_dir {
                            let mut updated = (*row).clone();
                            updated.size = size;
                            let new_item = BoxedAnyObject::new(Rc::new(updated));
                            ui_clone.store.splice(i, 1, &[new_item]);
                            break;
                        }
                    }
                }
            }
        }
        crate::actions::flash(
            &ui_clone,
            &format!(
                "calculated {} directories: total {}",
                count,
                format_size(false, total_calculated)
            ),
        );
    });
}

/// Compute SHA-256 and MD5 hashes of a file.
pub fn compute_file_hashes(path: &Path) -> std::io::Result<(String, String)> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0u8; 65536];

    let sha256_output = std::process::Command::new("sha256sum")
        .arg(path)
        .output();
    let md5_output = std::process::Command::new("md5sum")
        .arg(path)
        .output();

    let sha256_str = if let Ok(out) = sha256_output {
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()
    } else {
        // Fallback: read bytes
        let _ = file.read(&mut buffer);
        "unavailable".into()
    };

    let md5_str = if let Ok(out) = md5_output {
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()
    } else {
        "unavailable".into()
    };

    Ok((sha256_str, md5_str))
}

/// Open Checksums HUD dialog for the selected file(s).
pub fn open_checksum_dialog(ui: &Rc<Ui>) {
    let paths: Vec<PathBuf> = ui
        .selected_paths()
        .into_iter()
        .filter(|p| p.is_file())
        .collect();

    if paths.is_empty() {
        crate::actions::flash(ui, "select at least one file to calculate checksums");
        return;
    }

    let dialog = gtk::Window::builder()
        .title("File Checksums (SHA-256 & MD5)")
        .transient_for(&ui.window)
        .modal(true)
        .default_width(620)
        .default_height(360)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(14);
    root.set_margin_end(14);
    root.set_margin_top(14);
    root.set_margin_bottom(14);

    let title_lbl = gtk::Label::new(Some(&format!(
        "Calculating hashes for {} file(s)...",
        paths.len()
    )));
    title_lbl.set_xalign(0.0);
    title_lbl.add_css_class("preview-title");
    root.append(&title_lbl);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);

    let scroller = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .vexpand(true)
        .hexpand(true)
        .build();
    root.append(&scroller);

    let (tx, rx) = async_channel::unbounded::<(PathBuf, String, String)>();
    let paths_clone = paths.clone();

    std::thread::spawn(move || {
        for p in paths_clone {
            if let Ok((sha, md5)) = compute_file_hashes(&p) {
                let _ = tx.send_blocking((p, sha, md5));
            }
        }
    });

    let list_box_clone = list_box.clone();
    let title_lbl_clone = title_lbl.clone();

    glib::spawn_future_local(async move {
        let mut count = 0;
        while let Ok((path, sha, md5)) = rx.recv().await {
            count += 1;
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            let row = gtk::ListBoxRow::new();
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
            vbox.set_margin_start(8);
            vbox.set_margin_end(8);
            vbox.set_margin_top(6);
            vbox.set_margin_bottom(6);

            let fn_label = gtk::Label::new(Some(&filename));
            fn_label.set_xalign(0.0);
            fn_label.add_css_class("palette-title");
            vbox.append(&fn_label);

            // SHA-256 row
            let sha_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let sha_tag = gtk::Label::new(Some("SHA-256:"));
            sha_tag.add_css_class("dim-label");
            let sha_val = gtk::Label::new(Some(&sha));
            sha_val.set_xalign(0.0);
            sha_val.set_hexpand(true);
            sha_val.set_selectable(true);
            sha_val.add_css_class("terminal-title");

            let copy_sha_btn = gtk::Button::with_label("Copy");
            copy_sha_btn.add_css_class("flat");
            {
                let sha_to_copy = sha.clone();
                copy_sha_btn.connect_clicked(move |btn| {
                    if let Some(display) = gtk::gdk::Display::default() {
                        display.clipboard().set_text(&sha_to_copy);
                        btn.set_label("Copied!");
                    }
                });
            }

            sha_box.append(&sha_tag);
            sha_box.append(&sha_val);
            sha_box.append(&copy_sha_btn);
            vbox.append(&sha_box);

            // MD5 row
            let md5_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let md5_tag = gtk::Label::new(Some("MD5:    "));
            md5_tag.add_css_class("dim-label");
            let md5_val = gtk::Label::new(Some(&md5));
            md5_val.set_xalign(0.0);
            md5_val.set_hexpand(true);
            md5_val.set_selectable(true);
            md5_val.add_css_class("terminal-title");

            let copy_md5_btn = gtk::Button::with_label("Copy");
            copy_md5_btn.add_css_class("flat");
            {
                let md5_to_copy = md5.clone();
                copy_md5_btn.connect_clicked(move |btn| {
                    if let Some(display) = gtk::gdk::Display::default() {
                        display.clipboard().set_text(&md5_to_copy);
                        btn.set_label("Copied!");
                    }
                });
            }

            md5_box.append(&md5_tag);
            md5_box.append(&md5_val);
            md5_box.append(&copy_md5_btn);
            vbox.append(&md5_box);

            row.set_child(Some(&vbox));
            list_box_clone.append(&row);
        }
        title_lbl_clone.set_text(&format!("Checksums for {count} file(s):"));
    });

    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk::Align::End);
    let close_btn = gtk::Button::with_label("Close");
    {
        let dlg = dialog.clone();
        close_btn.connect_clicked(move |_| dlg.close());
    }
    btn_box.append(&close_btn);
    root.append(&btn_box);

    dialog.set_child(Some(&root));
    dialog.present();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKind {
    OnlyInSource,
    OnlyInTarget,
    NewerInSource,
    NewerInTarget,
    Identical,
    DifferentSize,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DirDiffItem {
    pub name: String,
    pub kind: DiffKind,
    pub src_size: u64,
    pub tgt_size: u64,
}

/// Compare two directories and compute entry-level diffs.
pub fn compare_directories(src: &Path, tgt: &Path) -> std::io::Result<Vec<DirDiffItem>> {
    let src_rows = crate::model::read_dir_rows(src)?;
    let tgt_rows = crate::model::read_dir_rows(tgt)?;

    let mut src_map: HashMap<String, Row> = HashMap::new();
    for r in src_rows {
        src_map.insert(r.name.clone(), r);
    }

    let mut tgt_map: HashMap<String, Row> = HashMap::new();
    for r in tgt_rows {
        tgt_map.insert(r.name.clone(), r);
    }

    let all_names: HashSet<String> = src_map
        .keys()
        .chain(tgt_map.keys())
        .cloned()
        .collect();

    let mut diffs = Vec::new();

    for name in all_names {
        match (src_map.get(&name), tgt_map.get(&name)) {
            (Some(s), None) => {
                diffs.push(DirDiffItem {
                    name,
                    kind: DiffKind::OnlyInSource,
                    src_size: s.size,
                    tgt_size: 0,
                });
            }
            (None, Some(t)) => {
                diffs.push(DirDiffItem {
                    name,
                    kind: DiffKind::OnlyInTarget,
                    src_size: 0,
                    tgt_size: t.size,
                });
            }
            (Some(s), Some(t)) => {
                let kind = if s.size == t.size && s.modified_unix == t.modified_unix {
                    DiffKind::Identical
                } else if s.modified_unix > t.modified_unix {
                    DiffKind::NewerInSource
                } else if t.modified_unix > s.modified_unix {
                    DiffKind::NewerInTarget
                } else {
                    DiffKind::DifferentSize
                };
                diffs.push(DirDiffItem {
                    name,
                    kind,
                    src_size: s.size,
                    tgt_size: t.size,
                });
            }
            (None, None) => {}
        }
    }

    diffs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(diffs)
}

/// Open the Directory Compare / Sync dialog.
pub fn open_compare_dialog(ui: &Rc<Ui>) {
    let src_path = ui.cwd_path();

    let dialog = gtk::Window::builder()
        .title("Compare / Sync Directories")
        .transient_for(&ui.window)
        .modal(true)
        .default_width(720)
        .default_height(500)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(14);
    root.set_margin_end(14);
    root.set_margin_top(14);
    root.set_margin_bottom(14);

    // Header inputs
    let grid = gtk::Grid::builder()
        .column_spacing(8)
        .row_spacing(6)
        .build();

    let src_lbl = gtk::Label::new(Some("Source (A):"));
    src_lbl.set_xalign(1.0);
    let src_entry = gtk::Entry::new();
    src_entry.set_text(&src_path.to_string_lossy());
    src_entry.set_hexpand(true);

    let tgt_lbl = gtk::Label::new(Some("Target (B):"));
    tgt_lbl.set_xalign(1.0);
    let tgt_entry = gtk::Entry::new();
    tgt_entry.set_hexpand(true);
    tgt_entry.set_placeholder_text(Some("Path to comparison directory"));

    let compare_btn = gtk::Button::with_label("Compare");
    compare_btn.add_css_class("suggested-action");

    grid.attach(&src_lbl, 0, 0, 1, 1);
    grid.attach(&src_entry, 1, 0, 1, 1);

    grid.attach(&tgt_lbl, 0, 1, 1, 1);
    grid.attach(&tgt_entry, 1, 1, 1, 1);
    grid.attach(&compare_btn, 2, 1, 1, 1);

    root.append(&grid);

    // Summary banner
    let summary_lbl = gtk::Label::new(Some("Enter a target directory and click Compare"));
    summary_lbl.set_xalign(0.0);
    summary_lbl.add_css_class("dim-label");
    root.append(&summary_lbl);

    // Results table
    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);

    let scroller = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .vexpand(true)
        .hexpand(true)
        .build();
    root.append(&scroller);

    // Footer actions
    let footer_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let select_diff_btn = gtk::Button::with_label("Select Differences in Lister");
    select_diff_btn.set_sensitive(false);

    let close_btn = gtk::Button::with_label("Close");
    {
        let dlg = dialog.clone();
        close_btn.connect_clicked(move |_| dlg.close());
    }

    footer_box.append(&select_diff_btn);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer_box.append(&spacer);
    footer_box.append(&close_btn);
    root.append(&footer_box);

    let diff_names_to_select = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));

    // Compare logic
    {
        let src_entry = src_entry.clone();
        let tgt_entry = tgt_entry.clone();
        let summary_lbl = summary_lbl.clone();
        let list_box = list_box.clone();
        let select_diff_btn = select_diff_btn.clone();
        let diff_names = diff_names_to_select.clone();

        compare_btn.connect_clicked(move |_| {
            let src_dir = PathBuf::from(src_entry.text().to_string());
            let tgt_dir = PathBuf::from(tgt_entry.text().to_string());

            if !src_dir.is_dir() || !tgt_dir.is_dir() {
                summary_lbl.set_text("⚠️ Error: both paths must be valid directories");
                return;
            }

            match compare_directories(&src_dir, &tgt_dir) {
                Ok(diffs) => {
                    while let Some(child) = list_box.first_child() {
                        list_box.remove(&child);
                    }

                    let mut only_src = 0;
                    let mut only_tgt = 0;
                    let mut modified = 0;
                    let mut identical = 0;
                    let mut to_sel = Vec::new();

                    for item in &diffs {
                        let (badge_text, badge_css) = match item.kind {
                            DiffKind::OnlyInSource => {
                                only_src += 1;
                                to_sel.push(item.name.clone());
                                ("Only in A (New)", "success-text")
                            }
                            DiffKind::OnlyInTarget => {
                                only_tgt += 1;
                                ("Only in B", "error-text")
                            }
                            DiffKind::NewerInSource => {
                                modified += 1;
                                to_sel.push(item.name.clone());
                                ("Newer in A", "success-text")
                            }
                            DiffKind::NewerInTarget => {
                                modified += 1;
                                ("Newer in B", "error-text")
                            }
                            DiffKind::DifferentSize => {
                                modified += 1;
                                to_sel.push(item.name.clone());
                                ("Different Size", "palette-cat")
                            }
                            DiffKind::Identical => {
                                identical += 1;
                                ("Identical", "dim-label")
                            }
                        };

                        let row = gtk::ListBoxRow::new();
                        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                        hbox.set_margin_start(8);
                        hbox.set_margin_end(8);
                        hbox.set_margin_top(4);
                        hbox.set_margin_bottom(4);

                        let name_lbl = gtk::Label::new(Some(&item.name));
                        name_lbl.set_xalign(0.0);
                        name_lbl.set_hexpand(true);

                        let badge = gtk::Label::new(Some(badge_text));
                        badge.add_css_class(badge_css);

                        hbox.append(&name_lbl);
                        hbox.append(&badge);
                        row.set_child(Some(&hbox));
                        list_box.append(&row);
                    }

                    summary_lbl.set_text(&format!(
                        "Results: {} in A only, {} in B only, {} modified, {} identical",
                        only_src, only_tgt, modified, identical
                    ));
                    *diff_names.borrow_mut() = to_sel;
                    select_diff_btn.set_sensitive(!diff_names.borrow().is_empty());
                }
                Err(e) => {
                    summary_lbl.set_text(&format!("⚠️ Error comparing: {e}"));
                }
            }
        });
    }

    // Select in Lister action
    {
        let ui_clone = ui.clone();
        let diff_names = diff_names_to_select.clone();
        let dlg = dialog.clone();
        select_diff_btn.connect_clicked(move |_| {
            let names = diff_names.borrow().clone();
            ui_clone.selection.unselect_all();
            for i in 0..ui_clone.selection.n_items() {
                if let Some(row) = ui_clone.row_at(i) {
                    if names.contains(&row.name) {
                        ui_clone.selection.select_item(i, false);
                    }
                }
            }
            dlg.close();
            crate::actions::flash(
                &ui_clone,
                &format!("Selected {} differing file(s)", names.len()),
            );
        });
    }

    dialog.set_child(Some(&root));
    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_dir_size_nonexistent() {
        let size = compute_dir_size(Path::new("/nonexistent/test/path"));
        assert_eq!(size, 0);
    }
}
