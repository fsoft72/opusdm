//! The Dock tab sections of the settings window: which edge the dock uses,
//! how big its icons are, and which apps are pinned.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_dock::{apps, list};
use opusdm_ipc::{DockPosition, PanelPosition};
use std::cell::RefCell;
use std::rc::Rc;

/// Icon sizes the drop-down offers, in pixels.
const ICON_SIZES: [i32; 3] = [32, 48, 64];

/// A section heading label styled like the other sections.
fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("heading");
    l
}

/// The dock edges the panel leaves free: every edge except the panel's own.
pub fn available_positions(panel: PanelPosition) -> Vec<DockPosition> {
    [DockPosition::Bottom, DockPosition::Top, DockPosition::Left, DockPosition::Right]
        .into_iter()
        .filter(|p| p.avoiding(panel) == *p)
        .collect()
}

/// Human name of a dock edge, as the drop-down shows it.
fn position_label(position: DockPosition) -> &'static str {
    match position {
        DockPosition::Bottom => "Bottom",
        DockPosition::Top => "Top",
        DockPosition::Left => "Left",
        DockPosition::Right => "Right",
    }
}

/// The entry of [`ICON_SIZES`] closest to `size`.
fn nearest_icon_size(size: i32) -> i32 {
    ICON_SIZES.into_iter().min_by_key(|s| (s - size).abs()).unwrap_or(48)
}

/// The DOCK section: screen edge and icon size.
pub struct DockSection {
    pub root: gtk::Box,
    position: gtk::DropDown,
    /// The edges `position` currently offers, in drop-down order.
    positions: RefCell<Vec<DockPosition>>,
    size: gtk::DropDown,
}

impl DockSection {
    /// Build the section seeded with the current dock settings; `panel` is
    /// the panel edge the dock must keep clear of.
    pub fn new(position: DockPosition, icon_size: i32, panel: PanelPosition) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading("DOCK"));

        let position_row = gtk::DropDown::new(None::<gio::ListModel>, gtk::Expression::NONE);
        let size_labels: Vec<String> = ICON_SIZES.iter().map(|s| format!("{s} px")).collect();
        let size_labels: Vec<&str> = size_labels.iter().map(String::as_str).collect();
        let size = gtk::DropDown::from_strings(&size_labels);
        let nearest = nearest_icon_size(icon_size);
        size.set_selected(ICON_SIZES.iter().position(|s| *s == nearest).unwrap_or(1) as u32);

        for (label, dropdown) in [("Position", &position_row), ("Icon size", &size)] {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let name = gtk::Label::new(Some(label));
            name.set_xalign(0.0);
            name.set_size_request(90, -1);
            row.append(&name);
            row.append(dropdown);
            root.append(&row);
        }
        let section = Rc::new(Self {
            root,
            position: position_row,
            positions: RefCell::new(Vec::new()),
            size,
        });
        section.fill_positions(position, panel);
        section
    }

    /// Offer only the edges free of `panel`, selecting `wanted` (or the edge
    /// it falls back to).
    fn fill_positions(&self, wanted: DockPosition, panel: PanelPosition) {
        let available = available_positions(panel);
        let labels: Vec<&str> = available.iter().map(|p| position_label(*p)).collect();
        let selected = available.iter().position(|p| *p == wanted.avoiding(panel)).unwrap_or(0);
        self.position.set_model(Some(&gtk::StringList::new(&labels)));
        self.position.set_selected(selected as u32);
        *self.positions.borrow_mut() = available;
    }

    /// The panel moved to another edge: re-offer the free edges, keeping the
    /// current choice when it is still free.
    pub fn set_panel(&self, panel: PanelPosition) {
        self.fill_positions(self.position(), panel);
    }

    /// The chosen dock edge.
    pub fn position(&self) -> DockPosition {
        let positions = self.positions.borrow();
        positions.get(self.position.selected() as usize).copied().unwrap_or_default()
    }

    /// The chosen icon size in pixels.
    pub fn icon_size(&self) -> i32 {
        ICON_SIZES.get(self.size.selected() as usize).copied().unwrap_or(48)
    }
}

/// The pinned apps section: an ordered list with move, remove and add.
pub struct DockAppsSection {
    pub root: gtk::Box,
    inner: Rc<AppsInner>,
}

struct AppsInner {
    ids: RefCell<Vec<String>>,
    list: gtk::ListBox,
}

impl DockAppsSection {
    /// Build the section seeded with the pinned `ids`; `window` parents the
    /// "Add..." file dialog.
    pub fn new(window: &impl IsA<gtk::Window>, ids: Vec<String>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading("DOCK APPS"));

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        root.append(&list);

        let add = gtk::Button::with_label("Add...");
        add.set_halign(gtk::Align::Start);
        root.append(&add);

        let inner = Rc::new(AppsInner { ids: RefCell::new(ids), list });
        inner.rebuild();

        let window = window.clone().upcast::<gtk::Window>();
        let target = inner.clone();
        add.connect_clicked(move |_| target.choose_app(&window));
        Self { root, inner }
    }

    /// The pinned app ids, in order.
    pub fn apps(&self) -> Vec<String> {
        self.inner.ids.borrow().clone()
    }
}

impl AppsInner {
    /// Repaint the list from `ids`.
    fn rebuild(self: &Rc<Self>) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let ids = self.ids.borrow().clone();
        for id in &ids {
            self.list.append(&self.build_row(id));
        }
    }

    /// One list row: app name, then up, down and remove buttons.
    fn build_row(self: &Rc<Self>, id: &str) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let name = apps::resolve(&[id.to_string()])
            .first()
            .map(|e| e.info.name().to_string())
            .unwrap_or_else(|| format!("{id} (not installed)"));
        let label = gtk::Label::new(Some(&name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        row.append(&label);

        let buttons: [(&str, Box<dyn Fn(&mut Vec<String>, &str)>); 3] = [
            ("go-up-symbolic", Box::new(|ids, id| {
                let at = ids.iter().position(|x| x == id).unwrap_or(0);
                list::insert_at(ids, id, at.saturating_sub(1));
            })),
            ("go-down-symbolic", Box::new(|ids, id| {
                let at = ids.iter().position(|x| x == id).unwrap_or(0);
                list::insert_at(ids, id, at + 2);
            })),
            ("list-remove-symbolic", Box::new(|ids, id| list::remove(ids, id))),
        ];
        for (icon, edit) in buttons {
            let button = gtk::Button::from_icon_name(icon);
            button.add_css_class("flat");
            let this = self.clone();
            let id = id.to_string();
            button.connect_clicked(move |_| {
                edit(&mut this.ids.borrow_mut(), &id);
                // Rebuilding destroys the clicked button; wait for the click
                // to finish.
                let this = this.clone();
                gtk::glib::idle_add_local_once(move || this.rebuild());
            });
            row.append(&button);
        }
        row
    }

    /// Let the user pick a `.desktop` file and append it to the list.
    fn choose_app(self: &Rc<Self>, window: &gtk::Window) {
        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.desktop");
        filter.set_name(Some("Applications"));
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Add Application")
            .filters(&filters)
            .initial_folder(&gio::File::for_path("/usr/share/applications"))
            .build();
        let this = self.clone();
        dialog.open(Some(window), gio::Cancellable::NONE, move |result| {
            let Some(id) = result.ok().and_then(|f| f.path()).and_then(|p| apps::id_for_desktop_file(&p)) else {
                return;
            };
            let end = this.ids.borrow().len();
            list::insert_at(&mut this.ids.borrow_mut(), &id, end);
            this.rebuild();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_exclude_the_panel_edge() {
        use DockPosition::*;
        assert_eq!(available_positions(PanelPosition::Top), vec![Bottom, Left, Right]);
        assert_eq!(available_positions(PanelPosition::Bottom), vec![Top, Left, Right]);
    }

    #[test]
    fn icon_size_snaps_to_the_nearest_offered_one() {
        assert_eq!(nearest_icon_size(48), 48);
        assert_eq!(nearest_icon_size(40), 32);
        assert_eq!(nearest_icon_size(200), 64);
        assert_eq!(nearest_icon_size(1), 32);
    }

    #[test]
    fn section_reads_back_seeded_values_and_follows_the_panel() {
        if gtk::init().is_err() {
            return;
        }
        let s = DockSection::new(DockPosition::Left, 64, PanelPosition::Top);
        assert_eq!((s.position(), s.icon_size()), (DockPosition::Left, 64));
        // A bottom dock is not offered under a bottom panel: it falls back.
        let s = DockSection::new(DockPosition::Bottom, 48, PanelPosition::Bottom);
        assert_eq!(s.position(), DockPosition::Top);
        // Moving the panel onto the chosen edge moves the choice.
        let s = DockSection::new(DockPosition::Top, 48, PanelPosition::Bottom);
        s.set_panel(PanelPosition::Top);
        assert_eq!(s.position(), DockPosition::Bottom);
    }
}
