//! "Advanced Select..." requester (F-21): select or deselect the listed rows
//! that satisfy a set of criteria.

use crate::select_criteria::{Criteria, Fields, KindFilter};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Choices of the type dropdown, in display order.
const KIND_CHOICES: [(&str, KindFilter); 3] = [
    ("Files and directories", KindFilter::Any),
    ("Files only", KindFilter::FilesOnly),
    ("Directories only", KindFilter::DirsOnly),
];

/// Now, as Unix seconds.
fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Add a labelled entry row to `content` and return the entry.
pub(crate) fn add_field(content: &gtk::Box, label: &str, placeholder: &str) -> gtk::Entry {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_width_chars(16);
    line.append(&text);
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some(placeholder));
    line.append(&entry);
    content.append(&line);
    entry
}

/// Select (`select`) or deselect the listed rows that match `criteria`.
/// Returns how many rows matched.
fn apply(ui: &Rc<Ui>, criteria: &Criteria, select: bool) -> u32 {
    let mut matched = 0;
    for i in 0..ui.view.selection.n_items() {
        if !ui.row_at(i).is_some_and(|row| criteria.matches(&row)) {
            continue;
        }
        matched += 1;
        if select {
            ui.view.selection.select_item(i, false);
        } else {
            ui.view.selection.unselect_item(i);
        }
    }
    matched
}

/// Open the requester.
pub fn open(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Advanced Select").size(440, 400);
    let content = shell.content().clone();
    let name = add_field(&content, "Name", "*.rs, *.toml, !test*");
    let min_size = add_field(&content, "Size at least", "1M");
    let max_size = add_field(&content, "Size at most", "2G");
    let after = add_field(&content, "Modified after", "7d or 2026-09-01 08:00");
    let before = add_field(&content, "Modified before", "30d or 2026-09-30");
    let kinds: Vec<&str> = KIND_CHOICES.iter().map(|(label, _)| *label).collect();
    let kind = gtk::DropDown::from_strings(&kinds);
    content.append(&gtk::Label::new(Some("Type")));
    content.append(&kind);
    let hint = gtk::Label::new(Some(
        "Empty fields are ignored. Times: 30m, 12h, 7d, 2w ago, or YYYY-MM-DD [HH:MM]. A size limit never matches a directory.",
    ));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    content.append(&hint);
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.set_wrap(true);
    error.add_css_class("error");
    content.append(&error);

    let select_btn = shell.button("Select");
    let deselect_btn = shell.button("Deselect");
    shell.close_button("Cancel");
    let window = shell.window().clone();

    for (button, select) in [(select_btn, true), (deselect_btn, false)] {
        let (ui, window, error) = (ui.clone(), window.clone(), error.clone());
        let (name, min_size, max_size, after, before, kind) =
            (name.clone(), min_size.clone(), max_size.clone(), after.clone(), before.clone(), kind.clone());
        button.connect_clicked(move |_| {
            let (n, lo, hi, a, b) = (name.text(), min_size.text(), max_size.text(), after.text(), before.text());
            let fields = Fields { name: &n, min_size: &lo, max_size: &hi, after: &a, before: &b };
            let kind = KIND_CHOICES.get(kind.selected() as usize).map(|(_, k)| *k).unwrap_or(KindFilter::Any);
            let criteria = match Criteria::parse(&fields, kind, now_secs()) {
                Ok(c) => c,
                Err(msg) => {
                    error.set_text(&msg);
                    return;
                }
            };
            let matched = apply(&ui, &criteria, select);
            let verb = if select { "selected" } else { "deselected" };
            crate::actions::flash(&ui, &format!("{matched} {verb}"));
            window.close();
        });
    }
    shell.present();
}
