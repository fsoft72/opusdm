//! Advanced Batch Renamer with live preview, regex and numbering.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
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

/// Open the Batch Rename dialog for the currently selected files.
pub fn open_batch_rename_dialog(ui: &Rc<Ui>) {
    let names = ui.selected_names();
    if names.is_empty() {
        crate::actions::flash(ui, "no files selected for batch rename");
        return;
    }

    let cwd = ui.cwd_path();

    let shell = opusdm_ui::DialogShell::new(
        &ui.window,
        &format!("Batch Rename - {} items", names.len()),
    )
    .size(680, 520);
    let root = shell.content();

    // Controls grid
    let grid = gtk::Grid::builder()
        .column_spacing(8)
        .row_spacing(6)
        .build();

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

    root.append(&grid);

    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Live preview table
    let preview_header = gtk::Label::new(Some("Live Preview:"));
    preview_header.set_xalign(0.0);
    preview_header.add_css_class("dim-label");
    root.append(&preview_header);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);

    let scroller = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .vexpand(true)
        .hexpand(true)
        .build();
    root.append(&scroller);

    let warning_label = gtk::Label::new(None);
    warning_label.set_xalign(0.0);
    warning_label.add_css_class("error-text");
    root.append(&warning_label);

    // Buttons
    shell.close_button("Cancel");
    let apply_btn = shell.button("Apply Rename");
    apply_btn.add_css_class("suggested-action");

    let current_mappings = Rc::new(RefCell::new(Vec::<(String, String)>::new()));

    // Closure to update live preview
    let update_preview = {
        let names = names.clone();
        let find_entry = find_entry.clone();
        let rep_entry = rep_entry.clone();
        let regex_check = regex_check.clone();
        let pre_entry = pre_entry.clone();
        let suf_entry = suf_entry.clone();
        let ext_check = ext_check.clone();
        let case_combo = case_combo.clone();
        let num_check = num_check.clone();
        let list_box = list_box.clone();
        let warning_label = warning_label.clone();
        let current_mappings = current_mappings.clone();

        Rc::new(move || {
            let case = match case_combo.selected() {
                1 => CaseTransform::Lowercase,
                2 => CaseTransform::Uppercase,
                3 => CaseTransform::TitleCase,
                4 => CaseTransform::SnakeCase,
                5 => CaseTransform::KebabCase,
                _ => CaseTransform::None,
            };

            let rules = RenameRules {
                find: find_entry.text().to_string(),
                replace: rep_entry.text().to_string(),
                use_regex: regex_check.is_active(),
                prefix: pre_entry.text().to_string(),
                suffix: suf_entry.text().to_string(),
                numbering: num_check.is_active(),
                number_start: 1,
                number_step: 1,
                number_padding: 3,
                case,
                preserve_ext: ext_check.is_active(),
            };

            let mappings = compute_batch_rename(&names, &rules);
            *current_mappings.borrow_mut() = mappings.clone();

            // Clear list
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }

            let mut seen_new_names = HashSet::new();
            let mut has_conflict = false;

            for (orig, new) in &mappings {
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

                if !seen_new_names.insert(new.clone()) {
                    new_lbl.add_css_class("error-text");
                    has_conflict = true;
                } else if orig != new {
                    new_lbl.add_css_class("success-text");
                }

                hbox.append(&orig_lbl);
                hbox.append(&arrow_lbl);
                hbox.append(&new_lbl);
                row.set_child(Some(&hbox));
                list_box.append(&row);
            }

            if has_conflict {
                warning_label.set_text("⚠️ Warning: Multiple items have identical new names (collision)!");
            } else {
                warning_label.set_text("");
            }
        })
    };

    // Connect inputs to live preview update
    {
        let up = update_preview.clone();
        find_entry.connect_changed(move |_| up());
    }
    {
        let up = update_preview.clone();
        rep_entry.connect_changed(move |_| up());
    }
    {
        let up = update_preview.clone();
        regex_check.connect_toggled(move |_| up());
    }
    {
        let up = update_preview.clone();
        pre_entry.connect_changed(move |_| up());
    }
    {
        let up = update_preview.clone();
        suf_entry.connect_changed(move |_| up());
    }
    {
        let up = update_preview.clone();
        ext_check.connect_toggled(move |_| up());
    }
    {
        let up = update_preview.clone();
        num_check.connect_toggled(move |_| up());
    }
    {
        let up = update_preview.clone();
        case_combo.connect_selected_notify(move |_| up());
    }

    // Initial calculation
    update_preview();

    // Apply
    {
        let dlg = shell.window().clone();
        let ui_clone = ui.clone();
        let current_mappings = current_mappings.clone();
        apply_btn.connect_clicked(move |_| {
            let mappings = current_mappings.borrow().clone();
            let mut renamed_count = 0;
            let mut errors = Vec::new();

            for (orig, new) in mappings {
                if orig == new {
                    continue;
                }
                let old_p = cwd.join(&orig);
                let new_p = cwd.join(&new);
                match std::fs::rename(&old_p, &new_p) {
                    Ok(()) => renamed_count += 1,
                    Err(e) => errors.push(format!("{orig} -> {new}: {e}")),
                }
            }

            dlg.close();
            ui_clone.reload();

            if !errors.is_empty() {
                crate::actions::flash(
                    &ui_clone,
                    &format!("Renamed {renamed_count} item(s) with {} error(s)", errors.len()),
                );
            } else {
                crate::actions::flash(
                    &ui_clone,
                    &format!("Successfully renamed {renamed_count} item(s)"),
                );
            }
        });
    }

    shell.present();
    find_entry.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_batch_rename_simple() {
        let names = vec!["file1.txt".into(), "file2.txt".into()];
        let mut rules = RenameRules::default();
        rules.prefix = "new_".into();
        let res = compute_batch_rename(&names, &rules);
        assert_eq!(res[0].1, "new_file1.txt");
        assert_eq!(res[1].1, "new_file2.txt");
    }

    #[test]
    fn test_compute_batch_rename_numbering() {
        let names = vec!["photo.jpg".into(), "photo.jpg".into()];
        let mut rules = RenameRules::default();
        rules.numbering = true;
        rules.number_start = 1;
        rules.number_padding = 3;
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
