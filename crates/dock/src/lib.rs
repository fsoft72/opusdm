//! opusdm-dock: a launcher-only dock for the OpusDM desktop.
//!
//! A single window with a row of application icons on a strip, docked to a
//! screen edge. It reserves no screen space: maximised windows cover it. The
//! strip can be see-through or show an image, and the icons near the pointer
//! can grow (with a compositor). See `docs/dock.md`.

use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::{DockConfig, DockImageMode, DockPosition, DockStyle, PanelPosition};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

mod icon;
pub mod list;
mod placement;
mod zoom;

/// The pinned apps and the default menu live in `opusdm-apps`; re-exported
/// so `opusdm_dock::apps` and `opusdm_dock::menu_defaults` keep working.
pub use opusdm_apps::{apps, menu_defaults};
mod window;

use apps::AppEntry;
use icon::DockIcon;
pub use placement::Placement;
use zoom::{Extent, Zoom};

/// How long the busy cursor stays after a launch, in milliseconds.
const BUSY_CURSOR_MS: u64 = 2000;

/// Time constant of the zoom animation, in milliseconds: each frame every
/// icon covers `1 - e^(-dt/TAU)` of the way to its target size.
const ZOOM_TAU_MS: f64 = 70.0;

/// How often the dock checks whether a compositor runs, in milliseconds.
/// GTK does not report a compositor that stops or starts later on X11.
const COMPOSITOR_CHECK_MS: u64 = 2000;

/// An icon this close to its target scale snaps to it and stops animating.
const ZOOM_SETTLE: f64 = 0.002;

/// Whether the hub has hidden the desktop chrome ("Hide All"); see
/// [`set_hidden`].
static HIDDEN: AtomicBool = AtomicBool::new(false);

/// Whether the settings ask for the dock ([`DockLayout::visible`]).
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Everything that decides where the dock sits, how big its icons are and
/// how it looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockLayout {
    /// Requested edge; the dock keeps clear of `panel` on its own.
    pub position: DockPosition,
    /// Launcher icon size in pixels.
    pub icon_size: i32,
    /// The panel's edge, which the dock must not share.
    pub panel: PanelPosition,
    /// Height of the panel strip in pixels.
    pub panel_height: i32,
    /// Whether the dock is shown at all.
    pub visible: bool,
    /// Background and zoom.
    pub style: DockStyle,
}

impl DockLayout {
    /// The layout with `visible` left out, to tell a show or hide apart
    /// from a change that needs the icons rebuilt.
    fn shape(&self) -> DockLayout {
        DockLayout { visible: true, ..self.clone() }
    }
}

/// Tell the dock whether the hub has hidden the desktop chrome. The hub
/// calls this before showing or hiding the dock window with
/// `set_visible(is_shown())`.
pub fn set_hidden(hidden: bool) {
    HIDDEN.store(hidden, Ordering::Relaxed);
}

/// Whether the dock window should be visible now: enabled in the settings
/// and not hidden by the hub.
pub fn is_shown() -> bool {
    ENABLED.load(Ordering::Relaxed) && !HIDDEN.load(Ordering::Relaxed)
}

/// Mutable dock state, changed by settings updates, monitor changes and
/// edits to `dock.json`.
struct State {
    layout: DockLayout,
    /// Primary monitor rect `(x, y, w, h)`.
    monitor: (i32, i32, i32, i32),
    entries: Vec<AppEntry>,
}

/// Where the zoom animation is.
#[derive(Default)]
struct ZoomState {
    /// The pointer is over the dock: the icons aim at the zoom.
    active: bool,
    /// Last pointer position along the axis, in rest row coordinates.
    pointer: Option<f64>,
    /// Current scale of every icon.
    scales: Vec<f64>,
    /// A tick callback is running.
    ticking: bool,
    /// Frame time of the previous tick, in microseconds.
    last_frame_us: Option<i64>,
    /// Frames of the running animation, the slowest layout step and the
    /// longest gap between frames, in milliseconds (logged at its end).
    frames: u32,
    worst_layout_ms: f64,
    worst_gap_ms: f64,
}

/// The dock window and the state it renders from.
struct Dock {
    window: gtk::ApplicationWindow,
    /// Holds the strip (first, so it is drawn under the icons) and the icons
    /// at positions set by [`Dock::layout_frame`].
    canvas: gtk::Fixed,
    /// The strip the icons sit on; its CSS is the dock background.
    shelf: gtk::Box,
    /// The icons, in dock order.
    icons: RefCell<Vec<DockIcon>>,
    state: RefCell<State>,
    zoom: RefCell<ZoomState>,
    /// The stylesheet of the background, rebuilt by [`Dock::apply_style`].
    style_provider: gtk::CssProvider,
    /// Whether an input region was set on the surface (zoom room present).
    input_shaped: Cell<bool>,
    /// Whether see-through pixels show what is below (a compositor runs);
    /// kept up to date by [`Dock::watch_compositor`].
    see_through: Cell<bool>,
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
    ENABLED.store(layout.visible, Ordering::Relaxed);

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
    win.add_css_class(WINDOW_CLASS);
    let canvas = gtk::Fixed::new();
    let shelf = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    shelf.add_css_class(SHELF_CLASS);
    // Clicks and drops on the strip belong to the window's controllers.
    shelf.set_can_target(false);
    win.set_child(Some(&canvas));

    let style_provider = gtk::CssProvider::new();
    style_provider.connect_parsing_error(|_, section, error| {
        opusdm_ipc::err_log!("opusdm-dock", "dock stylesheet error at {}: {error}", section.to_str());
    });
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &style_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let dock = Rc::new(Dock {
        window: win.clone(),
        canvas,
        shelf,
        icons: RefCell::new(Vec::new()),
        state: RefCell::new(State { layout, monitor, entries }),
        zoom: RefCell::new(ZoomState::default()),
        style_provider,
        input_shaped: Cell::new(false),
        see_through: Cell::new(compositor_running()),
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
            dock.apply_surface_hints();
            window::set_below(w);
            // The window manager adopts the window and runs its placement
            // policy shortly after map, and drops state messages sent before
            // it has adopted it: re-pin the geometry and re-send the states a
            // few times over the first second.
            let dock = dock.clone();
            opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || {
                dock.pin();
                window::set_below(&dock.window);
                // The frame the compositor hints go on comes with adoption.
                dock.apply_compositor_hints();
            });
        });
    }

    dock.install_window_controllers();
    dock.install_zoom_tracking();
    dock.rebuild_items();
    dock.watch_config();
    dock.watch_monitors();
    dock.watch_compositor();
    if is_shown() {
        win.present();
    } else {
        opusdm_ipc::debug_log!("opusdm-dock", "dock disabled in the settings: not shown");
    }

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
            extent: self.zoom_model_of(&s).extent(),
        }
    }

    /// Whether the display can show see-through windows (last check of
    /// [`compositor_running`]).
    fn can_see_through(&self) -> bool {
        self.see_through.get()
    }

    /// The zoom of the current icons, at rest scale 1.0 when the zoom is off
    /// or the display cannot show the room it grows into.
    fn zoom_model_of(&self, s: &State) -> Zoom {
        let style = &s.layout.style;
        let max_scale = if style.zoom_enabled && self.can_see_through() { style.zoom_percent as f64 / 100.0 } else { 1.0 };
        Zoom { count: s.entries.len(), icon_size: s.layout.icon_size, max_scale }
    }

    /// [`Dock::zoom_model_of`] the current state.
    fn zoom_model(&self) -> Zoom {
        self.zoom_model_of(&self.state.borrow())
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

    /// Rebuild the icons from the current entries, restyle the strip and
    /// resize the window.
    fn rebuild_items(self: &Rc<Self>) {
        while let Some(child) = self.canvas.first_child() {
            self.canvas.remove(&child);
        }
        let place = self.placement();
        let model = self.zoom_model();
        let max_size = (place.icon_size as f64 * model.max_scale).ceil() as i32;
        let scale = self.window.scale_factor();
        let entries = self.state.borrow().entries.clone();
        self.canvas.put(&self.shelf, 0.0, 0.0);
        let icons: Vec<DockIcon> = entries.iter().map(|entry| self.build_item(entry, place.icon_size, max_size, scale)).collect();
        for icon in &icons {
            self.canvas.put(icon, 0.0, 0.0);
        }
        *self.icons.borrow_mut() = icons;
        {
            let mut zoom = self.zoom.borrow_mut();
            zoom.active = false;
            zoom.pointer = None;
            zoom.scales = vec![1.0; entries.len()];
        }
        self.apply_style();
        let (w, h) = place.window_size(entries.len());
        self.canvas.set_size_request(w, h);
        self.layout_frame();
        self.window.set_default_size(w, h);
        self.apply_geometry();
        self.pin();
        self.apply_surface_hints();
    }

    /// One launcher slot: the app icon, its name as tooltip, click to launch.
    fn build_item(self: &Rc<Self>, entry: &AppEntry, icon_size: i32, max_size: i32, scale: i32) -> DockIcon {
        let image = DockIcon::new(entry.info.icon().as_ref(), icon_size, max_size, scale);
        image.add_css_class("opusdm-dock-item");
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
    fn install_drag_source(self: &Rc<Self>, image: &DockIcon, id: &str) {
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::MOVE);
        let payload = id.to_string();
        source.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&payload.to_value()))
        });
        let dragged = image.clone();
        let dock = self.clone();
        source.connect_drag_begin(move |src, _| {
            let paintable = gtk::WidgetPaintable::new(Some(&dragged));
            src.set_icon(Some(&paintable), 0, 0);
            // Drops land on the row at rest (see `Placement::insert_index`).
            dock.end_zoom();
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
    fn show_item_menu(self: &Rc<Self>, image: &DockIcon, entry: &AppEntry) {
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

    /// Apply a new layout live: show or hide the dock, and rebuild it when
    /// anything else changed. No-op when nothing changed.
    fn set_layout(self: &Rc<Self>, layout: DockLayout) {
        let old = self.state.borrow().layout.clone();
        if old == layout {
            return;
        }
        let rebuild = old.shape() != layout.shape();
        ENABLED.store(layout.visible, Ordering::Relaxed);
        self.state.borrow_mut().layout = layout;
        if rebuild {
            self.rebuild_items();
        }
        if self.window.is_visible() != is_shown() {
            opusdm_ipc::debug_log!("opusdm-dock", "dock {}", if is_shown() { "shown" } else { "hidden" });
            self.window.set_visible(is_shown());
        }
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

    /// Load the background stylesheet for the current style.
    fn apply_style(&self) {
        let style = self.state.borrow().layout.style.clone();
        let see_through = self.can_see_through();
        let image_uri = style.image.as_deref().and_then(|path| {
            if path.is_file() { return Some(gio::File::for_path(path).uri().to_string()); }
            opusdm_ipc::err_log!("opusdm-dock", "dock background image {} is not a file: drawn without it", path.display());
            None
        });
        if style.opacity < 100 && !see_through {
            opusdm_ipc::err_log!("opusdm-dock", "no compositor: the dock background stays opaque (opacity {}%)", style.opacity);
        }
        if style.zoom_enabled && !see_through {
            opusdm_ipc::err_log!("opusdm-dock", "no compositor: the dock icon zoom is off");
        }
        self.style_provider.load_from_data(&style_css(&style, image_uri.as_deref(), see_through));
    }

    /// The input region and the compositor hints of the realized window.
    /// With zoom room, only the strip takes the pointer at rest, so the
    /// see-through room above it never steals clicks from the desktop.
    fn apply_surface_hints(&self) {
        let Some(surface) = self.window.surface() else { return };
        self.apply_compositor_hints();
        let place = self.placement();
        if place.extent == Extent::default() && !self.input_shaped.get() {
            return;
        }
        let count = self.state.borrow().entries.len();
        let (w, h) = place.window_size(count);
        let rect = if self.zoom.borrow().active || place.extent == Extent::default() {
            (0, 0, w, h)
        } else {
            self.rest_strip_rect()
        };
        let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(rect.0, rect.1, rect.2, rect.3));
        surface.set_input_region(&region);
        self.input_shaped.set(place.extent != Extent::default());
    }

    /// [`window::set_compositor_hints`] for the current look: no shadow
    /// when part of the window is see-through.
    fn apply_compositor_hints(&self) {
        let see_through = self.placement().extent != Extent::default()
            || (self.state.borrow().layout.style.opacity < 100 && self.can_see_through());
        window::set_compositor_hints(&self.window, see_through);
    }

    /// Window rect `(x, y, w, h)` of the strip at rest.
    fn rest_strip_rect(&self) -> (i32, i32, i32, i32) {
        let place = self.placement();
        let model = self.zoom_model();
        let (length, thickness) = (model.rest_length(), model.strip_thickness());
        let (w, h) = place.window_size(model.count);
        let side = place.extent.side;
        match place.position() {
            DockPosition::Bottom => (side, h - thickness, length, thickness),
            DockPosition::Top => (side, 0, length, thickness),
            DockPosition::Left => (0, side, thickness, length),
            DockPosition::Right => (w - thickness, side, thickness, length),
        }
    }

    /// Put the strip and every icon where the current zoom puts them.
    fn layout_frame(&self) {
        let place = self.placement();
        let model = self.zoom_model();
        let (frame, (w, h)) = {
            let zoom = self.zoom.borrow();
            (model.frame(&zoom.scales, zoom.pointer, place.extent), place.window_size(model.count))
        };
        let position = place.position();
        let thickness = model.strip_thickness() as f64;
        // From (along the axis, across from the screen edge, size along,
        // size across) to window (x, y, w, h).
        let to_window = |axis: f64, cross: f64, along: f64, across: f64| -> (f64, f64, f64, f64) {
            match position {
                DockPosition::Bottom => (axis, h as f64 - cross - across, along, across),
                DockPosition::Top => (axis, cross, along, across),
                DockPosition::Left => (cross, axis, across, along),
                DockPosition::Right => (w as f64 - cross - across, axis, across, along),
            }
        };
        let (x, y, sw, sh) = to_window(frame.strip_axis, 0.0, frame.strip_length, thickness);
        self.shelf.set_size_request(sw.round() as i32, sh.round() as i32);
        self.canvas.move_(&self.shelf, x, y);
        for (icon, b) in self.icons.borrow().iter().zip(&frame.icons) {
            let size = b.size.round();
            let (x, y, _, _) = to_window(b.axis, b.cross, size, size);
            icon.set_size(size as i32);
            self.canvas.move_(icon, x, y);
        }
    }

    /// Follow the pointer over the dock: entering the strip starts the zoom,
    /// leaving the strip and the grown icons ends it.
    fn install_zoom_tracking(self: &Rc<Self>) {
        let motion = gtk::EventControllerMotion::new();
        let dock = self.clone();
        motion.connect_enter(move |_, x, y| dock.pointer_at(x, y));
        let dock = self.clone();
        motion.connect_motion(move |_, x, y| dock.pointer_at(x, y));
        let dock = self.clone();
        motion.connect_leave(move |_| dock.end_zoom());
        self.window.add_controller(motion);
    }

    /// The pointer is at window `(x, y)`.
    fn pointer_at(self: &Rc<Self>, x: f64, y: f64) {
        let place = self.placement();
        if place.extent == Extent::default() {
            return;
        }
        let horizontal = placement::is_horizontal(place.position());
        let (axis, _) = if horizontal { (x, y) } else { (y, x) };
        let active = self.zoom.borrow().active;
        let over = if active { self.over_grown_dock(x, y) } else { contains(self.rest_strip_rect(), x, y) };
        if !over {
            if active { self.end_zoom(); }
            return;
        }
        let rest_start = (place.extent.side + placement::DOCK_PAD + placement::BORDER) as f64;
        {
            let mut zoom = self.zoom.borrow_mut();
            zoom.pointer = Some(axis - rest_start);
            zoom.active = true;
        }
        if !active {
            opusdm_ipc::debug_log!("opusdm-dock", "zoom starts at {axis:.0}");
            self.apply_surface_hints();
        }
        self.start_ticking();
    }

    /// Whether window `(x, y)` is on the strip or on one of the icon slots
    /// as they are drawn now.
    fn over_grown_dock(&self, x: f64, y: f64) -> bool {
        let pad = placement::ITEM_PAD as f64;
        let shelf = self.shelf.allocation();
        if contains((shelf.x(), shelf.y(), shelf.width(), shelf.height()), x, y) {
            return true;
        }
        self.icons.borrow().iter().any(|icon| {
            let a = icon.allocation();
            let (ax, ay, aw, ah) = (a.x() as f64 - pad, a.y() as f64 - pad, a.width() as f64 + 2.0 * pad, a.height() as f64 + 2.0 * pad);
            x >= ax && x < ax + aw && y >= ay && y < ay + ah
        })
    }

    /// Let the icons shrink back to rest, and give the room above the strip
    /// back to the windows below.
    fn end_zoom(self: &Rc<Self>) {
        if !self.zoom.borrow().active {
            return;
        }
        self.zoom.borrow_mut().active = false;
        opusdm_ipc::debug_log!("opusdm-dock", "zoom ends");
        self.apply_surface_hints();
        self.start_ticking();
    }

    /// Run [`Dock::tick`] on every frame until the icons settle.
    fn start_ticking(self: &Rc<Self>) {
        {
            let mut zoom = self.zoom.borrow_mut();
            if zoom.ticking {
                return;
            }
            zoom.ticking = true;
            zoom.last_frame_us = None;
            zoom.frames = 0;
            zoom.worst_layout_ms = 0.0;
            zoom.worst_gap_ms = 0.0;
        }
        let dock = self.clone();
        self.canvas.add_tick_callback(move |_, clock| {
            if dock.tick(clock.frame_time()) {
                gtk::glib::ControlFlow::Continue
            } else {
                gtk::glib::ControlFlow::Break
            }
        });
    }

    /// One animation frame at frame time `now_us`: move every icon scale
    /// toward its target and lay the dock out. `false` once all settled.
    fn tick(&self, now_us: i64) -> bool {
        let started = Instant::now();
        let model = self.zoom_model();
        let settled = {
            let mut zoom = self.zoom.borrow_mut();
            let dt_ms = zoom.last_frame_us.map(|last| (now_us - last) as f64 / 1000.0).unwrap_or(16.0);
            zoom.last_frame_us = Some(now_us);
            zoom.worst_gap_ms = zoom.worst_gap_ms.max(if zoom.frames == 0 { 0.0 } else { dt_ms });
            zoom.frames += 1;
            let targets = model.target_scales(if zoom.active { zoom.pointer } else { None });
            if zoom.scales.len() != targets.len() {
                zoom.scales = vec![1.0; targets.len()];
            }
            let step = 1.0 - (-dt_ms.max(0.0) / ZOOM_TAU_MS).exp();
            let mut settled = true;
            for (scale, target) in zoom.scales.iter_mut().zip(&targets) {
                *scale += (target - *scale) * step;
                if (target - *scale).abs() < ZOOM_SETTLE {
                    *scale = *target;
                } else {
                    settled = false;
                }
            }
            settled
        };
        self.layout_frame();
        let mut zoom = self.zoom.borrow_mut();
        zoom.worst_layout_ms = zoom.worst_layout_ms.max(started.elapsed().as_secs_f64() * 1000.0);
        if !settled {
            return true;
        }
        zoom.ticking = false;
        opusdm_ipc::debug_log!(
            "opusdm-dock",
            "zoom settled after {} frames: slowest layout step {:.2} ms, longest frame gap {:.1} ms",
            zoom.frames,
            zoom.worst_layout_ms,
            zoom.worst_gap_ms
        );
        false
    }

    /// Restyle and rebuild when a compositor starts or stops: transparency
    /// and the zoom depend on it. Checked every [`COMPOSITOR_CHECK_MS`].
    fn watch_compositor(self: &Rc<Self>) {
        let dock = self.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(COMPOSITOR_CHECK_MS), move || {
            let running = compositor_running();
            if running != dock.see_through.get() {
                opusdm_ipc::debug_log!("opusdm-dock", "compositor {}: restyling the dock", if running { "started" } else { "stopped" });
                dock.see_through.set(running);
                dock.rebuild_items();
            }
            gtk::glib::ControlFlow::Continue
        });
    }
}

/// Whether see-through windows work on this display: a compositing manager
/// owns the screen (X11), or the display says it composites (Wayland), and
/// windows get an alpha channel. A failed X11 check is logged and counts as
/// no compositor, so the dock stays opaque rather than black.
fn compositor_running() -> bool {
    let Some(display) = gdk::Display::default() else { return false };
    if !display.is_rgba() {
        return false;
    }
    if opusdm_ipc::session::is_wayland() {
        return display.is_composited();
    }
    match opusdm_ui::with_x11(|x11| x11.compositor_running()) {
        Ok(running) => running,
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-dock", "cannot tell whether a compositor runs: {e}");
            false
        }
    }
}

/// Whether window `(x, y)` is inside `rect` `(x, y, w, h)`.
fn contains(rect: (i32, i32, i32, i32), x: f64, y: f64) -> bool {
    let (rx, ry, rw, rh) = rect;
    x >= rx as f64 && x < (rx + rw) as f64 && y >= ry as f64 && y < (ry + rh) as f64
}

/// Background of the dock window, `0xRRGGBB` (the panel's colour).
const DOCK_BACKGROUND: u32 = 0x2e3436;

/// Colour of the thin border around the dock, `0xRRGGBB`.
const DOCK_BORDER_COLOR: u32 = 0x555753;

/// Background of the app item under the pointer, `0xRRGGBB`.
const DOCK_HOVER: u32 = 0x3c4043;

/// Corner radius of the hover highlight, in pixels.
const DOCK_HOVER_RADIUS: i32 = 6;

/// CSS class of the dock window.
const WINDOW_CLASS: &str = "opusdm-dock-window";

/// CSS class of the strip the icons sit on.
const SHELF_CLASS: &str = "opusdm-dock";

/// The dock stylesheet: opaque background, thin border, hover highlight.
fn dock_css() -> String {
    format!(
        "window {{ background-color: #{DOCK_BACKGROUND:06x}; border: {border}px solid #{DOCK_BORDER_COLOR:06x}; }}\n\
         .opusdm-dock-item:hover {{ background-color: #{DOCK_HOVER:06x}; border-radius: {DOCK_HOVER_RADIUS}px; }}",
        border = placement::BORDER,
    )
}

/// The stylesheet of the dock window and its strip for `style`: the strip
/// carries the colour, the border and `image_uri` (a `file://` URI), faded
/// as a whole to the style opacity. Without `see_through` the window keeps
/// the opaque panel colour and the strip stays opaque.
fn style_css(style: &DockStyle, image_uri: Option<&str>, see_through: bool) -> String {
    let window_background = if see_through { "transparent".to_string() } else { format!("#{DOCK_BACKGROUND:06x}") };
    let opacity = if see_through { style.opacity as f64 / 100.0 } else { 1.0 };
    let image = match image_uri {
        Some(uri) => {
            let (size, repeat) = match style.image_mode {
                DockImageMode::Fill => ("cover", "no-repeat"),
                DockImageMode::Tile => ("auto", "repeat"),
            };
            format!(" background-image: url(\"{uri}\"); background-size: {size}; background-repeat: {repeat}; background-position: center;")
        }
        None => String::new(),
    };
    // With the zoom the size is the hover feedback; the highlight would also
    // stay on an icon that slid away from under a still pointer.
    let hover = if style.zoom_enabled && see_through {
        format!("\nwindow.{WINDOW_CLASS} .opusdm-dock-item:hover {{ background-color: transparent; }}")
    } else {
        String::new()
    };
    format!(
        "window.{WINDOW_CLASS} {{ background-color: {window_background}; border-width: 0; }}\n\
         .{SHELF_CLASS} {{ background-color: #{DOCK_BACKGROUND:06x}; border: {border}px solid #{DOCK_BORDER_COLOR:06x}; opacity: {opacity:.2};{image} }}{hover}",
        border = placement::BORDER,
    )
}

/// Install [`dock_css`] for the whole display.
fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&dock_css());
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stylesheet_is_built_from_the_named_colours() {
        let css = dock_css();
        assert!(css.contains("window { background-color: #2e3436; "), "{css}");
        assert!(css.contains(&format!("border: {}px solid #555753;", placement::BORDER)), "{css}");
        assert!(css.contains(".opusdm-dock-item:hover { background-color: #3c4043; border-radius: 6px; }"), "{css}");
    }

    #[test]
    fn the_classic_style_is_the_old_opaque_look() {
        let css = style_css(&DockStyle::default(), None, true);
        assert!(css.contains("window.opusdm-dock-window { background-color: transparent; border-width: 0; }"), "{css}");
        assert!(css.contains(".opusdm-dock { background-color: #2e3436; border: 1px solid #555753; opacity: 1.00; }"), "{css}");
    }

    #[test]
    fn opacity_and_image_go_on_the_strip() {
        let style = DockStyle { opacity: 40, image_mode: DockImageMode::Tile, ..Default::default() };
        let css = style_css(&style, Some("file:///tmp/wood%20grain.png"), true);
        assert!(css.contains("opacity: 0.40;"), "{css}");
        assert!(css.contains("background-image: url(\"file:///tmp/wood%20grain.png\"); background-size: auto; background-repeat: repeat;"), "{css}");
        let css = style_css(&DockStyle { image_mode: DockImageMode::Fill, ..style }, Some("file:///a.png"), true);
        assert!(css.contains("background-size: cover; background-repeat: no-repeat;"), "{css}");
    }

    #[test]
    fn the_zoom_drops_the_hover_highlight() {
        let zoomed = DockStyle { zoom_enabled: true, ..Default::default() };
        assert!(style_css(&zoomed, None, true).contains(".opusdm-dock-item:hover { background-color: transparent; }"));
        assert!(!style_css(&DockStyle::default(), None, true).contains(":hover"));
        assert!(!style_css(&zoomed, None, false).contains(":hover"), "no compositor, no zoom: the highlight stays");
    }

    #[test]
    fn without_a_compositor_everything_stays_opaque() {
        let css = style_css(&DockStyle { opacity: 10, ..Default::default() }, None, false);
        assert!(css.contains("window.opusdm-dock-window { background-color: #2e3436;"), "{css}");
        assert!(css.contains("opacity: 1.00;"), "{css}");
    }

    #[test]
    fn a_show_or_hide_alone_needs_no_rebuild() {
        let layout = DockLayout {
            position: DockPosition::Bottom,
            icon_size: 48,
            panel: PanelPosition::Top,
            panel_height: 40,
            visible: true,
            style: DockStyle::default(),
        };
        let hidden = DockLayout { visible: false, ..layout.clone() };
        assert_eq!(layout.shape(), hidden.shape());
        let zoomed = DockLayout { style: DockStyle { zoom_enabled: true, ..Default::default() }, ..layout.clone() };
        assert_ne!(layout.shape(), zoomed.shape());
    }
}
