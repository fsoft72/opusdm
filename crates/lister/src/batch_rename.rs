//! Advanced Batch Renamer with live preview, regex and numbering.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseTransform {
    None,
    Lowercase,
    Uppercase,
    TitleCase,
    SnakeCase,
    KebabCase,
}

#[derive(Debug, Clone)]
pub struct RenameRules {
    pub find: String,
    pub replace: String,
    pub use_regex: bool,
    pub prefix: String,
    pub suffix: String,
    pub numbering: bool,
    pub number_start: usize,
    pub number_step: usize,
    pub number_padding: usize,
    pub case: CaseTransform,
    pub preserve_ext: bool,
}

impl Default for RenameRules {
    fn default() -> Self {
        Self {
            find: String::new(),
            replace: String::new(),
            use_regex: false,
            prefix: String::new(),
            suffix: String::new(),
            numbering: false,
            number_start: 1,
            number_step: 1,
            number_padding: 3,
            case: CaseTransform::None,
            preserve_ext: true,
        }
    }
}

/// Compute new names for a list of original filenames given renaming rules.
pub fn compute_batch_rename(names: &[String], rules: &RenameRules) -> Vec<(String, String)> {
    let mut results = Vec::with_capacity(names.len());

    for (i, orig) in names.iter().enumerate() {
        let (base, ext) = if rules.preserve_ext {
            if let Some((b, e)) = orig.rsplit_once('.') {
                if !b.is_empty() {
                    (b.to_string(), format!(".{e}"))
                } else {
                    (orig.clone(), String::new())
                }
            } else {
                (orig.clone(), String::new())
            }
        } else {
            (orig.clone(), String::new())
        };

        let mut transformed = base;

        // 1. Find & Replace
        if !rules.find.is_empty() {
            if rules.use_regex {
                if let Ok(Some(re)) = glib::Regex::new(
                    &rules.find,
                    glib::RegexCompileFlags::DEFAULT,
                    glib::RegexMatchFlags::DEFAULT,
                ) {
                    if let Ok(rep) = re.replace(
                        &transformed,
                        0,
                        &rules.replace,
                        glib::RegexMatchFlags::DEFAULT,
                    ) {
                        transformed = rep.to_string();
                    }
                }
            } else {
                transformed = transformed.replace(&rules.find, &rules.replace);
            }
        }

        // 2. Case transformation
        transformed = match rules.case {
            CaseTransform::None => transformed,
            CaseTransform::Lowercase => transformed.to_lowercase(),
            CaseTransform::Uppercase => transformed.to_uppercase(),
            CaseTransform::TitleCase => to_title_case(&transformed),
            CaseTransform::SnakeCase => to_snake_case(&transformed),
            CaseTransform::KebabCase => to_kebab_case(&transformed),
        };

        // 3. Prefix & Suffix
        if !rules.prefix.is_empty() {
            transformed = format!("{}{}", rules.prefix, transformed);
        }
        if !rules.suffix.is_empty() {
            transformed = format!("{}{}", transformed, rules.suffix);
        }

        // 4. Numbering
        if rules.numbering {
            let num = rules.number_start + i * rules.number_step;
            let num_str = format!("{:0width$}", num, width = rules.number_padding);
            transformed = format!("{transformed}_{num_str}");
        }

        let final_name = format!("{transformed}{ext}");
        results.push((orig.clone(), final_name));
    }

    results
}

fn to_title_case(s: &str) -> String {
    let mut res = String::new();
    let mut capitalize_next = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if capitalize_next {
                for upper in c.to_uppercase() {
                    res.push(upper);
                }
                capitalize_next = false;
            } else {
                for lower in c.to_lowercase() {
                    res.push(lower);
                }
            }
        } else {
            res.push(c);
            capitalize_next = true;
        }
    }
    res
}

fn to_snake_case(s: &str) -> String {
    let mut res = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_whitespace() || c == '-' {
            res.push('_');
        } else if c.is_uppercase() {
            if i > 0 && !res.ends_with('_') {
                res.push('_');
            }
            for lower in c.to_lowercase() {
                res.push(lower);
            }
        } else {
            res.push(c);
        }
    }
    res
}

fn to_kebab_case(s: &str) -> String {
    let mut res = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_whitespace() || c == '_' {
            res.push('-');
        } else if c.is_uppercase() {
            if i > 0 && !res.ends_with('-') {
                res.push('-');
            }
            for lower in c.to_lowercase() {
                res.push(lower);
            }
        } else {
            res.push(c);
        }
    }
    res
}

/// What the dialog's apply button does with the computed `(old, new)` names.
pub type ApplyMappings = Rc<dyn Fn(&Rc<Ui>, Vec<(String, String)>)>;

/// Open the Batch Rename dialog for the currently selected files.
pub fn open_batch_rename_dialog(ui: &Rc<Ui>) {
    if ui.selected_names().is_empty() {
        crate::actions::flash(ui, "no files selected for batch rename");
        return;
    }
    open_rules_dialog(ui, "Batch Rename", "Apply Rename", Rc::new(rename_in_place));
}

/// Rename every changed entry inside the current directory. The renames
/// run on a worker thread; a failure shows every error in a requester.
fn rename_in_place(ui: &Rc<Ui>, mappings: Vec<(String, String)>) {
    let dir = ui.cwd_path();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gtk::gio::spawn_blocking(move || rename_all(&dir, &mappings)).await;
        ui.reload();
        let Ok((renamed_count, errors)) = result else {
            return crate::actions::flash(&ui, "batch rename failed: the worker thread panicked");
        };
        let (headline, detail) = rename_summary(renamed_count, &errors);
        let Some(detail) = detail else { return crate::actions::flash(&ui, &headline) };
        opusdm_ipc::err_log!("opusdm-lister", "batch rename: {headline}\n{detail}");
        opusdm_ui::requester(&ui.window, "Batch Rename", &headline, Some(&detail), &[("OK", ())], |_| {});
    });
}

/// The result of a batch rename for the user: a headline, plus every error
/// on its own line when there were any.
fn rename_summary(renamed: usize, errors: &[String]) -> (String, Option<String>) {
    if errors.is_empty() { return (format!("Successfully renamed {renamed} item(s)"), None); }
    let headline = format!("Renamed {renamed} item(s) with {} error(s)", errors.len());
    (headline, Some(errors.join("\n")))
}

/// Apply `(old, new)` renames inside `dir` without ever replacing a file.
///
/// Every changed entry is first moved to a temporary name, then to its final
/// name, so chains such as `a -> b, b -> c` do not depend on the order. An
/// entry whose final name is taken (by a file outside the batch, or by another
/// entry of it) is put back under its old name and reported. Returns the
/// number of renamed entries and one message per failure.
fn rename_all(dir: &Path, mappings: &[(String, String)]) -> (usize, Vec<String>) {
    let mut errors = Vec::new();
    let mut staged = Vec::new();
    for (i, (orig, new)) in mappings.iter().enumerate() {
        if orig == new {
            continue;
        }
        let tmp = dir.join(format!(".opusdm-rename-{}-{i}", std::process::id()));
        match opusdm_fsops::rename_noreplace(&dir.join(orig), &tmp) {
            Ok(()) => staged.push((orig, new, tmp)),
            Err(e) => errors.push(format!("{orig} -> {new}: {e}")),
        }
    }
    let mut renamed = 0;
    for (orig, new, tmp) in staged {
        match opusdm_fsops::rename_noreplace(&tmp, &dir.join(new)) {
            Ok(()) => renamed += 1,
            Err(e) => {
                errors.push(format!("{orig} -> {new}: {e}"));
                if let Err(e) = opusdm_fsops::rename_noreplace(&tmp, &dir.join(orig)) {
                    errors.push(format!("{orig}: cannot restore, left as {}: {e}", tmp.display()));
                }
            }
        }
    }
    (renamed, errors)
}

/// The case transformation the "Case" drop-down item `index` stands for.
fn case_from_index(index: u32) -> CaseTransform {
    match index {
        1 => CaseTransform::Lowercase,
        2 => CaseTransform::Uppercase,
        3 => CaseTransform::TitleCase,
        4 => CaseTransform::SnakeCase,
        5 => CaseTransform::KebabCase,
        _ => CaseTransform::None,
    }
}

/// The inputs of the rules dialog.
#[derive(Clone)]
struct RulesControls {
    find_entry: gtk::Entry,
    rep_entry: gtk::Entry,
    regex_check: gtk::CheckButton,
    pre_entry: gtk::Entry,
    suf_entry: gtk::Entry,
    ext_check: gtk::CheckButton,
    case_combo: gtk::DropDown,
    num_check: gtk::CheckButton,
}

impl RulesControls {
    /// Build the controls and the grid that lays them out.
    fn build() -> (gtk::Grid, Self) {
        let grid = gtk::Grid::builder().column_spacing(8).row_spacing(6).build();

        // Find & Replace
        let find_label = gtk::Label::new(Some("Find:"));
        find_label.set_xalign(1.0);
        let find_entry = gtk::Entry::new();
        find_entry.set_hexpand(true);
        find_entry.set_placeholder_text(Some("Text or regex pattern to match"));

        let rep_label = gtk::Label::new(Some("Replace:"));
        rep_label.set_xalign(1.0);
        let rep_entry = gtk::Entry::new();
        rep_entry.set_hexpand(true);
        rep_entry.set_placeholder_text(Some("Replacement string (supports $1, $2 with regex)"));

        let regex_check = gtk::CheckButton::with_label("Use Regex");

        grid.attach(&find_label, 0, 0, 1, 1);
        grid.attach(&find_entry, 1, 0, 1, 1);
        grid.attach(&regex_check, 2, 0, 1, 1);

        grid.attach(&rep_label, 0, 1, 1, 1);
        grid.attach(&rep_entry, 1, 1, 2, 1);

        // Prefix & Suffix
        let pre_label = gtk::Label::new(Some("Prefix:"));
        pre_label.set_xalign(1.0);
        let pre_entry = gtk::Entry::new();
        pre_entry.set_placeholder_text(Some("Prefix"));

        let suf_label = gtk::Label::new(Some("Suffix:"));
        suf_label.set_xalign(1.0);
        let suf_entry = gtk::Entry::new();
        suf_entry.set_placeholder_text(Some("Suffix"));

        let ext_check = gtk::CheckButton::with_label("Preserve Extension");
        ext_check.set_active(true);

        grid.attach(&pre_label, 0, 2, 1, 1);
        grid.attach(&pre_entry, 1, 2, 1, 1);
        grid.attach(&ext_check, 2, 2, 1, 1);

        grid.attach(&suf_label, 0, 3, 1, 1);
        grid.attach(&suf_entry, 1, 3, 1, 1);

        // Case Transformation
        let case_label = gtk::Label::new(Some("Case:"));
        case_label.set_xalign(1.0);
        let case_combo = gtk::DropDown::from_strings(&[
            "Keep Original",
            "lowercase",
            "UPPERCASE",
            "Title Case",
            "snake_case",
            "kebab-case",
        ]);
        grid.attach(&case_label, 0, 4, 1, 1);
        grid.attach(&case_combo, 1, 4, 1, 1);

        // Numbering
        let num_check = gtk::CheckButton::with_label("Add Sequence Number (_001)");
        grid.attach(&num_check, 1, 5, 2, 1);

        (grid, Self { find_entry, rep_entry, regex_check, pre_entry, suf_entry, ext_check, case_combo, num_check })
    }

    /// The rules the inputs describe right now.
    fn rules(&self) -> RenameRules {
        RenameRules {
            find: self.find_entry.text().to_string(),
            replace: self.rep_entry.text().to_string(),
            use_regex: self.regex_check.is_active(),
            prefix: self.pre_entry.text().to_string(),
            suffix: self.suf_entry.text().to_string(),
            numbering: self.num_check.is_active(),
            number_start: 1,
            number_step: 1,
            number_padding: 3,
            case: case_from_index(self.case_combo.selected()),
            preserve_ext: self.ext_check.is_active(),
        }
    }

    /// Call `on_change` whenever any input changes.
    fn connect_changed(&self, on_change: Rc<dyn Fn()>) {
        let up = on_change.clone();
        self.find_entry.connect_changed(move |_| up());
        let up = on_change.clone();
        self.rep_entry.connect_changed(move |_| up());
        let up = on_change.clone();
        self.regex_check.connect_toggled(move |_| up());
        let up = on_change.clone();
        self.pre_entry.connect_changed(move |_| up());
        let up = on_change.clone();
        self.suf_entry.connect_changed(move |_| up());
        let up = on_change.clone();
        self.ext_check.connect_toggled(move |_| up());
        let up = on_change.clone();
        self.num_check.connect_toggled(move |_| up());
        self.case_combo.connect_selected_notify(move |_| on_change());
    }
}

/// One preview row: the old name, an arrow and the new name, coloured by
/// whether it collides with an earlier one or changes.
fn build_preview_row(orig: &str, new: &str, collides: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);
    hbox.set_margin_top(4);
    hbox.set_margin_bottom(4);

    let orig_lbl = gtk::Label::new(Some(orig));
    orig_lbl.set_xalign(0.0);
    orig_lbl.set_hexpand(true);
    orig_lbl.add_css_class("dim-label");

    let arrow_lbl = gtk::Label::new(Some("➜"));

    let new_lbl = gtk::Label::new(Some(new));
    new_lbl.set_xalign(0.0);
    new_lbl.set_hexpand(true);

    if collides {
        new_lbl.add_css_class("error-text");
    } else if orig != new {
        new_lbl.add_css_class("success-text");
    }

    hbox.append(&orig_lbl);
    hbox.append(&arrow_lbl);
    hbox.append(&new_lbl);
    row.set_child(Some(&hbox));
    row
}

/// Replace the preview rows with `mappings` and show a warning when two of
/// them end up with the same new name.
fn render_preview(list_box: &gtk::ListBox, warning_label: &gtk::Label, mappings: &[(String, String)]) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let mut seen_new_names = HashSet::new();
    let mut has_conflict = false;
    for (orig, new) in mappings {
        let collides = !seen_new_names.insert(new.clone());
        has_conflict |= collides;
        list_box.append(&build_preview_row(orig, new, collides));
    }
    if has_conflict {
        warning_label.set_text("⚠️ Warning: Multiple items have identical new names (collision)!");
    } else {
        warning_label.set_text("");
    }
}

/// The rules dialog (find/replace, prefix, suffix, case, numbering) with a
/// live preview over the selected names. `on_apply` receives the final
/// `(old, new)` pairs when the apply button is pressed.
pub fn open_rules_dialog(ui: &Rc<Ui>, title: &str, apply_label: &str, on_apply: ApplyMappings) {
    let names = ui.selected_names();
    if names.is_empty() {
        return;
    }

    let shell = opusdm_ui::DialogShell::new(&ui.window, &format!("{title} - {} items", names.len())).size(680, 520);
    let root = shell.content();

    let (grid, controls) = RulesControls::build();
    root.append(&grid);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let preview_header = gtk::Label::new(Some("Live Preview:"));
    preview_header.set_xalign(0.0);
    preview_header.add_css_class("dim-label");
    root.append(&preview_header);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);
    let scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).hexpand(true).build();
    root.append(&scroller);

    let warning_label = gtk::Label::new(None);
    warning_label.set_xalign(0.0);
    warning_label.add_css_class("error-text");
    root.append(&warning_label);

    shell.close_button("Cancel");
    let apply_btn = shell.button(apply_label);
    apply_btn.add_css_class("suggested-action");

    let current_mappings = Rc::new(RefCell::new(Vec::<(String, String)>::new()));
    let update_preview: Rc<dyn Fn()> = {
        let controls = controls.clone();
        let current_mappings = current_mappings.clone();
        Rc::new(move || {
            let mappings = compute_batch_rename(&names, &controls.rules());
            render_preview(&list_box, &warning_label, &mappings);
            *current_mappings.borrow_mut() = mappings;
        })
    };
    controls.connect_changed(update_preview.clone());
    update_preview();

    {
        let dlg = shell.window().clone();
        let ui = ui.clone();
        apply_btn.connect_clicked(move |_| {
            let mappings = current_mappings.borrow().clone();
            dlg.close();
            on_apply(&ui, mappings);
        });
    }

    shell.present();
    controls.find_entry.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_case_drop_down_maps_to_its_transformation() {
        assert_eq!(case_from_index(0), CaseTransform::None);
        assert_eq!(case_from_index(1), CaseTransform::Lowercase);
        assert_eq!(case_from_index(2), CaseTransform::Uppercase);
        assert_eq!(case_from_index(3), CaseTransform::TitleCase);
        assert_eq!(case_from_index(4), CaseTransform::SnakeCase);
        assert_eq!(case_from_index(5), CaseTransform::KebabCase);
        assert_eq!(case_from_index(99), CaseTransform::None);
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    /// A chain `a -> b, b -> c` keeps all three contents, in any order.
    #[test]
    fn rename_all_handles_chains() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "A").unwrap();
        std::fs::write(dir.path().join("b"), "B").unwrap();
        let (n, errs) = rename_all(dir.path(), &pairs(&[("a", "b"), ("b", "c")]));
        assert_eq!((n, errs.len()), (2, 0));
        assert_eq!(std::fs::read_to_string(dir.path().join("b")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(dir.path().join("c")).unwrap(), "B");
    }

    /// A target held by a file outside the batch is refused and the source stays.
    #[test]
    fn rename_all_never_replaces_an_outside_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "A").unwrap();
        std::fs::write(dir.path().join("keep"), "K").unwrap();
        let (n, errs) = rename_all(dir.path(), &pairs(&[("a", "keep")]));
        assert_eq!((n, errs.len()), (0, 1));
        assert_eq!(std::fs::read_to_string(dir.path().join("a")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(dir.path().join("keep")).unwrap(), "K");
    }

    /// Two entries mapped to the same name: one wins, the other is restored.
    #[test]
    fn rename_all_restores_the_loser_of_a_name_clash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "A").unwrap();
        std::fs::write(dir.path().join("b"), "B").unwrap();
        let (n, errs) = rename_all(dir.path(), &pairs(&[("a", "x"), ("b", "x")]));
        assert_eq!((n, errs.len()), (1, 1));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        assert!(dir.path().join("x").exists() && dir.path().join("b").exists());
    }

    #[test]
    fn the_summary_lists_every_error() {
        assert_eq!(rename_summary(3, &[]), ("Successfully renamed 3 item(s)".to_string(), None));
        let errors = vec!["a -> x: File exists".to_string(), "b: cannot restore, left as /d/.opusdm-rename-1-1: EIO".to_string()];
        let (headline, detail) = rename_summary(1, &errors);
        assert_eq!(headline, "Renamed 1 item(s) with 2 error(s)");
        let detail = detail.expect("the errors are listed");
        for e in &errors {
            assert!(detail.contains(e.as_str()), "{detail}");
        }
    }

    #[test]
    fn test_compute_batch_rename_simple() {
        let names = vec!["file1.txt".into(), "file2.txt".into()];
        let rules = RenameRules {
            prefix: "new_".into(),
            ..RenameRules::default()
        };
        let res = compute_batch_rename(&names, &rules);
        assert_eq!(res[0].1, "new_file1.txt");
        assert_eq!(res[1].1, "new_file2.txt");
    }

    #[test]
    fn test_compute_batch_rename_numbering() {
        let names = vec!["photo.jpg".into(), "photo.jpg".into()];
        let rules = RenameRules {
            numbering: true,
            number_start: 1,
            number_padding: 3,
            ..RenameRules::default()
        };
        let res = compute_batch_rename(&names, &rules);
        assert_eq!(res[0].1, "photo_001.jpg");
        assert_eq!(res[1].1, "photo_002.jpg");
    }

    #[test]
    fn test_case_transformations() {
        assert_eq!(to_snake_case("My Cool File"), "my_cool_file");
        assert_eq!(to_kebab_case("My Cool File"), "my-cool-file");
        assert_eq!(to_title_case("my_cool_file"), "My_Cool_File");
    }
}
