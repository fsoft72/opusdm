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
/// updating the list store rows and the status bar asynchronously. Sizes
/// calculated before are reused; `force` recalculates them (GetSizes FORCE).
pub fn calculate_dir_sizes_async(ui: &Rc<Ui>, force: bool) {
    let cwd = ui.cwd_path();
    let mut target_dirs = Vec::new();

    let sel_names = ui.selected_names();
    let n_items = ui.view.store.n_items();

    for i in 0..n_items {
        if let Some(item) = ui.view.store.item(i) {
            if let Some(boxed) = item.downcast_ref::<BoxedAnyObject>() {
                let row = boxed.borrow::<Rc<Row>>();
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

    let (known, missing) = ui.caches.sizes.borrow_mut().split(target_dirs, force);
    let mut total_calculated = 0u64;
    let mut count = 0;
    for (idx, name, size) in known {
        apply_size(ui, idx, &name, size);
        total_calculated += size;
        count += 1;
    }
    if missing.is_empty() {
        crate::actions::flash(
            ui,
            &format!("{count} directories already calculated: total {} (use the force variant to recalculate)", format_size(false, total_calculated)),
        );
        return;
    }

    crate::actions::flash(ui, &format!("calculating sizes for {} director(ies)...", missing.len()));

    let ui_clone = ui.clone();
    let (tx, rx) = async_channel::unbounded::<(u32, String, PathBuf, u64)>();

    std::thread::spawn(move || {
        for (idx, name, path) in missing {
            let size = compute_dir_size(&path);
            let _ = tx.send_blocking((idx, name, path, size));
        }
    });

    glib::spawn_future_local(async move {
        while let Ok((idx, name, path, size)) = rx.recv().await {
            total_calculated += size;
            count += 1;
            ui_clone.caches.sizes.borrow_mut().put(path, size);
            apply_size(&ui_clone, idx, &name, size);
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

/// Show `size` on the directory row `name` at store index `idx`; the name is
/// re-checked in case the store changed underneath.
fn apply_size(ui: &Rc<Ui>, idx: u32, name: &str, size: u64) {
    let Some(item) = ui.view.store.item(idx) else { return };
    let Some(boxed) = item.downcast_ref::<BoxedAnyObject>() else { return };
    let row = boxed.borrow::<Rc<Row>>();
    if row.name == name && row.is_dir {
        let mut updated = (**row).clone();
        updated.size = size;
        let new_item = BoxedAnyObject::new(Rc::new(updated));
        drop(row);
        ui.view.store.splice(idx, 1, &[new_item]);
    }
}

/// Compute SHA-256 and MD5 hashes of a file by streaming it through both
/// digests in one pass, in-process (no `sha256sum`/`md5sum` subprocesses).
pub fn compute_file_hashes(path: &Path) -> std::io::Result<(String, String)> {
    use md5::{Digest, Md5};
    use sha2::Sha256;

    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0u8; 65536];
    let mut sha256 = Sha256::new();
    let mut md5 = Md5::new();

    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        sha256.update(&buffer[..n]);
        md5.update(&buffer[..n]);
    }

    Ok((format!("{:x}", sha256.finalize()), format!("{:x}", md5.finalize())))
}

/// One labeled hash value with a copy-to-clipboard button, as used for both
/// the SHA-256 and MD5 rows in the checksum dialog.
fn hash_row(label: &str, value: &str) -> gtk::Box {
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let tag = gtk::Label::new(Some(label));
    tag.add_css_class("dim-label");
    let val = gtk::Label::new(Some(value));
    val.set_xalign(0.0);
    val.set_hexpand(true);
    val.set_selectable(true);
    val.add_css_class("terminal-title");

    let copy_btn = gtk::Button::with_label("Copy");
    copy_btn.add_css_class("flat");
    let to_copy = value.to_string();
    copy_btn.connect_clicked(move |btn| {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&to_copy);
            btn.set_label("Copied!");
        }
    });

    hbox.append(&tag);
    hbox.append(&val);
    hbox.append(&copy_btn);
    hbox
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

    let shell = opusdm_ui::DialogShell::new(&ui.window, "File Checksums (SHA-256 & MD5)")
        .size(620, 360);
    let root = shell.content();

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

            vbox.append(&hash_row("SHA-256:", &sha));
            vbox.append(&hash_row("MD5:    ", &md5));

            row.set_child(Some(&vbox));
            list_box_clone.append(&row);
        }
        title_lbl_clone.set_text(&format!("Checksums for {count} file(s):"));
    });

    shell.close_button("Close");
    shell.present();
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
pub struct DirDiffItem {
    pub name: String,
    pub kind: DiffKind,
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
            (Some(_), None) => {
                diffs.push(DirDiffItem {
                    name,
                    kind: DiffKind::OnlyInSource,
                });
            }
            (None, Some(_)) => {
                diffs.push(DirDiffItem {
                    name,
                    kind: DiffKind::OnlyInTarget,
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
                });
            }
            (None, None) => {}
        }
    }

    diffs.sort_by_key(|a| a.name.to_lowercase());
    Ok(diffs)
}

/// Open the Directory Compare / Sync dialog.
pub fn open_compare_dialog(ui: &Rc<Ui>) {
    let src_path = ui.cwd_path();

    let shell = opusdm_ui::DialogShell::new(&ui.window, "Compare / Sync Directories")
        .size(720, 500);
    let root = shell.content();

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
    let select_diff_btn = shell.button("Select Differences in Lister");
    select_diff_btn.set_sensitive(false);
    shell.button_spacer();
    shell.close_button("Close");

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
        let dlg = shell.window().clone();
        select_diff_btn.connect_clicked(move |_| {
            let names = diff_names.borrow().clone();
            ui_clone.view.selection.unselect_all();
            for i in 0..ui_clone.view.selection.n_items() {
                if let Some(row) = ui_clone.row_at(i) {
                    if names.contains(&row.name) {
                        ui_clone.view.selection.select_item(i, false);
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

    shell.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_dir_size_nonexistent() {
        let size = compute_dir_size(Path::new("/nonexistent/test/path"));
        assert_eq!(size, 0);
    }

    #[test]
    fn compute_file_hashes_matches_known_digests() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hello.txt");
        std::fs::write(&path, b"hello world").unwrap();
        let (sha256, md5) = compute_file_hashes(&path).unwrap();
        assert_eq!(sha256, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
        assert_eq!(md5, "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }
}
