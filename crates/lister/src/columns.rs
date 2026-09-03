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

pub fn add_column(
    view: &gtk::ColumnView,
    title: &str,
    kind: ColKind,
    ui_getter: impl Fn() -> Option<Rc<crate::window::Ui>> + 'static + Clone,
) {
    let factory = gtk::SignalListItemFactory::new();
    {
        let ui_getter = ui_getter.clone();
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

            // Right-click context menu gesture on cell
            {
                let click = gtk::GestureClick::new();
                click.set_button(gtk::gdk::BUTTON_SECONDARY);
                let label_weak = label.downgrade();
                let item_weak = item.downgrade();
                let ui_getter = ui_getter.clone();
                click.connect_pressed(move |gesture, n_press, x, y| {
                    if n_press != 1 {
                        return;
                    }
                    let Some(label) = label_weak.upgrade() else {
                        return;
                    };
                    let Some(item) = item_weak.upgrade() else {
                        return;
                    };
                    let Some(ui) = ui_getter() else {
                        return;
                    };
                    let row = item
                        .item()
                        .and_downcast::<BoxedAnyObject>()
                        .map(|o| o.borrow::<Rc<Row>>().clone());
                    let Some(row) = row else {
                        return;
                    };

                    // If clicked item is not selected, select it
                    let pos = item.position();
                    if !ui.selection.is_selected(pos) {
                        ui.selection.select_item(pos, true);
                    }

                    let target_path = ui.cwd_path().join(&row.name);
                    crate::context_menu::show_context_menu(&ui, &label, &row, target_path, x, y);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                });
                label.add_controller(click);
            }

            item.set_child(Some(&label));
        });
    }
    let tz = local_tz_offset_secs();
    let ui_getter_bind = ui_getter.clone();
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
                let (icons, indicators) = ui_getter_bind()
                    .map(|ui| {
                        let a = &ui.settings.borrow().appearance;
                        (a.file_icons, a.type_indicators)
                    })
                    .unwrap_or((false, false));
                label.set_xalign(0.0);
                label.set_text(&crate::glyphs::decorate_name(&row, icons, indicators));
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
