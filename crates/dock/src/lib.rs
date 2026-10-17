//! opusdm-dock: a launcher-only dock for the OpusDM desktop.
//!
//! A single opaque window with a row of application icons, docked to a screen
//! edge. It reserves no screen space: maximised windows cover it. See
//! `docs/dock.md`.

use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::{DockConfig, DockPosition, PanelPosition};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

pub mod list;
mod placement;

/// The pinned apps and the default menu live in `opusdm-apps`; re-exported
/// so `opusdm_dock::apps` and `opusdm_dock::menu_defaults` keep working.
pub use opusdm_apps::{apps, menu_defaults};
mod window;

use apps::AppEntry;
pub use placement::Placement;

/// How long the busy cursor stays after a launch, in milliseconds.
const BUSY_CURSOR_MS: u64 = 2000;

/// Everything that decides where the dock sits and how big its icons are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockLayout {
    /// Requested edge; the dock keeps clear of `panel` on its own.
    pub position: DockPosition,
    /// Launcher icon size in pixels.
    pub icon_size: i32,
    /// The panel's edge, which the dock must not share.
    pub panel: PanelPosition,
    /// Height of the panel strip in pixels.
    pub panel_height: i32,
}

/// Mutable dock state, changed by settings updates, monitor changes and
/// edits to `dock.json`.
struct State {
    layout: DockLayout,
    /// Primary monitor rect `(x, y, w, h)`.
    monitor: (i32, i32, i32, i32),
    entries: Vec<AppEntry>,
}

/// The dock window and the state it renders from.
struct Dock {
    window: gtk::ApplicationWindow,
    row: gtk::Box,
    state: RefCell<State>,
    /// Keeps the `dock.json` watch alive.
    config_monitor: RefCell<Option<gio::FileMonitor>>,
}

/// Title of the dock window; the hub uses it to hide and show the dock.
pub const WINDOW_TITLE: &str = "OpusDM Dock";

/// Build the dock on the primary monitor and present it. The window is owned
/// by `app`, so it lives as long as the application does. The returned
/// function applies a new layout live; call it when the desktop settings
/// change.
pub fn attach(app: &gtk::Application, layout: DockLayout) -> Rc<dyn Fn(DockLayout)> {
    install_css();

    let entries = apps::resolve(&initial_app_ids());
    let monitor = primary_monitor();
    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title(WINDOW_TITLE)
        .decorated(false)
        // Resizable on the GTK side so the window follows the X geometry we
        // pin; the WM is kept from resizing it by the min = max size hints.
        .resizable(true)
        .build();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("opusdm-dock");
    set_margins(&row, placement::DOCK_PAD);
    win.set_child(Some(&row));

    let dock = Rc::new(Dock {
        window: win.clone(),
        row,
        state: RefCell::new(State { layout, monitor, entries }),
        config_monitor: RefCell::new(None),
    });

    // Wayland: a bottom layer surface on its edge (a no-op on X11).
    opusdm_ui::layer::init(&win, &window::layer_spec(dock.placement()));
    {
        let dock = dock.clone();
        win.connect_realize(move |_| dock.apply_geometry());
    }
    {
        let dock = dock.clone();
        win.connect_map(move |w| {
            dock.apply_geometry();
            window::set_below(w);
            // The window manager adopts the window and runs its placement
            // policy shortly after map, and drops state messages sent before
            // it has adopted it: re-pin the geometry and re-send the states a
            // few times over the first second.
            let dock = dock.clone();
            opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || {
                dock.pin();
                window::set_below(&dock.window);
            });
        });
    }

    dock.install_window_controllers();
    dock.rebuild_items();
    dock.watch_config();
    dock.watch_monitors();
    win.present();

    Rc::new(move |new_layout| dock.set_layout(new_layout))
}

/// The pinned app ids to start with: `dock.json`, or the default set written
/// out as `dock.json` so the Settings tool and the dock start from one list.
fn initial_app_ids() -> Vec<String> {
    if let Some(cfg) = opusdm_ipc::dock::load_dock_config() {
        return cfg.apps;
    }
    let apps = apps::default_app_ids();
    if let Err(e) = opusdm_ipc::dock::save_dock_config(&DockConfig { apps: apps.clone() }) {
        opusdm_ipc::err_log!("dock", "cannot write dock.json: {e}");
    }
    apps
}

/// Rect `(x, y, w, h)` of the primary monitor, or a 1280x800 fallback.
fn primary_monitor() -> (i32, i32, i32, i32) {
    gtk::gdk::Display::default()
        .as_ref()
        .map(opusdm_ui::list_monitors)
        .and_then(|rects| rects.first().copied())
        .unwrap_or((0, 0, 1280, 800))
}

impl Dock {
    /// The current placement, from the layout and the primary monitor.
    fn placement(&self) -> Placement {
        let s = self.state.borrow();
        Placement {
            position: s.layout.position,
            icon_size: s.layout.icon_size,
            panel: s.layout.panel,
            panel_height: s.layout.panel_height,
            mon_x: s.monitor.0,
            mon_y: s.monitor.1,
            mon_w: s.monitor.2,
            mon_h: s.monitor.3,
        }
    }

    /// Apply the DOCK hints and geometry to the window, if it is realized.
    fn apply_geometry(&self) {
        if self.window.surface().is_none() {
            return;
        }
        let count = self.state.borrow().entries.len();
        window::make_dock_window(&self.window, self.placement(), count);
    }

    /// Re-assert position and size only.
    fn pin(&self) {
        if self.window.surface().is_none() {
            return;
        }
        let count = self.state.borrow().entries.len();
        window::pin_geometry(&self.window, self.placement(), count);
    }

    /// Rebuild the icon row from the current entries and resize the window.
    fn rebuild_items(self: &Rc<Self>) {
        while let Some(child) = self.row.first_child() {
            self.row.remove(&child);
        }
        let place = self.placement();
        self.row.set_orientation(if placement::is_horizontal(place.position()) {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        });
        let entries = self.state.borrow().entries.clone();
        for entry in &entries {
            self.row.append(&self.build_item(entry, place.icon_size));
        }
        let (_, _, w, h) = place.rect(entries.len());
        self.window.set_default_size(w, h);
        self.apply_geometry();
        self.pin();
    }

    /// One launcher slot: the app icon, its name as tooltip, click to launch.
    fn build_item(self: &Rc<Self>, entry: &AppEntry, icon_size: i32) -> gtk::Image {
        let image = match entry.info.icon() {
            Some(icon) => gtk::Image::from_gicon(&icon),
            None => gtk::Image::from_icon_name("application-x-executable"),
        };
        image.add_css_class("opusdm-dock-item");
        image.set_pixel_size(icon_size);
        set_margins(&image, placement::ITEM_PAD);
        image.set_tooltip_text(Some(entry.info.name().as_str()));

        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        let dock = self.clone();
        let info = entry.info.clone();
        click.connect_released(move |_, _, _, _| dock.launch(&info));
        image.add_controller(click);

        let menu_click = gtk::GestureClick::new();
        menu_click.set_button(gdk::BUTTON_SECONDARY);
        let dock = self.clone();
        let menu_entry = entry.clone();
        let target = image.clone();
        menu_click.connect_pressed(move |gesture, _, _, _| {
            // Claim the click so the dock background menu does not open too.
            gesture.set_state(gtk::EventSequenceState::Claimed);
            dock.show_item_menu(&target, &menu_entry);
        });
        image.add_controller(menu_click);

        self.install_drag_source(&image, &entry.id);
        image
    }

    /// Make `image` draggable: within the dock it reorders, and dropping it
    /// on nothing removes the app.
    fn install_drag_source(self: &Rc<Self>, image: &gtk::Image, id: &str) {
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::MOVE);
        let payload = id.to_string();
        source.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&payload.to_value()))
        });
        let dragged = image.clone();
        source.connect_drag_begin(move |src, _| {
            let paintable = gtk::WidgetPaintable::new(Some(&dragged));
            src.set_icon(Some(&paintable), 0, 0);
        });
        let dock = self.clone();
        let removed = id.to_string();
        source.connect_drag_cancel(move |_, _, reason| {
            if reason != gdk::DragCancelReason::NoTarget {
                return false;
            }
            // Rebuilding the row destroys the drag source; do it once the
            // drag machinery has finished with it.
            let dock = dock.clone();
            let removed = removed.clone();
            gtk::glib::idle_add_local_once(move || dock.remove_app(&removed));
            true
        });
        image.add_controller(source);
    }

    /// Launch `info` and show the busy cursor for a moment.
    fn launch(&self, info: &gio::DesktopAppInfo) {
        apps::launch(info);
        self.window.set_cursor_from_name(Some("progress"));
        let window = self.window.clone();
        gtk::glib::timeout_add_local_once(Duration::from_millis(BUSY_CURSOR_MS), move || {
            window.set_cursor(None);
        });
    }

    /// Drop targets (reorder, add a `.desktop` file) and the background menu.
    fn install_window_controllers(self: &Rc<Self>) {
        // Reorder: an app id dragged from one of the dock's own icons. Only
        // MOVE is offered, so nothing outside the dock can be affected.
        let reorder = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
        let dock = self.clone();
        reorder.connect_drop(move |_, value, x, y| {
            let Ok(id) = value.get::<String>() else { return false };
            let index = dock.placement().insert_index(dock.state.borrow().entries.len(), x, y);
            let mut ids = apps::ids(&dock.state.borrow().entries);
            list::insert_at(&mut ids, &id, index);
            dock.commit(ids);
            true
        });
        self.window.add_controller(reorder);

        // Add: `.desktop` files dragged in from a file manager or the
        // desktop. Only COPY is offered, so the source never deletes them.
        let add = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let dock = self.clone();
        add.connect_drop(move |_, value, x, y| {
            let Ok(files) = value.get::<gdk::FileList>() else { return false };
            let new_ids: Vec<String> = files
                .files()
                .iter()
                .filter_map(|f| f.path())
                .filter_map(|p| apps::id_for_desktop_file(&p))
                .collect();
            if new_ids.is_empty() {
                return false;
            }
            let first = dock.placement().insert_index(dock.state.borrow().entries.len(), x, y);
            let mut ids = apps::ids(&dock.state.borrow().entries);
            for (offset, id) in new_ids.iter().enumerate() {
                list::insert_at(&mut ids, id, first + offset);
            }
            dock.commit(ids);
            true
        });
        self.window.add_controller(add);

        let menu_click = gtk::GestureClick::new();
        menu_click.set_button(gdk::BUTTON_SECONDARY);
        let dock = self.clone();
        menu_click.connect_pressed(move |_, _, x, y| dock.show_background_menu(x, y));
        self.window.add_controller(menu_click);
    }

    /// Make `ids` the pinned list and persist it. The file monitor's echo of
    /// this write is a no-op (see [`Dock::set_app_ids`]).
    fn commit(self: &Rc<Self>, ids: Vec<String>) {
        self.set_app_ids(&ids);
        let apps = apps::ids(&self.state.borrow().entries);
        if let Err(e) = opusdm_ipc::dock::save_dock_config(&DockConfig { apps }) {
            opusdm_ipc::err_log!("dock", "cannot write dock.json: {e}");
        }
    }

    /// Unpin `id`.
    fn remove_app(self: &Rc<Self>, id: &str) {
        let mut ids = apps::ids(&self.state.borrow().entries);
        list::remove(&mut ids, id);
        self.commit(ids);
    }

    /// Popover direction that opens away from the dock's edge.
    fn menu_direction(&self) -> gtk::PositionType {
        match self.placement().position() {
            DockPosition::Bottom => gtk::PositionType::Top,
            DockPosition::Top => gtk::PositionType::Bottom,
            DockPosition::Left => gtk::PositionType::Right,
            DockPosition::Right => gtk::PositionType::Left,
        }
    }

    /// Show a popover of flat buttons on `parent`, one per `(label, action)`.
    /// Each button closes the popover, then runs its action.
    fn show_menu(&self, parent: &impl IsA<gtk::Widget>, at: Option<(f64, f64)>, items: Vec<(String, Box<dyn Fn()>)>) {
        let popover = gtk::Popover::new();
        popover.set_parent(parent);
        popover.set_position(self.menu_direction());
        popover.set_has_arrow(false);
        if let Some((x, y)) = at {
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        }
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for (label, action) in items {
            let button = gtk::Button::with_label(&label);
            button.add_css_class("flat");
            let popover = popover.clone();
            button.connect_clicked(move |_| {
                popover.popdown();
                action();
            });
            menu.append(&button);
        }
        popover.set_child(Some(&menu));
        // Unparent once closed, after GTK is done with the popover.
        popover.connect_closed(|p| {
            let p = p.clone();
            gtk::glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
    }

    /// Context menu of one icon: its desktop actions, then "Remove from Dock".
    fn show_item_menu(self: &Rc<Self>, image: &gtk::Image, entry: &AppEntry) {
        let mut items: Vec<(String, Box<dyn Fn()>)> = Vec::new();
        for action in entry.info.list_actions() {
            let info = entry.info.clone();
            let name = entry.info.action_name(&action).to_string();
            let action = action.to_string();
            items.push((name, Box::new(move || apps::launch_action(&info, &action))));
        }
        let dock = self.clone();
        let id = entry.id.clone();
        items.push((
            "Remove from Dock".to_string(),
            Box::new(move || {
                // After the popover has closed: removing rebuilds the row.
                let dock = dock.clone();
                let id = id.clone();
                gtk::glib::idle_add_local_once(move || dock.remove_app(&id));
            }),
        ));
        self.show_menu(image, None, items);
    }

    /// Context menu of the dock background: "Add Application...".
    fn show_background_menu(self: &Rc<Self>, x: f64, y: f64) {
        let dock = self.clone();
        let items: Vec<(String, Box<dyn Fn()>)> =
            vec![("Add Application...".to_string(), Box::new(move || dock.choose_app_to_add()))];
        self.show_menu(&self.window, Some((x, y)), items);
    }

    /// Let the user pick a `.desktop` file and pin it at the end.
    fn choose_app_to_add(self: &Rc<Self>) {
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
        let dock = self.clone();
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Some(id) = result.ok().and_then(|f| f.path()).and_then(|p| apps::id_for_desktop_file(&p)) else {
                return;
            };
            let mut ids = apps::ids(&dock.state.borrow().entries);
            let end = ids.len();
            list::insert_at(&mut ids, &id, end);
            dock.commit(ids);
        });
    }

    /// Apply a new layout live. No-op when nothing changed.
    fn set_layout(self: &Rc<Self>, layout: DockLayout) {
        if self.state.borrow().layout == layout {
            return;
        }
        self.state.borrow_mut().layout = layout;
        self.rebuild_items();
    }

    /// Replace the pinned apps with `ids`. No-op when the resolved list is
    /// the same, which also absorbs the echo of the dock's own writes.
    fn set_app_ids(self: &Rc<Self>, ids: &[String]) {
        let entries = apps::resolve(ids);
        if apps::ids(&entries) == apps::ids(&self.state.borrow().entries) {
            return;
        }
        self.state.borrow_mut().entries = entries;
        self.rebuild_items();
    }

    /// Reload `dock.json` whenever it changes on disk, so the Settings tool
    /// can edit the list while the dock runs.
    fn watch_config(self: &Rc<Self>) {
        let dir = opusdm_ipc::settings::config_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            opusdm_ipc::err_log!("dock", "cannot create {}: {e}", dir.display());
        }
        let file = gio::File::for_path(opusdm_ipc::dock::dock_config_path());
        let monitor = match file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(m) => m,
            Err(e) => {
                opusdm_ipc::err_log!("dock", "cannot watch dock.json: {e}");
                return;
            }
        };
        let dock = self.clone();
        monitor.connect_changed(move |_, _, _, event| {
            use gio::FileMonitorEvent::{ChangesDoneHint, Created, Deleted};
            if !matches!(event, ChangesDoneHint | Created | Deleted) {
                return;
            }
            let ids = opusdm_ipc::dock::load_dock_config()
                .map(|cfg| cfg.apps)
                .unwrap_or_else(apps::default_app_ids);
            dock.set_app_ids(&ids);
        });
        *self.config_monitor.borrow_mut() = Some(monitor);
    }

    /// Follow the primary monitor across hotplug and screen size changes,
    /// once per settled layout change.
    fn watch_monitors(self: &Rc<Self>) {
        let Some(display) = gtk::gdk::Display::default() else { return };
        let dock = self.clone();
        opusdm_ui::watch_layout(&display, move |rects| dock.set_monitor(rects));
    }

    /// Re-dock onto the first of `rects`, when it changed.
    fn set_monitor(self: &Rc<Self>, rects: Vec<(i32, i32, i32, i32)>) {
        let Some(&rect) = rects.first() else { return };
        if self.state.borrow().monitor == rect {
            return;
        }
        self.state.borrow_mut().monitor = rect;
        self.rebuild_items();
    }
}

/// Give `widget` the same `margin` on all four sides.
fn set_margins(widget: &impl IsA<gtk::Widget>, margin: i32) {
    widget.set_margin_start(margin);
    widget.set_margin_end(margin);
    widget.set_margin_top(margin);
    widget.set_margin_bottom(margin);
}

/// The dock stylesheet: opaque background, thin border, hover highlight.
fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&format!(
        "window {{ background-color: #2e3436; border: {border}px solid #555753; }}\n\
         .opusdm-dock-item:hover {{ background-color: #3c4043; border-radius: 6px; }}",
        border = placement::BORDER,
    ));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
