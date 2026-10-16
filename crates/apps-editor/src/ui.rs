//! The Apps editor window: the menu tree on the left (drag a row to move it,
//! onto a folder to put it inside), the properties of the selected entry on
//! the right. Nothing reaches disk until Save.

use crate::files;
use gtk4 as gtk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::desktop_entry::{self, DesktopFields};
use opusdm_ipc::menu::{self, MenuItem};
use opusdm_ui::{clear_children, pick_file, section_heading, DirtyGuard, FileFilterSpec, FilePick};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Icon size of a tree row, in pixels.
const ROW_ICON_SIZE: i32 = 18;
/// Indentation of one tree level, in pixels.
const INDENT: i32 = 18;
/// Name given to a freshly created entry.
const NEW_ITEM_NAME: &str = "New Item";
/// Name suggested for a freshly created submenu.
const NEW_FOLDER_NAME: &str = "New Submenu";

/// The entries of the property pane.
struct Form {
    root: gtk::Box,
    /// The rows that only apply to applications (everything but the name).
    app_rows: Vec<gtk::Widget>,
    name: gtk::Entry,
    comment: gtk::Entry,
    exec: gtk::Entry,
    icon: gtk::Entry,
    path: gtk::Entry,
    terminal: gtk::CheckButton,
    preview: gtk::Image,
}

/// Everything the callbacks share.
struct Editor {
    window: gtk::ApplicationWindow,
    list: gtk::ListBox,
    form: Form,
    status: gtk::Label,
    items: RefCell<Vec<MenuItem>>,
    /// Fields of every app edited or created in this session, by id.
    edited: RefCell<HashMap<String, DesktopFields>>,
    /// Ids whose file must be written on Save.
    touched: RefCell<HashSet<String>>,
    /// Own entries removed from the menu, whose file is deleted on Save.
    to_delete: RefCell<Vec<String>>,
    /// Tree path of the row at each list index.
    rows: RefCell<Vec<Vec<usize>>>,
    /// The label of each row, to follow a name edit without a rebuild.
    labels: RefCell<Vec<gtk::Label>>,
    selected: RefCell<Option<Vec<usize>>>,
    dirty: Cell<bool>,
    /// True while the code (not the user) sets the form or the selection.
    quiet: Cell<bool>,
}

/// Open the editor window.
pub fn open(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Edit Apps")
        .default_width(920)
        .default_height(580)
        .build();

    let items = match opusdm_ipc::menu::load_menu_config() {
        Some(cfg) if !cfg.items.is_empty() => cfg.items,
        _ => opusdm_dock::menu_defaults::default_menu_items(),
    };

    let editor = Rc::new(Editor {
        window: window.clone(),
        list: gtk::ListBox::new(),
        form: _build_form(),
        status: gtk::Label::new(None),
        items: RefCell::new(items),
        edited: RefCell::default(),
        touched: RefCell::default(),
        to_delete: RefCell::default(),
        rows: RefCell::default(),
        labels: RefCell::default(),
        selected: RefCell::default(),
        dirty: Cell::new(false),
        quiet: Cell::new(false),
    });

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for set in [gtk::Widget::set_margin_top, gtk::Widget::set_margin_bottom, gtk::Widget::set_margin_start, gtk::Widget::set_margin_end] {
        set(outer.upcast_ref(), 10);
    }

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_vexpand(true);
    paned.set_position(380);
    paned.set_shrink_start_child(false);
    paned.set_start_child(Some(&editor.build_left()));
    paned.set_end_child(Some(&editor.form.root));
    outer.append(&paned);
    outer.append(&editor.build_bottom());
    window.set_child(Some(&outer));

    {
        let editor = editor.clone();
        DirtyGuard::attach(&window, "Discard the changes?", Some("The menu and the entries have unsaved changes."), move || editor.dirty.get(), || {});
    }

    editor.rebuild();
    editor.load_form();
    window.present();
}

/// The property pane, empty and insensitive until a row is selected.
fn _build_form() -> Form {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(10);
    root.set_hexpand(true);
    root.append(&section_heading("PROPERTIES"));

    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(10);
    let mut app_rows: Vec<gtk::Widget> = Vec::new();
    let mut add = |row: i32, label: &str, field: &gtk::Widget, app_only: bool| {
        let l = gtk::Label::new(Some(label));
        l.set_xalign(1.0);
        grid.attach(&l, 0, row, 1, 1);
        field.set_hexpand(true);
        grid.attach(field, 1, row, 1, 1);
        if app_only {
            app_rows.push(l.upcast());
            app_rows.push(field.clone());
        }
    };
    let name = gtk::Entry::new();
    let comment = gtk::Entry::new();
    let exec = gtk::Entry::new();
    let path = gtk::Entry::new();
    let icon = gtk::Entry::new();
    let terminal = gtk::CheckButton::with_label("Run in terminal");
    let preview = gtk::Image::new();
    preview.set_pixel_size(32);

    let icon_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    icon_row.append(&preview);
    icon.set_hexpand(true);
    icon.set_placeholder_text(Some("icon name or image path"));
    icon_row.append(&icon);
    let browse = gtk::Button::with_label("Browse...");
    icon_row.append(&browse);

    add(0, "Name", name.upcast_ref(), false);
    add(1, "Comment", comment.upcast_ref(), true);
    add(2, "Command", exec.upcast_ref(), true);
    add(3, "Icon", icon_row.upcast_ref(), true);
    add(4, "Work path", path.upcast_ref(), true);
    add(5, "", terminal.upcast_ref(), true);
    root.append(&grid);

    {
        let (icon, window_root) = (icon.clone(), root.clone());
        browse.connect_clicked(move |_| {
            let pick = FilePick {
                title: "Choose Icon",
                filter: Some(FileFilterSpec { name: "Images", mime_types: &["image/*"], ..Default::default() }),
                ..Default::default()
            };
            let parent = window_root.root().and_downcast::<gtk::Window>();
            let icon = icon.clone();
            pick_file(parent.as_ref(), &pick, move |path| icon.set_text(&path.to_string_lossy()));
        });
    }

    Form { root, app_rows, name, comment, exec, icon, path, terminal, preview }
}

/// `"0,2,1"` for a tree path, for the drag payload.
fn _encode(path: &[usize]) -> String {
    path.iter().map(usize::to_string).collect::<Vec<_>>().join(",")
}

/// Inverse of [`_encode`]; `None` for anything malformed.
fn _decode(text: &str) -> Option<Vec<usize>> {
    text.split(',').map(|p| p.parse().ok()).collect()
}

/// The entry at `path` in `items`.
fn _item_at<'a>(items: &'a [MenuItem], path: &[usize]) -> Option<&'a MenuItem> {
    let (&first, rest) = path.split_first()?;
    let item = items.get(first)?;
    if rest.is_empty() {
        return Some(item);
    }
    match item {
        MenuItem::Folder { items, .. } => _item_at(items, rest),
        MenuItem::App { .. } => None,
    }
}

/// Every row of the tree, depth first, as `(path, depth)`.
fn _flatten(items: &[MenuItem], prefix: &[usize], out: &mut Vec<(Vec<usize>, usize)>) {
    for (i, item) in items.iter().enumerate() {
        let mut path = prefix.to_vec();
        path.push(i);
        out.push((path.clone(), prefix.len()));
        if let MenuItem::Folder { items, .. } = item {
            _flatten(items, &path, out);
        }
    }
}

/// Every app id below `item` (itself included when it is an app).
fn _collect_ids(item: &MenuItem, out: &mut Vec<String>) {
    match item {
        MenuItem::App { id } => out.push(id.clone()),
        MenuItem::Folder { items, .. } => items.iter().for_each(|i| _collect_ids(i, out)),
    }
}

/// An image for an icon name or an absolute image path.
fn _icon_image(icon: &str, size: i32) -> gtk::Image {
    let image = if icon.starts_with('/') {
        gtk::Image::from_file(icon)
    } else if icon.is_empty() {
        gtk::Image::from_icon_name("application-x-executable")
    } else {
        gtk::Image::from_icon_name(icon)
    };
    image.set_pixel_size(size);
    image
}

impl Editor {
    /// The tree list with its toolbar.
    fn build_left(self: &Rc<Self>) -> gtk::Box {
        let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let button = |label: &str, tip: &str| {
            let b = gtk::Button::with_label(label);
            b.set_tooltip_text(Some(tip));
            bar.append(&b);
            b
        };
        let new_folder = button("New Submenu...", "Add a submenu next to the selection");
        let new_item = button("New Item", "Add an application next to the selection");
        let up = button("Up", "Move the selection up");
        let down = button("Down", "Move the selection down");
        let delete = button("Delete", "Remove the selection from the menu");
        let reset = button("Reset...", "Rebuild the menu from the installed apps");
        left.append(&bar);

        self.list.set_selection_mode(gtk::SelectionMode::Single);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&self.list));
        left.append(&scroller);

        {
            let this = self.clone();
            self.list.connect_row_selected(move |_, row| {
                if this.quiet.get() {
                    return;
                }
                let path = row.and_then(|r| this.rows.borrow().get(r.index() as usize).cloned());
                *this.selected.borrow_mut() = path;
                this.load_form();
            });
        }
        let this = self.clone();
        new_folder.connect_clicked(move |_| this.prompt_new_folder());
        let this = self.clone();
        new_item.connect_clicked(move |_| this.new_item());
        let this = self.clone();
        up.connect_clicked(move |_| this.shift(-1));
        let this = self.clone();
        down.connect_clicked(move |_| this.shift(1));
        let this = self.clone();
        delete.connect_clicked(move |_| this.prompt_delete());
        let this = self.clone();
        reset.connect_clicked(move |_| this.prompt_reset());
        left
    }

    /// Status line plus Save and Close.
    fn build_bottom(self: &Rc<Self>) -> gtk::Box {
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        self.status.set_xalign(0.0);
        self.status.set_hexpand(true);
        bottom.append(&self.status);
        let save = gtk::Button::with_label("Save");
        let close = gtk::Button::with_label("Close");
        bottom.append(&save);
        bottom.append(&close);
        let this = self.clone();
        save.connect_clicked(move |_| this.save());
        let this = self.clone();
        close.connect_clicked(move |_| this.window.close());
        self.wire_form();
        bottom
    }

    /// Connect the property entries, so a user edit lands in `edited`.
    fn wire_form(self: &Rc<Self>) {
        let f = &self.form;
        for entry in [&f.name, &f.comment, &f.exec, &f.icon, &f.path] {
            let this = self.clone();
            entry.connect_changed(move |_| this.form_edited());
        }
        let this = self.clone();
        f.terminal.connect_toggled(move |_| this.form_edited());
    }

    fn set_status(&self, text: &str) {
        self.status.set_text(text);
    }

    fn mark_dirty(&self) {
        self.dirty.set(true);
        self.set_status("Unsaved changes");
    }

    /// The name and icon a tree row shows for the app `id`.
    fn app_look(&self, id: &str) -> (String, gtk::Image) {
        if let Some(f) = self.edited.borrow().get(id) {
            return (f.name.clone(), _icon_image(&f.icon, ROW_ICON_SIZE));
        }
        match gio::DesktopAppInfo::new(id).or_else(|| gio::DesktopAppInfo::from_filename(id)) {
            Some(info) => {
                let image = match info.icon() {
                    Some(icon) => gtk::Image::from_gicon(&icon),
                    None => gtk::Image::from_icon_name("application-x-executable"),
                };
                image.set_pixel_size(ROW_ICON_SIZE);
                (info.name().to_string(), image)
            }
            None => (format!("{id} (not installed)"), _icon_image("", ROW_ICON_SIZE)),
        }
    }

    /// Repaint the whole tree, keeping the selection when its path still exists.
    fn rebuild(self: &Rc<Self>) {
        self.quiet.set(true);
        clear_children(&self.list);
        let mut flat = Vec::new();
        _flatten(&self.items.borrow(), &[], &mut flat);
        let mut rows = Vec::new();
        let mut labels = Vec::new();
        for (path, depth) in &flat {
            let items = self.items.borrow();
            let Some(item) = _item_at(&items, path) else { continue };
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            content.set_margin_start(*depth as i32 * INDENT + 4);
            content.set_margin_top(2);
            content.set_margin_bottom(2);
            let (text, image) = match item {
                MenuItem::App { id } => self.app_look(id),
                MenuItem::Folder { name, .. } => {
                    let image = gtk::Image::from_icon_name("folder-symbolic");
                    image.set_pixel_size(ROW_ICON_SIZE);
                    (name.clone(), image)
                }
            };
            content.append(&image);
            let label = gtk::Label::new(Some(&text));
            label.set_xalign(0.0);
            if matches!(item, MenuItem::Folder { .. }) {
                label.add_css_class("heading");
            }
            content.append(&label);
            self.attach_drag(&content, path);
            self.list.append(&content);
            rows.push(path.clone());
            labels.push(label);
        }
        *self.rows.borrow_mut() = rows;
        *self.labels.borrow_mut() = labels;
        let selected = self.selected.borrow().clone();
        let index = selected.and_then(|s| self.rows.borrow().iter().position(|r| *r == s));
        match index {
            Some(i) => self.list.select_row(self.list.row_at_index(i as i32).as_ref()),
            None => {
                self.list.unselect_all();
                *self.selected.borrow_mut() = None;
            }
        }
        self.quiet.set(false);
    }

    /// Dragging `row` carries its path; dropping a path on it moves that entry here.
    fn attach_drag(self: &Rc<Self>, row: &gtk::Box, path: &[usize]) {
        let drag = gtk::DragSource::new();
        drag.set_actions(gtk::gdk::DragAction::MOVE);
        let payload = _encode(path);
        drag.connect_prepare(move |_, _, _| Some(gtk::gdk::ContentProvider::for_value(&payload.to_value())));
        row.add_controller(drag);

        let drop = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
        let (this, target) = (self.clone(), path.to_vec());
        drop.connect_drop(move |_, value, _, _| {
            let Some(from) = value.get::<String>().ok().and_then(|s| _decode(&s)) else { return false };
            if !menu::move_to(&mut this.items.borrow_mut(), &from, &target) {
                return false;
            }
            this.mark_dirty();
            *this.selected.borrow_mut() = None;
            // Deferred: the rebuild destroys the row the drag is running on.
            let this = this.clone();
            glib::idle_add_local_once(move || {
                this.rebuild();
                this.load_form();
            });
            true
        });
        row.add_controller(drop);
    }

    /// The selected entry, if any.
    fn selected_item(&self) -> Option<(Vec<usize>, MenuItem)> {
        let path = self.selected.borrow().clone()?;
        let item = _item_at(&self.items.borrow(), &path)?.clone();
        Some((path, item))
    }

    /// Show the selected entry in the property pane.
    fn load_form(&self) {
        self.quiet.set(true);
        let f = &self.form;
        match self.selected_item() {
            None => {
                f.root.set_sensitive(false);
                for e in [&f.name, &f.comment, &f.exec, &f.icon, &f.path] {
                    e.set_text("");
                }
                f.terminal.set_active(false);
                f.preview.set_icon_name(Some("application-x-executable"));
            }
            Some((_, MenuItem::Folder { name, .. })) => {
                f.root.set_sensitive(true);
                f.app_rows.iter().for_each(|w| w.set_visible(false));
                f.name.set_text(&name);
            }
            Some((_, MenuItem::App { id })) => {
                f.root.set_sensitive(true);
                f.app_rows.iter().for_each(|w| w.set_visible(true));
                let fields = self
                    .edited
                    .borrow()
                    .get(&id)
                    .cloned()
                    .or_else(|| files::load_fields(&id))
                    .unwrap_or_default();
                f.name.set_text(&fields.name);
                f.comment.set_text(&fields.comment);
                f.exec.set_text(&fields.exec);
                f.icon.set_text(&fields.icon);
                f.path.set_text(&fields.path);
                f.terminal.set_active(fields.terminal);
                self.refresh_preview();
            }
        }
        self.quiet.set(false);
    }

    /// Redraw the icon preview from the icon entry.
    fn refresh_preview(&self) {
        let icon = self.form.icon.text();
        if icon.starts_with('/') {
            self.form.preview.set_from_file(Some(icon.as_str()));
        } else if icon.is_empty() {
            self.form.preview.set_icon_name(Some("application-x-executable"));
        } else {
            self.form.preview.set_icon_name(Some(icon.as_str()));
        }
    }

    /// A property entry changed: store it and follow the name in the tree.
    fn form_edited(&self) {
        if self.quiet.get() {
            return;
        }
        let Some((path, item)) = self.selected_item() else { return };
        let f = &self.form;
        let name = f.name.text().to_string();
        match item {
            MenuItem::Folder { .. } => {
                if name.trim().is_empty() {
                    return;
                }
                menu::rename_folder(&mut self.items.borrow_mut(), &path, name.trim());
            }
            MenuItem::App { id } => {
                let fields = DesktopFields {
                    name: name.clone(),
                    comment: f.comment.text().to_string(),
                    exec: f.exec.text().to_string(),
                    icon: f.icon.text().to_string(),
                    path: f.path.text().to_string(),
                    terminal: f.terminal.is_active(),
                };
                self.edited.borrow_mut().insert(id.clone(), fields);
                self.touched.borrow_mut().insert(id);
                self.refresh_preview();
            }
        }
        if let Some(label) = self.selected_index().and_then(|i| self.labels.borrow().get(i).cloned()) {
            label.set_text(&name);
        }
        self.mark_dirty();
    }

    /// List index of the selected row.
    fn selected_index(&self) -> Option<usize> {
        self.list.selected_row().map(|r| r.index() as usize)
    }

    /// The folder new entries go into: the selected folder, the parent of the
    /// selected app, else the root.
    fn container(&self) -> Vec<usize> {
        match self.selected_item() {
            Some((path, MenuItem::Folder { .. })) => path,
            Some((mut path, MenuItem::App { .. })) => {
                path.pop();
                path
            }
            None => Vec::new(),
        }
    }

    /// Select `path` and show it.
    fn select(self: &Rc<Self>, path: Vec<usize>) {
        *self.selected.borrow_mut() = Some(path);
        self.rebuild();
        self.load_form();
    }

    /// Add a blank application entry.
    fn new_item(self: &Rc<Self>) {
        let container = self.container();
        let id = desktop_entry::new_id(NEW_ITEM_NAME, |id| files::id_taken(id) || self.edited.borrow().contains_key(id));
        let fields = DesktopFields { name: NEW_ITEM_NAME.to_string(), ..Default::default() };
        self.edited.borrow_mut().insert(id.clone(), fields);
        self.touched.borrow_mut().insert(id.clone());
        let Some(len) = menu::children_len(&mut self.items.borrow_mut(), &container) else { return };
        menu::add_app(&mut self.items.borrow_mut(), &container, &id);
        self.mark_dirty();
        let mut path = container;
        path.push(len);
        self.select(path);
        self.form.name.grab_focus();
        self.form.name.select_region(0, -1);
    }

    /// Ask for a submenu name, then add it.
    fn prompt_new_folder(self: &Rc<Self>) {
        let this = self.clone();
        opusdm_ui::requester_entry(
            &self.window,
            "New Submenu",
            NEW_FOLDER_NAME,
            &[("Add", true), ("Cancel", false)],
            move |ok, text| {
                if !ok || text.trim().is_empty() {
                    return;
                }
                let container = this.container();
                let Some(len) = menu::children_len(&mut this.items.borrow_mut(), &container) else { return };
                menu::add_folder(&mut this.items.borrow_mut(), &container, text.trim());
                this.mark_dirty();
                let mut path = container;
                path.push(len);
                this.select(path);
            },
        );
    }

    /// Move the selection one place up (`-1`) or down (`1`) in its list.
    fn shift(self: &Rc<Self>, by: i32) {
        let Some((mut path, _)) = self.selected_item() else { return };
        {
            let mut items = self.items.borrow_mut();
            if by < 0 {
                menu::move_up(&mut items, &path);
            } else {
                menu::move_down(&mut items, &path);
            }
        }
        let last = path.len() - 1;
        let before = path[last];
        let siblings = {
            let parent = &path[..last];
            menu::children_len(&mut self.items.borrow_mut(), parent).unwrap_or(0)
        };
        path[last] = if by < 0 { before.saturating_sub(1) } else { (before + 1).min(siblings - 1) };
        self.mark_dirty();
        self.select(path);
    }

    /// Ask, then remove the selection from the menu.
    fn prompt_delete(self: &Rc<Self>) {
        let Some((path, item)) = self.selected_item() else { return };
        let (what, detail) = match &item {
            MenuItem::App { .. } => ("Remove this entry from the menu?", "An entry created by this editor is deleted on Save."),
            MenuItem::Folder { .. } => ("Remove this submenu and everything in it?", "Entries created by this editor are deleted on Save."),
        };
        let this = self.clone();
        opusdm_ui::requester(&self.window, "Request", what, Some(detail), &[("Remove", true), ("Cancel", false)], move |ok| {
            if !ok {
                return;
            }
            let mut ids = Vec::new();
            _collect_ids(&item, &mut ids);
            for id in ids {
                this.edited.borrow_mut().remove(&id);
                this.touched.borrow_mut().remove(&id);
                if id.starts_with(files::OWN_PREFIX) && !id.starts_with('/') {
                    this.to_delete.borrow_mut().push(id);
                }
            }
            menu::remove_at(&mut this.items.borrow_mut(), &path);
            *this.selected.borrow_mut() = None;
            this.mark_dirty();
            this.rebuild();
            this.load_form();
        });
    }

    /// Ask, then rebuild the menu from the installed apps.
    fn prompt_reset(self: &Rc<Self>) {
        let this = self.clone();
        opusdm_ui::requester(
            &self.window,
            "Request",
            "Reset the menu to defaults?",
            Some("This discards every custom submenu and entry order."),
            &[("Reset", true), ("Cancel", false)],
            move |ok| {
                if !ok {
                    return;
                }
                *this.items.borrow_mut() = opusdm_dock::menu_defaults::default_menu_items();
                *this.selected.borrow_mut() = None;
                this.mark_dirty();
                this.rebuild();
                this.load_form();
            },
        );
    }

    /// Write the touched `.desktop` files, delete the removed own ones and
    /// save `menu.json`.
    fn save(self: &Rc<Self>) {
        let mut errors = Vec::new();
        let touched: Vec<String> = self.touched.borrow().iter().cloned().collect();
        for id in &touched {
            let Some(fields) = self.edited.borrow().get(id).cloned() else { continue };
            if fields.exec.trim().is_empty() {
                errors.push(format!("{}: the command is empty", fields.name));
                continue;
            }
            match files::write_entry(id, &fields) {
                Ok(()) => {
                    self.touched.borrow_mut().remove(id);
                }
                Err(e) => errors.push(format!("{}: {e}", fields.name)),
            }
        }
        // A delete that failed stays in the list, so the next save retries it.
        let ids = std::mem::take(&mut *self.to_delete.borrow_mut());
        let (pending, delete_errors) = files::delete_entries(ids, files::delete_own_entry);
        *self.to_delete.borrow_mut() = pending;
        errors.extend(delete_errors);
        let cfg = opusdm_ipc::MenuConfig { items: self.items.borrow().clone() };
        if let Err(e) = opusdm_ipc::menu::save_menu_config(&cfg) {
            errors.push(format!("menu.json: {e}"));
        }
        if errors.is_empty() {
            self.dirty.set(false);
            self.set_status("Saved. The application menu picks it up the next time it opens.");
        } else {
            opusdm_ipc::err_log!("apps-editor", "save: {}", errors.join("; "));
            self.set_status(&format!("Not everything was saved: {}", errors.join("; ")));
        }
    }
}
