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
use crate::background::{run_streaming, CancelToken};

/// The size of a folder tree and how many entries could not be read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirSize {
    /// Bytes of the regular files that could be read.
    pub bytes: u64,
    /// Entries left out because they could not be read.
    pub skipped: u64,
}

/// Recursively calculate the total size in bytes of the files below `dir`,
/// staying on its filesystem. `None` when `cancel` was cancelled.
pub fn compute_dir_size(dir: &Path, cancel: &CancelToken) -> Option<DirSize> {
    let mut size = DirSize::default();
    for entry in walkdir::WalkDir::new(dir).min_depth(1).same_file_system(true) {
        if cancel.is_cancelled() { return None; }
        match entry.and_then(|e| e.metadata()) {
            Ok(meta) if meta.is_file() => size.bytes += meta.len(),
            Ok(_) => {}
            Err(_) => size.skipped += 1,
        }
    }
    Some(size)
}

/// One finished folder of a size scan.
struct SizeResult {
    idx: u32,
    name: String,
    path: PathBuf,
    size: DirSize,
}

/// Calculate sizes for selected directories (or all directories if none selected),
/// updating the list store rows and the status bar asynchronously. Sizes
/// calculated before are reused; `force` recalculates them (GetSizes FORCE).
/// A new calculation cancels the one still running in this Lister.
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

    let cancel = CancelToken::new();
    let previous = ui.caches.size_scan.replace(cancel.clone());
    previous.cancel();
    let totals = Rc::new(std::cell::Cell::new((total_calculated, count, 0u64)));
    let (ui_item, totals_item) = (ui.clone(), totals.clone());
    let ui_end = ui.clone();
    run_streaming(
        &cancel,
        move |out| {
            for (idx, name, path) in missing {
                let Some(size) = compute_dir_size(&path, out.cancel()) else { return };
                if !out.emit(SizeResult { idx, name, path, size }) { return; }
            }
        },
        move |done: SizeResult| {
            let (bytes, n, skipped) = totals_item.get();
            totals_item.set((bytes + done.size.bytes, n + 1, skipped + done.size.skipped));
            ui_item.caches.sizes.borrow_mut().put(done.path, done.size.bytes);
            apply_size(&ui_item, done.idx, &done.name, done.size.bytes);
        },
        move || {
            let (bytes, n, skipped) = totals.get();
            crate::actions::flash(&ui_end, &size_summary(n, bytes, skipped));
        },
    );
}

/// The message shown when a size calculation ends.
fn size_summary(count: usize, bytes: u64, skipped: u64) -> String {
    let text = format!("calculated {count} directories: total {}", format_size(false, bytes));
    if skipped == 0 { return text; }
    format!("{text} ({skipped} unreadable entries skipped)")
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

    let cancel = CancelToken::new();
    let list_box_clone = list_box.clone();
    let failed = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let hashed = Rc::new(std::cell::Cell::new(0usize));
    let (failed_item, hashed_item) = (failed.clone(), hashed.clone());
    run_streaming(
        &cancel,
        move |out| {
            for p in paths {
                let result = compute_file_hashes(&p);
                if !out.emit((p, result)) { return; }
            }
        },
        move |(path, result): (PathBuf, std::io::Result<(String, String)>)| {
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (sha, md5) = match result {
                Ok(hashes) => hashes,
                Err(e) => {
                    let text = format!("{}: {e}", path.display());
                    let label = gtk::Label::new(Some(&text));
                    label.set_xalign(0.0);
                    label.add_css_class("error-text");
                    list_box_clone.append(&label);
                    failed_item.borrow_mut().push(text);
                    return;
                }
            };
            hashed_item.set(hashed_item.get() + 1);

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
        },
        move || {
            let failed = failed.borrow();
            title_lbl.set_text(&checksum_title(hashed.get(), failed.len()));
            if !failed.is_empty() {
                opusdm_ipc::err_log!("opusdm-lister", "checksums: {}", failed.join("; "));
            }
        },
    );
    // Closing the dialog stops the hashing.
    shell.window().connect_close_request(move |_| {
        cancel.cancel();
        glib::Propagation::Proceed
    });

    shell.close_button("Close");
    shell.present();
}

/// The title of the checksum dialog once every file was read.
fn checksum_title(hashed: usize, failed: usize) -> String {
    if failed == 0 { return format!("Checksums for {hashed} file(s):"); }
    format!("Checksums for {hashed} file(s); {failed} file(s) could not be read:")
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

/// The label and CSS class of the badge that names a difference.
fn diff_badge(kind: &DiffKind) -> (&'static str, &'static str) {
    match kind {
        DiffKind::OnlyInSource => ("Only in A (New)", "success-text"),
        DiffKind::OnlyInTarget => ("Only in B", "error-text"),
        DiffKind::NewerInSource => ("Newer in A", "success-text"),
        DiffKind::NewerInTarget => ("Newer in B", "error-text"),
        DiffKind::DifferentSize => ("Different Size", "palette-cat"),
        DiffKind::Identical => ("Identical", "dim-label"),
    }
}

/// The counts shown in the summary line and the names "Select Differences" picks.
#[derive(Debug, Default, PartialEq, Eq)]
struct DiffSummary {
    only_src: usize,
    only_tgt: usize,
    modified: usize,
    identical: usize,
    /// Entries that are new or newer in the source, or differ in size.
    to_select: Vec<String>,
}

impl DiffSummary {
    /// Count the differences and collect the names to select in the Lister.
    fn of(diffs: &[DirDiffItem]) -> Self {
        let mut sum = DiffSummary::default();
        for item in diffs {
            match item.kind {
                DiffKind::OnlyInSource => sum.only_src += 1,
                DiffKind::OnlyInTarget => sum.only_tgt += 1,
                DiffKind::NewerInSource | DiffKind::NewerInTarget | DiffKind::DifferentSize => sum.modified += 1,
                DiffKind::Identical => sum.identical += 1,
            }
            if matches!(item.kind, DiffKind::OnlyInSource | DiffKind::NewerInSource | DiffKind::DifferentSize) {
                sum.to_select.push(item.name.clone());
            }
        }
        sum
    }

    /// The text of the summary line.
    fn text(&self) -> String {
        format!(
            "Results: {} in A only, {} in B only, {} modified, {} identical",
            self.only_src, self.only_tgt, self.modified, self.identical
        )
    }
}

/// One result row: the entry name and its badge.
fn build_diff_row(item: &DirDiffItem) -> gtk::ListBoxRow {
    let (badge_text, badge_css) = diff_badge(&item.kind);
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
    row
}

/// The widgets of the compare dialog the handlers need.
struct CompareWidgets {
    src_entry: gtk::Entry,
    tgt_entry: gtk::Entry,
    compare_btn: gtk::Button,
}

/// The header grid: the two path entries and the Compare button.
fn build_compare_header(src_path: &Path) -> (gtk::Grid, CompareWidgets) {
    let grid = gtk::Grid::builder().column_spacing(8).row_spacing(6).build();

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
    (grid, CompareWidgets { src_entry, tgt_entry, compare_btn })
}

/// Compare the two folders typed in the entries when Compare is clicked and
/// show the rows, the summary line and the names to select.
fn connect_compare(
    inputs: &CompareWidgets,
    summary_lbl: &gtk::Label,
    list_box: &gtk::ListBox,
    select_diff_btn: &gtk::Button,
    diff_names: &Rc<std::cell::RefCell<Vec<String>>>,
) {
    let src_entry = inputs.src_entry.clone();
    let tgt_entry = inputs.tgt_entry.clone();
    let summary_lbl = summary_lbl.clone();
    let list_box = list_box.clone();
    let select_diff_btn = select_diff_btn.clone();
    let diff_names = diff_names.clone();

    inputs.compare_btn.connect_clicked(move |_| {
        let src_dir = PathBuf::from(src_entry.text().to_string());
        let tgt_dir = PathBuf::from(tgt_entry.text().to_string());

        if !src_dir.is_dir() || !tgt_dir.is_dir() {
            summary_lbl.set_text("⚠️ Error: both paths must be valid directories");
            return;
        }

        let diffs = match compare_directories(&src_dir, &tgt_dir) {
            Ok(diffs) => diffs,
            Err(e) => {
                summary_lbl.set_text(&format!("⚠️ Error comparing: {e}"));
                return;
            }
        };
        while let Some(child) = list_box.first_child() {
            list_box.remove(&child);
        }
        for item in &diffs {
            list_box.append(&build_diff_row(item));
        }
        let summary = DiffSummary::of(&diffs);
        summary_lbl.set_text(&summary.text());
        select_diff_btn.set_sensitive(!summary.to_select.is_empty());
        *diff_names.borrow_mut() = summary.to_select;
    });
}

/// Select the names in `diff_names` in the Lister and close the dialog.
fn connect_select_in_lister(
    ui: &Rc<Ui>,
    dlg: &gtk::Window,
    select_diff_btn: &gtk::Button,
    diff_names: &Rc<std::cell::RefCell<Vec<String>>>,
) {
    let ui = ui.clone();
    let diff_names = diff_names.clone();
    let dlg = dlg.clone();
    select_diff_btn.connect_clicked(move |_| {
        let names = diff_names.borrow().clone();
        ui.view.selection.unselect_all();
        for i in 0..ui.view.selection.n_items() {
            if let Some(row) = ui.row_at(i) {
                if names.contains(&row.name) {
                    ui.view.selection.select_item(i, false);
                }
            }
        }
        dlg.close();
        crate::actions::flash(&ui, &format!("Selected {} differing file(s)", names.len()));
    });
}

/// Open the Directory Compare / Sync dialog.
pub fn open_compare_dialog(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Compare / Sync Directories").size(720, 500);
    let root = shell.content();

    let (grid, inputs) = build_compare_header(&ui.cwd_path());
    root.append(&grid);

    let summary_lbl = gtk::Label::new(Some("Enter a target directory and click Compare"));
    summary_lbl.set_xalign(0.0);
    summary_lbl.add_css_class("dim-label");
    root.append(&summary_lbl);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);
    let scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).hexpand(true).build();
    root.append(&scroller);

    let select_diff_btn = shell.button("Select Differences in Lister");
    select_diff_btn.set_sensitive(false);
    shell.button_spacer();
    shell.close_button("Close");

    let diff_names = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    connect_compare(&inputs, &summary_lbl, &list_box, &select_diff_btn, &diff_names);
    connect_select_in_lister(ui, shell.window(), &select_diff_btn, &diff_names);

    shell.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, kind: DiffKind) -> DirDiffItem {
        DirDiffItem { name: name.to_string(), kind }
    }

    #[test]
    fn the_summary_counts_each_kind_and_selects_what_the_source_adds_or_changes() {
        let diffs = [
            item("new", DiffKind::OnlyInSource),
            item("gone", DiffKind::OnlyInTarget),
            item("fresh", DiffKind::NewerInSource),
            item("stale", DiffKind::NewerInTarget),
            item("resized", DiffKind::DifferentSize),
            item("same", DiffKind::Identical),
        ];
        let sum = DiffSummary::of(&diffs);
        assert_eq!((sum.only_src, sum.only_tgt, sum.modified, sum.identical), (1, 1, 3, 1));
        assert_eq!(sum.to_select, ["new", "fresh", "resized"]);
        assert_eq!(sum.text(), "Results: 1 in A only, 1 in B only, 3 modified, 1 identical");
    }

    #[test]
    fn an_empty_comparison_selects_nothing() {
        assert_eq!(DiffSummary::of(&[]), DiffSummary::default());
    }

    #[test]
    fn test_compute_dir_size_nonexistent() {
        let size = compute_dir_size(Path::new("/nonexistent/test/path"), &CancelToken::new()).unwrap();
        assert_eq!(size.bytes, 0);
    }

    #[test]
    fn dir_size_adds_the_files_and_counts_what_it_cannot_read() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a"), [0u8; 10]).unwrap();
        std::fs::create_dir(td.path().join("sub")).unwrap();
        std::fs::write(td.path().join("sub/b"), [0u8; 5]).unwrap();
        let locked = td.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("c"), [0u8; 7]).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let size = compute_dir_size(td.path(), &CancelToken::new()).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(size.bytes, 15);
        assert_eq!(size.skipped, 1, "the locked folder is reported, not silently dropped");
    }

    #[test]
    fn a_cancelled_dir_size_gives_nothing() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a"), b"x").unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(compute_dir_size(td.path(), &cancel), None);
    }

    #[test]
    fn the_size_summary_reports_skipped_entries() {
        assert_eq!(size_summary(2, 0, 0), "calculated 2 directories: total 0 B");
        assert!(size_summary(1, 0, 3).ends_with("(3 unreadable entries skipped)"));
    }

    #[test]
    fn the_checksum_title_reports_unreadable_files() {
        assert_eq!(checksum_title(2, 0), "Checksums for 2 file(s):");
        assert_eq!(checksum_title(1, 2), "Checksums for 1 file(s); 2 file(s) could not be read:");
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
