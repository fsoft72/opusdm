//! "Lister Format..." dialog (F-22) and the Show / Hide pattern prompts (F-23).

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::{FormatColumn, ListerFormat, Separate, ToHub};
use std::cell::RefCell;
use std::rc::Rc;

/// Choices of the separation dropdown, in display order.
pub const SEPARATE_CHOICES: [(&str, Separate); 3] = [
    ("Directories first", Separate::DirsFirst),
    ("Files first", Separate::FilesFirst),
    ("Mixed", Separate::Mixed),
];

/// Swap the row at `idx` with its neighbour above (`up`) or below. Returns the
/// new index, or `None` when the row is already at that edge.
pub fn move_column(rows: &mut [(FormatColumn, bool)], idx: usize, up: bool) -> Option<usize> {
    let target = if up { idx.checked_sub(1)? } else { idx + 1 };
    if idx >= rows.len() || target >= rows.len() {
        return None;
    }
    rows.swap(idx, target);
    Some(target)
}

/// The editor's column list for `format`: shown columns in order (checked),
/// then the hidden ones (unchecked).
pub fn rows_from_format(format: &ListerFormat) -> Vec<(FormatColumn, bool)> {
    let mut rows: Vec<(FormatColumn, bool)> = format.columns.iter().map(|c| (*c, true)).collect();
    rows.extend(format.hidden_columns().into_iter().map(|c| (c, false)));
    rows
}

/// Build the format the editor widgets describe.
pub fn format_from_rows(
    rows: &[(FormatColumn, bool)],
    sort: FormatColumn,
    reverse: bool,
    separate: Separate,
    show: &str,
    hide: &str,
    gauge: bool,
) -> ListerFormat {
    ListerFormat {
        columns: rows.iter().filter(|(_, on)| *on).map(|(c, _)| *c).collect(),
        sort,
        reverse,
        separate,
        show_glob: show.trim().to_string(),
        hide_glob: hide.trim().to_string(),
        gauge,
    }
    .sanitized()
}

/// Which pattern list a prompt edits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    Show,
    Hide,
}

/// Rebuild the column list widget from `rows`.
fn rebuild(list: &gtk::Box, rows: &Rc<RefCell<Vec<(FormatColumn, bool)>>>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let snapshot = rows.borrow().clone();
    for (idx, (column, on)) in snapshot.iter().enumerate() {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let check = gtk::CheckButton::with_label(column.title());
        check.set_active(*on);
        check.set_hexpand(true);
        // Name is what the list is about; it can move but not disappear.
        check.set_sensitive(*column != FormatColumn::Name);
        {
            let rows = rows.clone();
            let column = *column;
            check.connect_toggled(move |c| {
                if let Some(row) = rows.borrow_mut().iter_mut().find(|r| r.0 == column) {
                    row.1 = c.is_active();
                }
            });
        }
        line.append(&check);
        for (label, up) in [("Up", true), ("Down", false)] {
            let button = gtk::Button::with_label(label);
            let (list, rows) = (list.clone(), rows.clone());
            button.connect_clicked(move |_| {
                let moved = move_column(&mut rows.borrow_mut(), idx, up);
                if moved.is_some() {
                    rebuild(&list, &rows);
                }
            });
            line.append(&button);
        }
        list.append(&line);
    }
}

/// Send the store to the hub. The `FormatsUpdate` that comes back applies it.
fn send_store(ui: &Rc<Ui>, store: opusdm_ipc::FormatStore) {
    *ui.format_state.temp.borrow_mut() = None;
    ui.invalidate_format();
    if let Err(e) = ui.hub.client.send(&ToHub::SetFormats(store)) {
        crate::actions::flash(ui, &format!("cannot save format: {e}"));
    }
}

/// Open the "Lister Format..." dialog for the current directory.
pub fn open(ui: &Rc<Ui>) {
    let dir = ui.cwd_path();
    if crate::archive::parse_archive_path(&dir).is_some() {
        crate::actions::flash(ui, "formats do not apply inside an archive");
        return;
    }
    let initial = ui.effective_format(&dir);
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Lister Format").size(480, 600).scrolled();

    let rows = Rc::new(RefCell::new(rows_from_format(&initial)));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    shell.content().append(&gtk::Label::new(Some("Columns (top to bottom = left to right)")));
    shell.content().append(&list);
    rebuild(&list, &rows);

    let titles: Vec<&str> = FormatColumn::ALL.iter().map(|c| c.title()).collect();
    let sort_dd = gtk::DropDown::from_strings(&titles);
    sort_dd.set_selected(FormatColumn::ALL.iter().position(|c| *c == initial.sort).unwrap_or(0) as u32);
    shell.content().append(&gtk::Label::new(Some("Sort by")));
    shell.content().append(&sort_dd);
    let reverse = gtk::CheckButton::with_label("Reverse (descending)");
    reverse.set_active(initial.reverse);
    shell.content().append(&reverse);

    let separate_labels: Vec<&str> = SEPARATE_CHOICES.iter().map(|(l, _)| *l).collect();
    let separate_dd = gtk::DropDown::from_strings(&separate_labels);
    separate_dd.set_selected(SEPARATE_CHOICES.iter().position(|(_, s)| *s == initial.separate).unwrap_or(0) as u32);
    shell.content().append(&gtk::Label::new(Some("Separation")));
    shell.content().append(&separate_dd);

    let show = gtk::Entry::new();
    show.set_text(&initial.show_glob);
    show.set_placeholder_text(Some("*.rs, *.toml, !test*"));
    let hide = gtk::Entry::new();
    hide.set_text(&initial.hide_glob);
    hide.set_placeholder_text(Some("*.o, *.tmp, !keep.o"));
    shell.content().append(&gtk::Label::new(Some("Show only (files; empty = all)")));
    shell.content().append(&show);
    shell.content().append(&gtk::Label::new(Some("Hide (files)")));
    shell.content().append(&hide);
    let gauge = gtk::CheckButton::with_label("Free space gauge");
    gauge.set_active(initial.gauge);
    shell.content().append(&gauge);

    // Read the widgets back into a format.
    let collect: Rc<dyn Fn() -> ListerFormat> = {
        let (rows, sort_dd, reverse, separate_dd, show, hide, gauge) =
            (rows.clone(), sort_dd.clone(), reverse.clone(), separate_dd.clone(), show.clone(), hide.clone(), gauge.clone());
        Rc::new(move || {
            let sort = FormatColumn::ALL.get(sort_dd.selected() as usize).copied().unwrap_or(FormatColumn::Name);
            let separate = SEPARATE_CHOICES.get(separate_dd.selected() as usize).map(|(_, s)| *s).unwrap_or_default();
            format_from_rows(&rows.borrow(), sort, reverse.is_active(), separate, show.text().as_str(), hide.text().as_str(), gauge.is_active())
        })
    };

    let window = shell.window().clone();
    let use_btn = shell.button("Use");
    let save_dir = shell.button("Save (this directory)");
    let save_tree = shell.button("Save (with subdirectories)");
    let save_default = shell.button("Save as default");
    let remove = shell.button("Remove saved format");
    let reset = shell.button("Reset");
    shell.close_button("Cancel");

    {
        let (ui, collect, window) = (ui.clone(), collect.clone(), window.clone());
        use_btn.connect_clicked(move |_| {
            ui.use_format(collect());
            window.close();
        });
    }
    for (button, inherit) in [(save_dir, Some(false)), (save_tree, Some(true)), (save_default, None)] {
        let (ui, collect, window, dir) = (ui.clone(), collect.clone(), window.clone(), dir.clone());
        button.connect_clicked(move |_| {
            let mut store = ui.hub.formats.borrow().clone();
            match inherit {
                Some(inherit) => store.upsert(dir.clone(), inherit, collect()),
                None => store.set_default(collect()),
            }
            send_store(&ui, store);
            window.close();
        });
    }
    {
        let (ui, window, dir) = (ui.clone(), window.clone(), dir.clone());
        remove.connect_clicked(move |_| {
            let mut store = ui.hub.formats.borrow().clone();
            if !store.remove(&dir) {
                crate::actions::flash(&ui, "this directory has no saved format of its own");
                return;
            }
            send_store(&ui, store);
            window.close();
        });
    }
    {
        let (rows, list, sort_dd, reverse, separate_dd, show, hide, gauge) =
            (rows.clone(), list.clone(), sort_dd.clone(), reverse.clone(), separate_dd.clone(), show.clone(), hide.clone(), gauge.clone());
        reset.connect_clicked(move |_| {
            *rows.borrow_mut() = rows_from_format(&ListerFormat::builtin());
            rebuild(&list, &rows);
            sort_dd.set_selected(0);
            reverse.set_active(false);
            separate_dd.set_selected(0);
            show.set_text("");
            hide.set_text("");
            gauge.set_active(false);
        });
    }
    shell.present();
}

/// Ask for a Show or Hide pattern list and save it where the format in effect
/// came from (F-23). An empty answer clears the list.
pub fn set_pattern(ui: &Rc<Ui>, kind: PatternKind) {
    let dir = ui.cwd_path();
    if crate::archive::parse_archive_path(&dir).is_some() {
        crate::actions::flash(ui, "patterns do not apply inside an archive");
        return;
    }
    let format = ui.effective_format(&dir);
    let (title, initial) = match kind {
        PatternKind::Show => ("Show only files matching (empty = all)", format.show_glob.clone()),
        PatternKind::Hide => ("Hide files matching (empty = none)", format.hide_glob.clone()),
    };
    let ui = ui.clone();
    opusdm_ui::requester_entry(&ui.window.clone(), title, &initial, &[("OK", true), ("Cancel", false)], move |accepted, text| {
        if !accepted {
            return;
        }
        let mut format = ui.effective_format(&dir);
        match kind {
            PatternKind::Show => format.show_glob = text.trim().to_string(),
            PatternKind::Hide => format.hide_glob = text.trim().to_string(),
        }
        let source = ui.format_source(&dir);
        let mut store = ui.hub.formats.borrow().clone();
        store.save_from(&source, &dir, format);
        send_store(&ui, store);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_row_swaps_it_with_its_neighbour() {
        let mut rows = rows_from_format(&ListerFormat::builtin());
        assert_eq!(move_column(&mut rows, 1, true), Some(0));
        assert_eq!(rows[0].0, FormatColumn::Size);
        assert_eq!(rows[1].0, FormatColumn::Name);
        assert_eq!(move_column(&mut rows, 0, false), Some(1));
        assert_eq!(rows[0].0, FormatColumn::Name);
    }

    #[test]
    fn moving_past_either_edge_does_nothing() {
        let mut rows = rows_from_format(&ListerFormat::builtin());
        let before = rows.clone();
        assert_eq!(move_column(&mut rows, 0, true), None);
        let last = rows.len() - 1;
        assert_eq!(move_column(&mut rows, last, false), None);
        assert_eq!(move_column(&mut rows, 99, true), None);
        assert_eq!(rows, before);
    }

    #[test]
    fn rows_list_shown_columns_first_then_hidden() {
        let f = ListerFormat { columns: vec![FormatColumn::Size, FormatColumn::Name], ..ListerFormat::builtin() };
        let rows = rows_from_format(&f);
        assert_eq!(&rows[..2], &[(FormatColumn::Size, true), (FormatColumn::Name, true)]);
        assert!(rows[2..].iter().all(|(_, on)| !on));
        assert_eq!(rows.len(), FormatColumn::ALL.len());
    }

    #[test]
    fn rows_roundtrip_through_format_from_rows() {
        let f = ListerFormat {
            columns: vec![FormatColumn::Date, FormatColumn::Name],
            sort: FormatColumn::Date,
            reverse: true,
            separate: Separate::Mixed,
            show_glob: "*.rs".into(),
            hide_glob: "*.o".into(),
            gauge: true,
        };
        let back = format_from_rows(&rows_from_format(&f), f.sort, f.reverse, f.separate, "*.rs", "*.o", true);
        assert_eq!(back, f);
    }

    #[test]
    fn unchecking_name_still_keeps_name_and_a_hidden_sort_falls_back() {
        let mut rows = rows_from_format(&ListerFormat::builtin());
        for r in rows.iter_mut() {
            r.1 = r.0 == FormatColumn::Size;
        }
        let f = format_from_rows(&rows, FormatColumn::Date, false, Separate::DirsFirst, "", "", false);
        assert_eq!(f.columns, vec![FormatColumn::Size, FormatColumn::Name]);
        assert_eq!(f.sort, FormatColumn::Name);
    }

    #[test]
    fn patterns_are_trimmed() {
        let f = format_from_rows(&rows_from_format(&ListerFormat::builtin()), FormatColumn::Name, false, Separate::DirsFirst, "  *.rs ", " ", false);
        assert_eq!(f.show_glob, "*.rs");
        assert_eq!(f.hide_glob, "");
    }
}
