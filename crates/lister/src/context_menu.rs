//! Context menu (right-click / secondary click) on Lister rows.

use crate::keys::CommandId;
use crate::model::Row;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The popover being filled: each item closes it, then runs its action.
struct Menu {
    ui: Rc<Ui>,
    popover: gtk::Popover,
    items: gtk::Box,
}

impl Menu {
    /// An empty menu popping up at (`x`, `y`) of `anchor`.
    fn new(ui: &Rc<Ui>, anchor: &impl IsA<gtk::Widget>, x: f64, y: f64) -> Self {
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        popover.set_has_arrow(true);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let items = gtk::Box::new(gtk::Orientation::Vertical, 0);
        items.set_margin_start(2);
        items.set_margin_end(2);
        items.set_margin_top(2);
        items.set_margin_bottom(2);
        items.add_css_class("context-menu");
        Self { ui: ui.clone(), popover, items }
    }

    /// Add a flat, left-aligned button that closes the menu and runs `action`.
    fn item(&self, title: &str, action: impl Fn(&Rc<Ui>) + 'static) {
        let btn = gtk::Button::with_label(title);
        btn.add_css_class("flat");
        btn.set_halign(gtk::Align::Fill);
        if let Some(child) = btn.child().and_downcast::<gtk::Label>() {
            child.set_xalign(0.0);
        }
        let ui = self.ui.clone();
        let popover = self.popover.clone();
        btn.connect_clicked(move |_| {
            popover.popdown();
            action(&ui);
        });
        self.items.append(&btn);
    }

    /// Add a horizontal separator.
    fn separator(&self) {
        self.items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }

    /// Show the menu.
    fn popup(self) {
        self.popover.set_child(Some(&self.items));
        self.popover.popup();
    }
}

/// Set `path` as the desktop background. The path goes in as a literal
/// template, so a name holding a `{...}` placeholder is refused instead of
/// being expanded.
pub(crate) fn set_background(ui: &Rc<Ui>, path: &Path) {
    let text = path.to_string_lossy().into_owned();
    if text.contains('{') {
        crate::actions::flash(ui, "cannot set a background from a path containing '{'");
        return;
    }
    let mut args = std::collections::BTreeMap::new();
    args.insert("path".to_string(), text);
    let steps = vec![opusdm_ipc::function::FnStep::Internal {
        cmd: opusdm_ipc::function::InternalCmd::SetBackground,
        args: opusdm_ipc::function::InternalArgs(args),
    }];
    ui.start_run(opusdm_ipc::ToHub::RunFunction { steps });
}

/// Leave the selection (when it holds `path`, else `path` alone) out on the
/// desktop. A path that cannot be left out (inside an archive, say) is
/// refused with a message.
pub(crate) fn leave_out(ui: &Rc<Ui>, path: &Path) {
    let paths = crate::crypt_dialog::targets_for(ui.selected_paths(), path);
    if let Err(msg) = opusdm_ipc::leftout::check_leavable(&paths) {
        crate::actions::flash(ui, &msg);
        return;
    }
    if let Err(e) = ui.hub.client.send(&opusdm_ipc::ToHub::LeaveOut { paths }) {
        crate::actions::flash(ui, &format!("lost the connection to the hub: {e}"));
    }
}

/// The paths `opusdm-viewer` can open: all of `paths` when every one is on
/// the local disk, else an error naming the first that is not (an entry
/// inside an archive or on a remote server).
pub fn viewer_targets(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    if paths.is_empty() { return Err("nothing selected".to_string()); }
    if let Some(missing) = paths.iter().find(|p| !p.exists()) {
        return Err(format!("cannot open {} in the viewer: not a local file", missing.display()));
    }
    Ok(paths)
}

/// Start `opusdm-viewer` with `paths` (files and folders) as its arguments.
pub fn open_in_viewer(ui: &Rc<Ui>, paths: Vec<PathBuf>) {
    let paths = match viewer_targets(paths) {
        Ok(paths) => paths,
        Err(msg) => return crate::actions::flash(ui, &msg),
    };
    if let Err(e) = crate::image_viewer::launch(&paths) {
        crate::actions::flash(ui, &format!("cannot start {}: {e}", crate::image_viewer::VIEWER_BIN));
    }
}

/// Commands offered by the menu on an empty area of the file list.
pub const EMPTY_AREA_COMMANDS: [CommandId; 2] = [CommandId::Mkdir, CommandId::NewFile];

/// CSS names of the column view parts that have a menu of their own (rows)
/// or none at all (the header).
const NOT_EMPTY_CSS_NAMES: [&str; 2] = ["row", "header"];

/// True when none of `css_names` (the picked widget and its ancestors up to
/// the column view) is a row or the header, that is, the click landed on
/// the empty area below the rows.
pub fn is_empty_area<'a>(mut css_names: impl Iterator<Item = &'a str>) -> bool {
    !css_names.any(|name| NOT_EMPTY_CSS_NAMES.contains(&name))
}

/// Show the menu of an empty area of the file list: create a folder or a file
/// in the current folder.
pub fn show_empty_area_menu(ui: &Rc<Ui>, anchor: &impl IsA<gtk::Widget>, x: f64, y: f64) {
    let menu = Menu::new(ui, anchor, x, y);
    for cmd in EMPTY_AREA_COMMANDS {
        let Some(meta) = cmd.meta() else { continue };
        menu.item(meta.title, move |ui| crate::palette::execute_command(ui, cmd.clone()));
    }
    menu.popup();
}

/// Right click on the column view outside any row: the empty area menu.
/// The cells claim their own clicks, so this only sees the rest.
pub fn attach_empty_area(ui: &Rc<Ui>, column_view: &gtk::ColumnView) {
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let weak = Rc::downgrade(ui);
    click.connect_pressed(move |gesture, n_press, x, y| {
        if n_press != 1 { return; }
        let Some(ui) = weak.upgrade() else { return };
        let Some(view) = gesture.widget() else { return };
        let Some(picked) = view.pick(x, y, gtk::PickFlags::DEFAULT) else { return };
        let names: Vec<String> = std::iter::successors(Some(picked), |w| w.parent())
            .take_while(|w| w != &view)
            .map(|w| w.css_name().to_string())
            .collect();
        if !is_empty_area(names.iter().map(String::as_str)) { return; }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        show_empty_area_menu(&ui, &view, x, y);
    });
    column_view.add_controller(click);
}

/// The Lister popover as the sink of the shared entry menu.
impl opusdm_fileactions::MenuSink for Menu {
    fn item(&self, title: &str, action: Box<dyn Fn()>) {
        Menu::item(self, title, move |_| action());
    }

    fn separator(&self) {
        Menu::separator(self);
    }
}

/// Show the context menu for a row: the entry menu shared with the Desktop
/// (`opusdm_fileactions::menu`), with this Lister as its host.
pub fn show_context_menu(
    ui: &Rc<Ui>,
    anchor: &(impl IsA<gtk::Widget> + Clone),
    row: &Row,
    target_path: PathBuf,
    x: f64,
    y: f64,
) {
    let menu = Menu::new(ui, anchor, x, y);
    let target = opusdm_fileactions::MenuTarget {
        path: target_path,
        name: row.name.clone(),
        is_dir: row.is_dir,
        size: row.size,
        modified: row.modified_unix,
        elsewhere: false,
    };
    opusdm_fileactions::menu::fill_entry_menu(&menu, &crate::file_host::ListerHost::rc(ui), target);
    menu.popup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_area_offers_make_dir_and_new_file() {
        assert_eq!(EMPTY_AREA_COMMANDS, [CommandId::Mkdir, CommandId::NewFile]);
        assert!(EMPTY_AREA_COMMANDS.iter().all(|c| c.meta().is_some()));
    }

    #[test]
    fn the_viewer_gets_every_local_file_and_folder_in_order() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let paths = vec![file.clone(), td.path().to_path_buf()];
        assert_eq!(viewer_targets(paths.clone()).unwrap(), paths);
    }

    #[test]
    fn the_viewer_refuses_an_empty_selection_or_a_path_not_on_disk() {
        assert_eq!(viewer_targets(Vec::new()).unwrap_err(), "nothing selected");
        let td = tempfile::tempdir().unwrap();
        let inside = td.path().join("a.zip#dir/file");
        let err = viewer_targets(vec![td.path().to_path_buf(), inside]).unwrap_err();
        assert!(err.contains("a.zip#dir/file") && err.contains("not a local file"), "{err}");
    }

    #[test]
    fn a_click_below_the_rows_is_an_empty_area() {
        assert!(is_empty_area(["listview"].into_iter()));
        assert!(is_empty_area(std::iter::empty()));
    }

    #[test]
    fn a_click_on_a_row_or_the_header_is_not_an_empty_area() {
        assert!(!is_empty_area(["cell", "row", "listview"].into_iter()));
        assert!(!is_empty_area(["button", "header"].into_iter()));
    }
}
