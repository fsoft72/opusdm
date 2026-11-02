//! The Dock tab sections of the settings window: which edge the dock uses,
//! how big its icons are, and which apps are pinned.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_dock::{apps, list};
use opusdm_ipc::{DockImageMode, DockPosition, DockStyle, PanelPosition};
use std::cell::RefCell;
use opusdm_ui::{clear_children, labeled_row, pick_file, section_heading, FileFilterSpec, FilePick, LabelWidth};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// An edit of the dock id list for one entry (move up, move down, remove).
type ListEdit = Box<dyn Fn(&mut Vec<String>, &str)>;

/// Where the Add Application dialog opens.
const SYSTEM_APPS_DIR: &str = "/usr/share/applications";

/// Icon sizes the drop-down offers, in pixels.
const ICON_SIZES: [i32; 3] = [32, 48, 64];

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
        root.append(&section_heading("DOCK"));

        let position_row = gtk::DropDown::new(None::<gio::ListModel>, gtk::Expression::NONE);
        let size_labels: Vec<String> = ICON_SIZES.iter().map(|s| format!("{s} px")).collect();
        let size_labels: Vec<&str> = size_labels.iter().map(String::as_str).collect();
        let size = gtk::DropDown::from_strings(&size_labels);
        let nearest = nearest_icon_size(icon_size);
        size.set_selected(ICON_SIZES.iter().position(|s| *s == nearest).unwrap_or(1) as u32);

        for (label, dropdown) in [("Position", &position_row), ("Icon size", &size)] {
            root.append(&labeled_row(label, LabelWidth::Pixels(90), 8, dropdown));
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
        root.append(&section_heading("DOCK APPS"));

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
        clear_children(&self.list);
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

        let buttons: [(&str, ListEdit); 3] = [
            ("go-up-symbolic", Box::new(|ids, id| {
                let at = ids.iter().position(|x| x == id).unwrap_or(0);
                list::insert_at(ids, id, at.saturating_sub(1));
            })),
            ("go-down-symbolic", Box::new(|ids, id| {
                let at = ids.iter().position(|x| x == id).unwrap_or(0);
                list::insert_at(ids, id, at + 2);
            })),
            ("list-remove-symbolic", Box::new(list::remove)),
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
        let pick = FilePick {
            title: "Add Application",
            filter: Some(FileFilterSpec { name: "Applications", patterns: &["*.desktop"], ..Default::default() }),
            initial_folder: Some(Path::new(SYSTEM_APPS_DIR)),
            ..Default::default()
        };
        let this = self.clone();
        pick_file(Some(window), &pick, move |path| {
            let Some(id) = apps::id_for_desktop_file(&path) else {
                opusdm_ipc::err_log!("opusdm-settings", "{} is not an installed application", path.display());
                return;
            };
            let end = this.ids.borrow().len();
            list::insert_at(&mut this.ids.borrow_mut(), &id, end);
            this.rebuild();
        });
    }
}

/// Image fill modes the drop-down offers, in drop-down order.
const IMAGE_MODES: [(DockImageMode, &str); 2] = [(DockImageMode::Fill, "Fill"), (DockImageMode::Tile, "Tile")];

/// Width of the labels of the look rows, in pixels (as the DOCK section).
const LOOK_LABEL_WIDTH: i32 = 90;

/// The dock look sections: shown or hidden, background opacity and image,
/// and the icon zoom on mouse over.
pub struct DockLookSection {
    pub root: gtk::Box,
    visible: gtk::CheckButton,
    opacity: gtk::Scale,
    image: Rc<RefCell<Option<PathBuf>>>,
    image_mode: gtk::DropDown,
    zoom: gtk::CheckButton,
    zoom_percent: gtk::Scale,
}

impl DockLookSection {
    /// Build the sections seeded with `visible` and `style`; `window` parents
    /// the image file dialog.
    pub fn new(window: &impl IsA<gtk::Window>, visible: bool, style: &DockStyle) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let visible_check = gtk::CheckButton::with_label("Show the dock");
        visible_check.set_active(visible);
        root.append(&visible_check);

        root.append(&section_heading("DOCK BACKGROUND"));
        let (lo, hi) = opusdm_ipc::DOCK_OPACITY_RANGE;
        let opacity = percent_scale(lo, hi, style.opacity);
        root.append(&labeled_row("Opacity", LabelWidth::Pixels(LOOK_LABEL_WIDTH), 8, &opacity));

        let image = Rc::new(RefCell::new(style.image.clone()));
        let image_label = gtk::Label::new(None);
        image_label.set_xalign(0.0);
        image_label.set_hexpand(true);
        image_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let choose = gtk::Button::with_label("Choose...");
        let clear = gtk::Button::with_label("Clear");
        let image_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        image_box.set_hexpand(true);
        image_box.append(&image_label);
        image_box.append(&choose);
        image_box.append(&clear);
        root.append(&labeled_row("Image", LabelWidth::Pixels(LOOK_LABEL_WIDTH), 8, &image_box));

        let mode_labels: Vec<&str> = IMAGE_MODES.iter().map(|(_, label)| *label).collect();
        let image_mode = gtk::DropDown::from_strings(&mode_labels);
        image_mode.set_selected(IMAGE_MODES.iter().position(|(m, _)| *m == style.image_mode).unwrap_or(0) as u32);
        root.append(&labeled_row("Image fit", LabelWidth::Pixels(LOOK_LABEL_WIDTH), 8, &image_mode));

        root.append(&section_heading("DOCK ZOOM"));
        let zoom = gtk::CheckButton::with_label("Magnify the icons under the pointer");
        zoom.set_active(style.zoom_enabled);
        root.append(&zoom);
        let (lo, hi) = opusdm_ipc::DOCK_ZOOM_RANGE;
        let zoom_percent = percent_scale(lo, hi, style.zoom_percent);
        root.append(&labeled_row("Zoom size", LabelWidth::Pixels(LOOK_LABEL_WIDTH), 8, &zoom_percent));

        let note = gtk::Label::new(Some("Opacity below 100% and the zoom need a compositor (picom); without one the dock stays opaque and still."));
        note.set_xalign(0.0);
        note.set_wrap(true);
        note.add_css_class("dim-label");
        root.append(&note);

        // The image row shows the chosen file; the fit only matters with one.
        let show_image = {
            let (image, image_label, clear, image_mode) = (image.clone(), image_label.clone(), clear.clone(), image_mode.clone());
            move || {
                let chosen = image.borrow().clone();
                let text = chosen.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
                image_label.set_text(text.as_deref().unwrap_or("(none)"));
                image_label.set_tooltip_text(chosen.as_deref().map(|p| p.to_string_lossy().into_owned()).as_deref());
                clear.set_sensitive(chosen.is_some());
                image_mode.set_sensitive(chosen.is_some());
            }
        };
        let show_image: Rc<dyn Fn()> = Rc::new(show_image);
        show_image();
        {
            let (image, show_image) = (image.clone(), show_image.clone());
            clear.connect_clicked(move |_| {
                *image.borrow_mut() = None;
                show_image();
            });
        }
        {
            let window = window.clone().upcast::<gtk::Window>();
            let (image, show_image) = (image.clone(), show_image.clone());
            choose.connect_clicked(move |_| {
                let current = image.borrow().clone();
                let pick = FilePick {
                    title: "Dock Background Image",
                    filter: Some(FileFilterSpec { name: "Images", patterns: &[], suffixes: &[], mime_types: &["image/*"] }),
                    initial_file: current.as_deref(),
                    initial_folder: None,
                };
                let (image, show_image) = (image.clone(), show_image.clone());
                pick_file(Some(&window), &pick, move |path| {
                    *image.borrow_mut() = Some(path);
                    show_image();
                });
            });
        }
        // The zoom size only matters while the zoom is on.
        zoom_percent.set_sensitive(style.zoom_enabled);
        {
            let zoom_percent = zoom_percent.clone();
            zoom.connect_toggled(move |c| zoom_percent.set_sensitive(c.is_active()));
        }

        Rc::new(Self { root, visible: visible_check, opacity, image, image_mode, zoom, zoom_percent })
    }

    /// Whether the dock is to be shown.
    pub fn visible(&self) -> bool {
        self.visible.is_active()
    }

    /// The chosen look.
    pub fn style(&self) -> DockStyle {
        DockStyle {
            opacity: self.opacity.value().round() as u32,
            image: self.image.borrow().clone(),
            image_mode: IMAGE_MODES.get(self.image_mode.selected() as usize).map(|(m, _)| *m).unwrap_or_default(),
            zoom_enabled: self.zoom.is_active(),
            zoom_percent: self.zoom_percent.value().round() as u32,
        }
        .sanitized()
    }
}

/// A horizontal percent slider from `lo` to `hi` seeded with `value`.
fn percent_scale(lo: u32, hi: u32, value: u32) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, lo as f64, hi as f64, 1.0);
    scale.set_value(value.clamp(lo, hi) as f64);
    scale.set_digits(0);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_hexpand(true);
    scale
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
        crate::gtk_test::run(|| {
            let s = DockSection::new(DockPosition::Left, 64, PanelPosition::Top);
            assert_eq!((s.position(), s.icon_size()), (DockPosition::Left, 64));
            // A bottom dock is not offered under a bottom panel: it falls back.
            let s = DockSection::new(DockPosition::Bottom, 48, PanelPosition::Bottom);
            assert_eq!(s.position(), DockPosition::Top);
            // Moving the panel onto the chosen edge moves the choice.
            let s = DockSection::new(DockPosition::Top, 48, PanelPosition::Bottom);
            s.set_panel(PanelPosition::Top);
            assert_eq!(s.position(), DockPosition::Bottom);
        });
    }

    #[test]
    fn look_section_reads_back_seeded_values() {
        crate::gtk_test::run(|| {
            let window = gtk::Window::new();
            let style = DockStyle { opacity: 35, image: Some("/tmp/wood.png".into()), image_mode: DockImageMode::Tile, zoom_enabled: true, zoom_percent: 180 };
            let s = DockLookSection::new(&window, false, &style);
            assert!(!s.visible());
            assert_eq!(s.style(), style);
            let s = DockLookSection::new(&window, true, &DockStyle::default());
            assert!(s.visible());
            assert_eq!(s.style(), DockStyle::default());
        });
    }
}
