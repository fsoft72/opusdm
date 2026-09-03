//! Column setup and cell rendering for the Lister ColumnView.

use crate::model::{
    format_access, format_group, format_mtime, format_size, format_user,
    local_tz_offset_secs, Row,
};
use gtk4 as gtk;
use gtk::glib::BoxedAnyObject;
use gtk::prelude::*;
use std::rc::Rc;

#[derive(Clone, Copy)]
pub enum ColKind {
    Name,
    Size,
    Modified,
    User,
    Group,
    Access,
}

/// Toggle the CSS class that shows selection. Needed because row-dir/row-file
/// paint an opaque background on every cell label, which otherwise sits on
/// top of (and hides) GTK's normal selection highlight.
pub fn apply_selected_class(label: &gtk::Label, selected: bool) {
    if selected {
        label.add_css_class("cell-selected");
    } else {
        label.remove_css_class("cell-selected");
    }
}

pub fn add_column(view: &gtk::ColumnView, title: &str, kind: ColKind) {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, list_item| {
        let item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_valign(gtk::Align::Fill);
        if matches!(kind, ColKind::Access) {
            label.add_css_class("monospace");
        }
        // Selection can change without a rebind (no row.item() change), so
        // this has to be tracked on its own signal, not just in bind.
        {
            let label = label.clone();
            item.connect_selected_notify(move |li| apply_selected_class(&label, li.is_selected()));
        }
        item.set_child(Some(&label));
    });
    let tz = local_tz_offset_secs();
    factory.connect_bind(move |_, list_item| {
        let item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = item
            .item()
            .and_downcast::<BoxedAnyObject>()
            .map(|o| o.borrow::<Rc<Row>>().clone());
        let Some(row) = row else { return };
        let label = item.child().and_downcast::<gtk::Label>().unwrap();
        // Every cell in the row gets the same dir/file background+text CSS
        // class, so the whole row reads as one colored line.
        label.remove_css_class("row-dir");
        label.remove_css_class("row-file");
        label.add_css_class(if row.is_dir { "row-dir" } else { "row-file" });
        // A rebind reuses this widget for a possibly different row; resync
        // the selection highlight to the row now bound, not the old one.
        apply_selected_class(&label, item.is_selected());
        match kind {
            ColKind::Name => {
                let mark = if row.broken_link {
                    " (broken link)"
                } else if row.is_symlink {
                    " ->"
                } else {
                    ""
                };
                label.set_xalign(0.0);
                label.set_text(&format!("{}{}", row.name, mark));
            }
            ColKind::Size => {
                label.set_xalign(1.0);
                label.set_text(&format_size(row.is_dir, row.size));
            }
            ColKind::Modified => {
                label.set_xalign(0.0);
                label.set_text(&format_mtime(row.modified_unix, tz));
            }
            ColKind::User => {
                label.set_xalign(0.0);
                label.set_text(&format_user(row.uid));
            }
            ColKind::Group => {
                label.set_xalign(0.0);
                label.set_text(&format_group(row.gid));
            }
            ColKind::Access => {
                label.set_xalign(0.0);
                label.set_text(&format_access(row.mode, row.is_dir, row.is_symlink));
            }
        }
    });
    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_expand(matches!(kind, ColKind::Name));
    column.set_resizable(true);
    view.append_column(&column);
}
