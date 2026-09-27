//! opusdm-dock: a launcher-only dock for the OpusDM desktop.
//!
//! A single opaque window with a row of application icons, docked to a screen
//! edge. It reserves no screen space: maximised windows cover it. See
//! `docs/dock.md`.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::{DockConfig, DockPosition, PanelPosition};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

mod apps;
mod placement;
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
        .title("OpusDM Dock")
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

    {
        let dock = dock.clone();
        win.connect_realize(move |_| dock.apply_geometry());
    }
    {
        let dock = dock.clone();
        win.connect_map(move |w| {
            dock.apply_geometry();
            window::set_below(w);
            // The window manager runs its placement policy around map time,
            // so re-pin the geometry a few times over the first second.
            for delay in [80_u64, 400, 1000] {
                let dock = dock.clone();
                gtk::glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                    dock.pin();
                });
            }
        });
    }

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
        image
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

    /// Follow the primary monitor across hotplug and screen size changes.
    fn watch_monitors(self: &Rc<Self>) {
        if let Some(display) = gtk::gdk::Display::default() {
            let dock = self.clone();
            opusdm_ui::watch_monitors(&display, move |rects| dock.set_monitor(rects));
        }
        let dock = self.clone();
        opusdm_ui::watch_screen_size(move |_w, _h| {
            let rects = gtk::gdk::Display::default()
                .as_ref()
                .map(opusdm_ui::list_monitors)
                .unwrap_or_default();
            dock.set_monitor(rects);
        });
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
    provider.load_from_data(
        "window { background-color: #2e3436; border: 1px solid #555753; }\n\
         .opusdm-dock-item:hover { background-color: #3c4043; border-radius: 6px; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
