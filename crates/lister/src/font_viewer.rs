//! ViewFont (F-49): a small previewer for the fonts installed on the system.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;

/// Text the preview starts with.
const SAMPLE_TEXT: &str = "The quick brown fox jumps over the lazy dog\nABCDEFGHIJKLMNOPQRSTUVWXYZ\nabcdefghijklmnopqrstuvwxyz\n0123456789 !?@#%&*()[]{}";

/// Sizes the spin button allows, in points.
const SIZE_RANGE: (f64, f64) = (6.0, 120.0);

/// Size the preview starts at.
const DEFAULT_SIZE: f64 = 24.0;

/// The families of `all` that contain `query` (ignoring case), sorted
/// without regard to case and without duplicates. An empty query keeps all.
pub fn filter_families(all: &[String], query: &str) -> Vec<String> {
    let query = query.trim().to_lowercase();
    let mut kept: Vec<String> = all.iter().filter(|f| query.is_empty() || f.to_lowercase().contains(&query)).cloned().collect();
    kept.sort_by_key(|f| f.to_lowercase());
    kept.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    kept
}

/// The Pango font description text for a choice, e.g. `DejaVu Sans Bold Italic 24`.
pub fn description(family: &str, size: f64, bold: bool, italic: bool) -> String {
    let mut text = family.to_string();
    if bold {
        text.push_str(" Bold");
    }
    if italic {
        text.push_str(" Italic");
    }
    format!("{text} {}", size.round() as i64)
}

/// The CSS that makes the `fontsample` widgets show the chosen font. Quotes
/// and backslashes in the family are escaped.
pub fn sample_css(family: &str, size: f64, bold: bool, italic: bool) -> String {
    let family = family.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        ".fontsample {{ font-family: \"{family}\"; font-size: {}pt; font-weight: {}; font-style: {}; }}",
        size.round() as i64,
        if bold { "bold" } else { "normal" },
        if italic { "italic" } else { "normal" },
    )
}

/// Every installed family name.
fn installed_families(window: &impl IsA<gtk::Widget>) -> Vec<String> {
    let Some(map) = window.pango_context().font_map() else { return Vec::new() };
    map.list_families().iter().map(|f| f.name().to_string()).collect()
}

/// Open the previewer.
pub fn open(ui: &Rc<Ui>) {
    let families = installed_families(&ui.window);
    let shell = opusdm_ui::DialogShell::new(&ui.window, "View Font").modal(false).size(760, 460);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    body.set_vexpand(true);
    shell.content().append(&body);

    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let search = gtk::SearchEntry::new();
    let list = gtk::ListBox::new();
    let list_scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).min_content_width(240).build();
    left.append(&search);
    left.append(&list_scroll);
    body.append(&left);

    let right = gtk::Box::new(gtk::Orientation::Vertical, 6);
    right.set_hexpand(true);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let size = gtk::SpinButton::with_range(SIZE_RANGE.0, SIZE_RANGE.1, 1.0);
    size.set_value(DEFAULT_SIZE);
    let bold = gtk::CheckButton::with_label("Bold");
    let italic = gtk::CheckButton::with_label("Italic");
    controls.append(&gtk::Label::new(Some("Size")));
    controls.append(&size);
    controls.append(&bold);
    controls.append(&italic);
    right.append(&controls);
    let sample = gtk::TextView::new();
    sample.buffer().set_text(SAMPLE_TEXT);
    sample.set_wrap_mode(gtk::WrapMode::WordChar);
    let sample_scroll = gtk::ScrolledWindow::builder().child(&sample).vexpand(true).hexpand(true).build();
    right.append(&sample_scroll);
    let name_label = gtk::Label::new(None);
    name_label.set_xalign(0.0);
    right.append(&name_label);
    body.append(&right);
    shell.close_button("Close");
    let window = shell.present();

    let fill = {
        let list = list.clone();
        Rc::new(move |names: Vec<String>| {
            while let Some(row) = list.first_child() {
                list.remove(&row);
            }
            for name in names {
                let label = gtk::Label::new(Some(&name));
                label.set_xalign(0.0);
                label.set_margin_start(6);
                label.set_margin_top(3);
                label.set_margin_bottom(3);
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&label));
                list.append(&row);
            }
        })
    };
    fill(filter_families(&families, ""));
    {
        let (families, fill) = (families.clone(), fill.clone());
        search.connect_search_changed(move |s| fill(filter_families(&families, s.text().as_str())));
    }

    sample.add_css_class("fontsample");
    let provider = gtk::CssProvider::new();
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
    }
    let preview = {
        let (list, size, bold, italic, name_label, provider) = (list.clone(), size.clone(), bold.clone(), italic.clone(), name_label.clone(), provider.clone());
        Rc::new(move || {
            let Some(family) = list
                .selected_row()
                .and_then(|r| r.child())
                .and_downcast::<gtk::Label>()
                .map(|l| l.text().to_string())
            else {
                return;
            };
            provider.load_from_data(&sample_css(&family, size.value(), bold.is_active(), italic.is_active()));
            name_label.set_text(&description(&family, size.value(), bold.is_active(), italic.is_active()));
        })
    };
    {
        let preview = preview.clone();
        list.connect_row_selected(move |_, _| preview());
    }
    {
        let preview = preview.clone();
        size.connect_value_changed(move |_| preview());
    }
    for check in [&bold, &italic] {
        let preview = preview.clone();
        check.connect_toggled(move |_| preview());
    }
    window.connect_close_request(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_remove_provider_for_display(&display, &provider);
        }
        gtk::glib::Propagation::Proceed
    });
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn filtering_ignores_case_sorts_and_removes_duplicates() {
        let all = names(&["Zilla", "dejavu sans", "DejaVu Serif", "Dejavu Sans", "Arial"]);
        assert_eq!(filter_families(&all, ""), names(&["Arial", "dejavu sans", "DejaVu Serif", "Zilla"]));
        assert_eq!(filter_families(&all, " SERIF "), names(&["DejaVu Serif"]));
        assert!(filter_families(&all, "nothing").is_empty());
    }

    #[test]
    fn the_css_escapes_the_family_and_sets_the_style() {
        let css = sample_css("Weird \"Name\"", 18.4, true, false);
        assert!(css.contains("font-family: \"Weird \\\"Name\\\"\""), "{css}");
        assert!(css.contains("font-size: 18pt") && css.contains("font-weight: bold") && css.contains("font-style: normal"), "{css}");
    }

    #[test]
    fn the_description_carries_family_style_and_size() {
        assert_eq!(description("DejaVu Sans", 24.0, false, false), "DejaVu Sans 24");
        assert_eq!(description("Fira Code", 12.4, true, true), "Fira Code Bold Italic 12");
    }
}
