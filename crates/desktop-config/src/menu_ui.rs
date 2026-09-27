//! The Menu tab: editing the custom application menu (`menu.json`) - a tree
//! of folders and apps, edited with per-row buttons. Empty (the default)
//! means "no custom menu yet"; the panel then keeps showing its
//! auto-generated, category-based menu.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_dock::apps;
use opusdm_ipc::menu::{self, MenuItem};
use std::cell::RefCell;
use std::rc::Rc;

/// A section heading label styled like the other sections.
fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("heading");
    l
}

/// The Menu tab: a tree editor over `menu.json`'s items, plus "Add
/// Folder"/"Add App..." buttons that act on the root.
pub struct MenuSection {
    pub root: gtk::Box,
    inner: Rc<MenuInner>,
}

struct MenuInner {
    items: RefCell<Vec<MenuItem>>,
    tree: gtk::Box,
    window: gtk::Window,
}

impl MenuSection {
    /// Build the section seeded with the current menu tree; `window` parents
    /// the "Add App..." file dialog and the folder-name requester.
    pub fn new(window: &impl IsA<gtk::Window>, items: Vec<MenuItem>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading("MENU"));

        let hint = gtk::Label::new(Some(
            "Empty: the menu shows every installed app, grouped by category. \
             Add a folder or an app below to build a custom menu instead.",
        ));
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        hint.add_css_class("dim-label");
        root.append(&hint);

        let tree = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.append(&tree);

        let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let add_folder = gtk::Button::with_label("Add Folder...");
        let add_app = gtk::Button::with_label("Add App...");
        add_row.append(&add_folder);
        add_row.append(&add_app);
        root.append(&add_row);

        let inner = Rc::new(MenuInner {
            items: RefCell::new(items),
            tree,
            window: window.clone().upcast::<gtk::Window>(),
        });
        inner.rebuild();

        {
            let inner = inner.clone();
            add_folder.connect_clicked(move |_| inner.prompt_add_folder(&[]));
        }
        {
            let inner = inner.clone();
            add_app.connect_clicked(move |_| inner.choose_app(&[]));
        }

        Self { root, inner }
    }

    /// The edited menu tree, in order.
    pub fn items(&self) -> Vec<MenuItem> {
        self.inner.items.borrow().clone()
    }
}

impl MenuInner {
    /// Repaint the whole tree from `items`.
    fn rebuild(self: &Rc<Self>) {
        while let Some(row) = self.tree.first_child() {
            self.tree.remove(&row);
        }
        let items = self.items.borrow().clone();
        self.build_level(&items, &[], &self.tree);
    }

    /// Render one level of the tree into `into`, each row's `path` locating
    /// it inside `self.items` for the button callbacks.
    fn build_level(self: &Rc<Self>, items: &[MenuItem], path: &[usize], into: &gtk::Box) {
        for (i, item) in items.iter().enumerate() {
            let mut child_path = path.to_vec();
            child_path.push(i);
            into.append(&self.build_row(item, &child_path));
        }
    }

    /// One tree row: icon/name (or folder name), then the buttons that apply
    /// to it - move up/down, remove, and, for a folder, add-into and rename.
    fn build_row(self: &Rc<Self>, item: &MenuItem, path: &[usize]) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        match item {
            MenuItem::App { id } => {
                let name = apps::resolve(&[id.clone()])
                    .first()
                    .map(|e| e.info.name().to_string())
                    .unwrap_or_else(|| format!("{id} (not installed)"));
                let label = gtk::Label::new(Some(&name));
                label.set_xalign(0.0);
                label.set_hexpand(true);
                header.append(&label);
            }
            MenuItem::Folder { name, .. } => {
                let label = gtk::Label::new(Some(name));
                label.set_xalign(0.0);
                label.set_hexpand(true);
                label.add_css_class("heading");
                header.append(&label);

                let add_folder = gtk::Button::from_icon_name("folder-new-symbolic");
                add_folder.add_css_class("flat");
                add_folder.set_tooltip_text(Some("Add Folder"));
                let this = self.clone();
                let folder_path = path.to_vec();
                add_folder.connect_clicked(move |_| this.prompt_add_folder(&folder_path));
                header.append(&add_folder);

                let add_app = gtk::Button::from_icon_name("list-add-symbolic");
                add_app.add_css_class("flat");
                add_app.set_tooltip_text(Some("Add App"));
                let this = self.clone();
                let folder_path = path.to_vec();
                add_app.connect_clicked(move |_| this.choose_app(&folder_path));
                header.append(&add_app);

                let rename = gtk::Button::from_icon_name("document-edit-symbolic");
                rename.add_css_class("flat");
                rename.set_tooltip_text(Some("Rename"));
                let this = self.clone();
                let folder_path = path.to_vec();
                let current = name.clone();
                rename.connect_clicked(move |_| this.prompt_rename(&folder_path, &current));
                header.append(&rename);
            }
        }

        for (icon, edit) in Self::move_and_remove(path) {
            let button = gtk::Button::from_icon_name(icon);
            button.add_css_class("flat");
            let this = self.clone();
            button.connect_clicked(move |_| {
                edit(&mut this.items.borrow_mut());
                let this = this.clone();
                // Rebuilding destroys the clicked button; wait for the click
                // to finish.
                gtk::glib::idle_add_local_once(move || this.rebuild());
            });
            header.append(&button);
        }
        row.append(&header);

        if let MenuItem::Folder { items, .. } = item {
            let nested = gtk::Box::new(gtk::Orientation::Vertical, 2);
            nested.set_margin_start(18);
            self.build_level(items, path, &nested);
            row.append(&nested);
        }
        row
    }

    /// The move-up/move-down/remove buttons shared by every row, bound to
    /// `path`.
    fn move_and_remove(path: &[usize]) -> [(&'static str, Box<dyn Fn(&mut Vec<MenuItem>)>); 3] {
        let p = path.to_vec();
        let up = p.clone();
        let down = p.clone();
        let rm = p;
        [
            ("go-up-symbolic", Box::new(move |items: &mut Vec<MenuItem>| menu::move_up(items, &up))),
            ("go-down-symbolic", Box::new(move |items: &mut Vec<MenuItem>| menu::move_down(items, &down))),
            ("list-remove-symbolic", Box::new(move |items: &mut Vec<MenuItem>| menu::remove_at(items, &rm))),
        ]
    }

    /// Ask for a folder name and append it inside the folder at
    /// `folder_path` (empty = root).
    fn prompt_add_folder(self: &Rc<Self>, folder_path: &[usize]) {
        let this = self.clone();
        let folder_path = folder_path.to_vec();
        opusdm_ui::requester_entry(
            &self.window,
            "Add Folder",
            "",
            &[("Add", true), ("Cancel", false)],
            move |ok, text| {
                if !ok || text.trim().is_empty() {
                    return;
                }
                menu::add_folder(&mut this.items.borrow_mut(), &folder_path, text.trim());
                this.rebuild();
            },
        );
    }

    /// Rename the folder at `path`.
    fn prompt_rename(self: &Rc<Self>, path: &[usize], current: &str) {
        let this = self.clone();
        let path = path.to_vec();
        opusdm_ui::requester_entry(
            &self.window,
            "Rename Folder",
            current,
            &[("Rename", true), ("Cancel", false)],
            move |ok, text| {
                if !ok || text.trim().is_empty() {
                    return;
                }
                menu::rename_folder(&mut this.items.borrow_mut(), &path, text.trim());
                this.rebuild();
            },
        );
    }

    /// Let the user pick a `.desktop` file and append it inside the folder
    /// at `folder_path` (empty = root).
    fn choose_app(self: &Rc<Self>, folder_path: &[usize]) {
        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.desktop");
        filter.set_name(Some("Applications"));
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let mut builder = gtk::FileDialog::builder().title("Add Application").filters(&filters);
        if let Some(home) = dirs::home_dir() {
            builder = builder.initial_folder(&gio::File::for_path(home));
        }
        let dialog = builder.build();
        let this = self.clone();
        let folder_path = folder_path.to_vec();
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Some(id) = result.ok().and_then(|f| f.path()).and_then(|p| apps::id_for_desktop_file(&p)) else {
                return;
            };
            menu::add_app(&mut this.items.borrow_mut(), &folder_path, &id);
            this.rebuild();
        });
    }
}
