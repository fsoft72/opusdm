//! Column setup and cell rendering for the Lister ColumnView.

use crate::model::{
    dir_first_cmp, format_access, format_group, format_mtime, format_size, format_user,
    local_tz_offset_secs, Row, SortCol,
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

impl ColKind {
    /// Ordering key this column sorts on when its header is clicked.
    fn sort_col(self) -> SortCol {
        match self {
            ColKind::Name => SortCol::Name,
            ColKind::Size => SortCol::Size,
            ColKind::Modified => SortCol::Modified,
            ColKind::User => SortCol::User,
            ColKind::Group => SortCol::Group,
            ColKind::Access => SortCol::Access,
        }
    }
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
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let title_owned = title.to_string();
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
                    if !ui.view.selection.is_selected(pos) {
                        ui.view.selection.select_item(pos, true);
                    }

                    let target_path = ui.cwd_path().join(&row.name);
                    crate::context_menu::show_context_menu(&ui, &label, &row, target_path, x, y);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                });
                label.add_controller(click);
            }

            // Drag source: read the row bound to this cell right now, not the
            // selection model - GtkColumnView applies a click's selection
            // change on release, which lands after the drag threshold has
            // already fired `connect_prepare`, so trusting `ui.view.selection`
            // here would see stale (often empty) state.
            {
                let item_weak = item.downgrade();
                let ui_getter = ui_getter.clone();
                let src = gtk::DragSource::new();
                src.set_actions(gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
                src.connect_prepare(move |_, x, y| {
                    crate::dlog!("drag prepare: cell at ({x}, {y})");
                    let Some(item) = item_weak.upgrade() else {
                        crate::dlog!("drag prepare: item gone");
                        return None;
                    };
                    let Some(ui) = ui_getter() else {
                        crate::dlog!("drag prepare: no ui");
                        return None;
                    };
                    let Some(row) = item
                        .item()
                        .and_downcast::<BoxedAnyObject>()
                        .map(|o| o.borrow::<Rc<Row>>().clone())
                    else {
                        crate::dlog!("drag prepare: no row bound");
                        return None;
                    };
                    let pos = item.position();
                    let paths = if ui.view.selection.is_selected(pos) {
                        ui.selected_paths()
                    } else {
                        vec![ui.cwd_path().join(&row.name)]
                    };
                    crate::dlog!("drag prepare: pos={pos} row={} paths={paths:?}", row.name);
                    if paths.is_empty() {
                        crate::dlog!("drag prepare: empty paths, aborting");
                        return None;
                    }
                    let payload = opusdm_iconview::dnd::paths_to_uri_list(&paths);
                    crate::dlog!("drag prepare: payload={payload:?}");
                    Some(gtk::gdk::ContentProvider::for_bytes(
                        "text/uri-list",
                        &gtk::glib::Bytes::from(payload.as_bytes()),
                    ))
                });
                {
                    let label_id = title_owned.clone();
                    src.connect_drag_begin(move |_, _| {
                        crate::dlog!("drag begin on column {label_id}");
                    });
                }
                src.connect_drag_end(move |_, _, delete| {
                    crate::dlog!("drag end, delete={delete}");
                });
                src.connect_drag_cancel(move |_, _, reason| {
                    crate::dlog!("drag cancel: {reason:?}");
                    false
                });
                label.add_controller(src);
            }

            if matches!(kind, ColKind::Name) {
                let cell = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                let icon = gtk::Image::new();
                icon.set_pixel_size(16);
                icon.set_visible(false);
                cell.append(&icon);
                cell.append(&label);
                item.set_child(Some(&cell));
            } else {
                item.set_child(Some(&label));
            }
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
        let label = if matches!(kind, ColKind::Name) {
            let cell = item.child().and_downcast::<gtk::Box>().unwrap();
            cell.last_child().and_downcast::<gtk::Label>().unwrap()
        } else {
            item.child().and_downcast::<gtk::Label>().unwrap()
        };
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
                let (icon_set, indicators) = ui_getter_bind()
                    .map(|ui| {
                        let s = ui.hub.settings.borrow();
                        (s.theme.icon_set.clone(), s.appearance.type_indicators)
                    })
                    .unwrap_or((opusdm_ipc::IconSetId::default(), false));
                label.set_xalign(0.0);
                let icon_widget = item
                    .child()
                    .and_downcast::<gtk::Box>()
                    .unwrap()
                    .first_child()
                    .and_downcast::<gtk::Image>()
                    .unwrap();
                if let Some(bytes) = crate::glyphs::icon_bytes_for_row(&row, &icon_set) {
                    let gbytes = gtk::glib::Bytes::from_owned(bytes.into_owned());
                    if let Ok(texture) = gtk::gdk::Texture::from_bytes(&gbytes) {
                        icon_widget.set_paintable(Some(&texture));
                        icon_widget.set_visible(true);
                    }
                } else {
                    icon_widget.set_visible(false);
                }
                label.set_text(&crate::glyphs::decorate_name(&row, indicators));
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
    // Per-column sorter: click the header to sort on this key. GTK's
    // ColumnView toggles ascending/descending and draws the arrow indicator
    // on the active column on its own; the sorter always compares ascending
    // and keeps directories on top in both directions.
    let sort_col = kind.sort_col();
    let sorter = gtk::CustomSorter::new(move |a, b| {
        let ra = a.downcast_ref::<BoxedAnyObject>().unwrap().borrow::<Rc<Row>>().clone();
        let rb = b.downcast_ref::<BoxedAnyObject>().unwrap().borrow::<Rc<Row>>().clone();
        dir_first_cmp(&ra, &rb, sort_col, false, opusdm_ipc::Separate::DirsFirst).into()
    });

    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_expand(matches!(kind, ColKind::Name));
    column.set_resizable(true);
    column.set_sorter(Some(&sorter));
    view.append_column(&column);
    column
}
