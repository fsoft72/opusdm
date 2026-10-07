//! opusdm-panel: a KDE Plasma-style top panel for the OpusDM desktop.
//!
//! A separate top-docked window (owned by the hub's `gtk::Application`) with an
//! application menu, a virtual-desktop pager and per-desktop window list on the
//! left, and CPU / network / clock widgets on the right. Independent of the hub
//! socket server: it needs only X11/EWMH, `/proc` and the `.desktop` database.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::PanelPosition;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod appmenu;
mod dbusmenu;
mod device_popup;
mod devices_center;
mod audio_center;
mod audio_model;
mod network_center;
mod network_model;
mod devices_model;
mod ewmh;
mod sni;
mod systray;
mod window;
pub mod widgets;

pub use window::PANEL_HEIGHT;

use window::Placement;

use ewmh::EwmhEvent;
use widgets::pager::Pager;
use widgets::tasklist::Tasklist;
use widgets::tray::Tray;

/// The slice of EWMH state the left-hand widgets render from.
#[derive(Default)]
struct PanelState {
    desktop_count: u32,
    current_desktop: u32,
    desktop_names: Vec<String>,
    clients: Vec<ewmh::ClientWin>,
    active: Option<u32>,
    /// Docked tray icon xids, in dock order. Only the primary bar's `Tray`
    /// widget reads this (see `BarWindow.tray`); kept in shared state so a
    /// monitor-hotplug rebuild can reseed a freshly built primary bar
    /// without waiting for the next dock/undock event.
    tray_order: Vec<u32>,
}

/// One bar window and the widgets on it that need repainting on EWMH events.
struct BarWindow {
    window: gtk::ApplicationWindow,
    /// Edge and monitor rect; the map/settle closures read it live so a
    /// position change re-docks the window without rebuilding it.
    place: Rc<Cell<Placement>>,
    /// The application menu button, whose popover direction follows `place`.
    menu: gtk::MenuButton,
    pager: Rc<Pager>,
    tasklist: Rc<Tasklist>,
    /// Kept alive so the strip stays registered for refreshes.
    _quick_apps: Rc<widgets::quickapps::QuickApps>,
    /// `Some` only for the primary bar (the one built from the first
    /// monitor rect) - one tray manager total, not one per monitor.
    tray: Option<Rc<Tray>>,
}

/// Pixels between the device, audio and network tray buttons.
const TRAY_SPACING: i32 = 2;

/// Build one full bar (menu, pager, quick apps, tasklist, device, audio and network trays, sysmon, clock) on the monitor
/// and edge `place` describes, and present it. `state` seeds the pager
/// and tasklist immediately, so a bar created after startup (monitor plugged
/// in later) doesn't show empty widgets until the next EWMH event.
fn build_bar_window(
    app: &gtk::Application,
    place: Placement,
    state: &Rc<RefCell<PanelState>>,
    is_primary: bool,
    tray_cmd_tx: systray::TraySender,
    sni_cmd_tx: std::sync::mpsc::Sender<sni::SniCmd>,
    sni_evt_rx: Option<async_channel::Receiver<sni::SniEvent>>,
    devices: Rc<devices_center::DeviceCenter>,
    audio: Rc<audio_center::AudioCenter>,
    network: Rc<network_center::NetworkCenter>,
) -> BarWindow {
    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title(WINDOW_TITLE)
        .decorated(false)
        // Resizable on the GTK side so the dock follows the X geometry we pin
        // when the screen size changes; see the same note in opusdm-hub's
        // toolbar.rs. The WM is kept from resizing it by the min = max size
        // hints in `window.rs`.
        .resizable(true)
        .default_width(place.mon_w)
        .default_height(window::PANEL_HEIGHT)
        .build();
    let place = Rc::new(Cell::new(place));

    // Built before `connect_map` so the reposition call below can capture it;
    // used again when assembling the right-side box further down.
    let tray = if is_primary {
        let icon_size = systray::icon_square_size(window::PANEL_HEIGHT);
        let tray = Tray::new(icon_size, tray_cmd_tx);
        tray.set_origin(place.get().mon_x, place.get().bar_y());
        tray.update(&state.borrow().tray_order);
        Some(tray)
    } else {
        None
    };
    let sni_widget = if is_primary {
        sni_evt_rx.map(|rx| widgets::sni::build(sni_cmd_tx, rx))
    } else {
        None
    };

    {
        let place = place.clone();
        win.connect_realize(move |w| window::make_panel_window(w, place.get()));
    }
    {
        let tray_for_map = tray.clone();
        let place = place.clone();
        win.connect_map(move |w| {
            window::make_panel_window(w, place.get());
            // Re-pin the geometry and re-assert the stacking a few times over
            // the first second: the window manager runs its placement policy
            // and the hub's full-screen desktop window settles its own
            // geometry around the same moment, either of which can bury a
            // just-mapped dock. A tray reposition rides along, since a
            // rebuilt primary bar's placeholders haven't been laid out yet
            // when this closure is first scheduled.
            let w = w.clone();
            let tray = tray_for_map.clone();
            let place = place.clone();
            opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || {
                window::pin_geometry(&w, place.get());
                window::raise_above(&w);
                if let Some(tray) = &tray {
                    tray.reposition_now();
                }
            });
        });
    }

    install_css();

    let bar = gtk::CenterBox::new();
    bar.add_css_class("opusdm-panel");
    bar.set_height_request(window::PANEL_HEIGHT);

    // Left side: application menu, pager, window list.
    let pager = Rc::new(Pager::new());
    let tasklist = Rc::new(Tasklist::new());
    {
        let s = state.borrow();
        pager.update(s.desktop_count, s.current_desktop, &s.desktop_names, &s.clients, s.active);
        tasklist.update(&s.clients, s.current_desktop, s.active, s.desktop_count, &s.desktop_names);
    }

    let left = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    left.set_margin_start(6);
    left.set_hexpand(true);
    let menu = appmenu::build(&win, place.get().position);
    left.append(&menu);
    left.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    left.append(pager.widget());
    left.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    let quick_apps = widgets::quickapps::QuickApps::new();
    left.append(quick_apps.widget());
    left.append(tasklist.widget());
    bar.set_start_widget(Some(&left));

    // Right side: tray (primary bar only), CPU, network, clock.
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    right.set_margin_end(8);
    if let Some(tray) = &tray {
        right.append(tray.widget());
    }
    if let Some(sni_widget) = &sni_widget {
        right.append(sni_widget);
    }
    if tray.is_some() || sni_widget.is_some() {
        right.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    }
    // The device, audio and network trays sit right before the CPU and network
    // monitors, close together, on the primary bar only (a device must not be
    // listed twice). The network tray draws the separator that closes the group.
    if is_primary {
        let trays = gtk::Box::new(gtk::Orientation::Horizontal, TRAY_SPACING);
        trays.append(&devices.button());
        trays.append(&audio.button());
        trays.append(&network.button());
        right.append(&trays);
    }
    right.append(&widgets::sysmon::build());
    right.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    right.append(&widgets::clock::build());
    bar.set_end_widget(Some(&right));

    win.set_child(Some(&bar));
    // While the desktop is hidden a rebuilt bar (monitor change) stays hidden
    // too; the hub shows every panel window again on reveal.
    if !is_hidden() {
        win.present();
    }

    BarWindow { window: win, place, menu, pager, tasklist, tray, _quick_apps: quick_apps }
}

/// Title of every panel bar window; the hub uses it to hide and show the panel.
pub const WINDOW_TITLE: &str = "OpusDM Panel";

/// Whether the desktop is hidden: a bar built while it is stays unpresented.
static HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tell the panel the desktop is now hidden (`true`) or revealed. The hub
/// hides and shows the existing bar windows itself; this only stops a bar
/// rebuilt in between (monitor change) from popping up while hidden.
pub fn set_hidden(hidden: bool) {
    HIDDEN.store(hidden, std::sync::atomic::Ordering::Relaxed);
}

/// Whether [`set_hidden`] last said hidden.
pub fn is_hidden() -> bool {
    HIDDEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Build one bar window per monitor, wire every widget, and keep them in sync
/// with monitor hotplug and mirror changes. Identical monitor rects (a
/// mirrored output) share a single bar, since X shows the same pixels twice
/// there - see `opusdm_ui::list_monitors`.
///
/// The bars start docked to `position`. The returned function re-docks them
/// to another edge live (rebuilding every bar); call it when the desktop
/// settings change.
pub fn attach(
    app: &gtk::Application,
    position: PanelPosition,
    on_open_folder: Rc<dyn Fn(&std::path::Path)>,
) -> Rc<dyn Fn(PanelPosition)> {
    let position = Rc::new(Cell::new(position));
    let devices = devices_center::DeviceCenter::new(on_open_folder, position.clone());
    let audio = audio_center::AudioCenter::new();
    let network = network_center::NetworkCenter::new();
    let state = Rc::new(RefCell::new(PanelState::default()));
    let windows: Rc<RefCell<Vec<BarWindow>>> = Rc::new(RefCell::new(Vec::new()));
    let (tray_cmd_tx, tray_evt_rx) = systray::spawn(window::PANEL_HEIGHT);
    let (sni_cmd_tx, sni_evt_rx) = sni::spawn();
    // Only the primary bar's widget ever drains this receiver - it isn't
    // `Clone`, and a monitor-hotplug rebuild after the first one won't have
    // it to hand the new primary bar (known v1 gap, see the SNI plan).
    let sni_evt_rx = Rc::new(RefCell::new(Some(sni_evt_rx)));

    let rebuild: Rc<dyn Fn(Vec<(i32, i32, i32, i32)>)> = {
        let app = app.clone();
        let state = state.clone();
        let windows = windows.clone();
        let tray_cmd_tx = tray_cmd_tx.clone();
        let sni_cmd_tx = sni_cmd_tx.clone();
        let sni_evt_rx = sni_evt_rx.clone();
        let position = position.clone();
        let devices = devices.clone();
        let audio = audio.clone();
        let network = network.clone();
        Rc::new(move |mut rects: Vec<(i32, i32, i32, i32)>| {
            for bw in windows.borrow_mut().drain(..) {
                bw.window.close();
            }
            if rects.is_empty() {
                rects.push((0, 0, 1280, 800));
            }
            let mut rx_slot = sni_evt_rx.borrow_mut();
            let built = rects
                .into_iter()
                .enumerate()
                .map(|(i, (x, y, w, h))| {
                    let rx = if i == 0 { rx_slot.take() } else { None };
                    let place = Placement { position: position.get(), mon_x: x, mon_y: y, mon_w: w, mon_h: h };
                    build_bar_window(&app, place, &state, i == 0, tray_cmd_tx.clone(), sni_cmd_tx.clone(), rx, devices.clone(), audio.clone(), network.clone())
                })
                .collect();
            drop(rx_slot);
            *windows.borrow_mut() = built;
        })
    };

    let display = gtk::gdk::Display::default();
    rebuild(display.as_ref().map(opusdm_ui::list_monitors).unwrap_or_default());

    if let Some(display) = display.as_ref() {
        let rebuild = rebuild.clone();
        opusdm_ui::watch_monitors(display, move |rects| rebuild(rects));
    }
    // Mirrors opusdm-hub's toolbar.rs: under Xephyr, GDK's monitor list can
    // lag a host-window resize, so also re-scan on the reliable root
    // `ConfigureNotify` signal, not just GDK's own change notifications.
    {
        let rebuild = rebuild.clone();
        opusdm_ui::watch_screen_size(move |_w, _h| {
            let rects = gtk::gdk::Display::default()
                .as_ref()
                .map(opusdm_ui::list_monitors)
                .unwrap_or_default();
            rebuild(rects);
        });
    }

    // Drain the EWMH observer into every bar's widgets.
    let rx = ewmh::spawn();
    {
        let (state, windows) = (state.clone(), windows.clone());
        gtk::glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                apply_ewmh(&state, &windows.borrow(), ev);
            }
        });
    }

    // Drain the tray thread into shared state and the primary bar's widget.
    {
        let (state, windows) = (state.clone(), windows.clone());
        gtk::glib::spawn_future_local(async move {
            while let Ok(ev) = tray_evt_rx.recv().await {
                let mut s = state.borrow_mut();
                match ev {
                    systray::TrayEvent::IconDocked(xid) => s.tray_order.push(xid),
                    systray::TrayEvent::IconRemoved(xid) => s.tray_order.retain(|&x| x != xid),
                }
                let order = s.tray_order.clone();
                drop(s);
                for bw in windows.borrow().iter() {
                    if let Some(tray) = &bw.tray {
                        tray.update(&order);
                    }
                }
            }
        });
    }

    // Live re-dock: move the existing bars to the new edge in place, so the
    // primary bar keeps its tray and SNI widgets.
    Rc::new(move |new_position: PanelPosition| {
        if position.replace(new_position) == new_position {
            return;
        }
        for bw in windows.borrow().iter() {
            redock_bar(bw, new_position);
        }
    })
}

/// Move one bar to `position`: update its placement, flip the app menu's
/// popover direction, re-apply the dock hints and struts, and re-pin the
/// geometry a few times while the window manager settles (see `connect_map`
/// in [`build_bar_window`]).
fn redock_bar(bw: &BarWindow, position: PanelPosition) {
    let mut place = bw.place.get();
    place.position = position;
    bw.place.set(place);
    appmenu::set_direction(&bw.menu, position);
    if let Some(tray) = &bw.tray {
        tray.set_origin(place.mon_x, place.bar_y());
    }
    window::make_panel_window(&bw.window, place);
    window::pin_geometry(&bw.window, place);
    window::raise_above(&bw.window);
    let win = bw.window.clone();
    let cell = bw.place.clone();
    let tray = bw.tray.clone();
    opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || {
        window::pin_geometry(&win, cell.get());
        if let Some(tray) = &tray {
            tray.reposition_now();
        }
    });
}

/// Fold one observer snapshot into `state` and repaint every bar it touched.
fn apply_ewmh(state: &Rc<RefCell<PanelState>>, windows: &[BarWindow], ev: EwmhEvent) {
    let mut s = state.borrow_mut();
    let mut geometry_only = false;
    match ev {
        EwmhEvent::Desktops { count, current, names } => {
            s.desktop_count = count;
            s.current_desktop = current;
            s.desktop_names = names;
        }
        EwmhEvent::CurrentDesktop(current) => s.current_desktop = current,
        EwmhEvent::Clients(clients) => s.clients = clients,
        EwmhEvent::ActiveWindow(active) => s.active = active,
        EwmhEvent::Geometries(rects) => {
            for c in s.clients.iter_mut() {
                if let Some((_, r)) = rects.iter().find(|(xid, _)| *xid == c.xid) {
                    c.rect = *r;
                }
            }
            geometry_only = true;
        }
    }
    for bw in windows {
        bw.pager.update(s.desktop_count, s.current_desktop, &s.desktop_names, &s.clients, s.active);
        if geometry_only {
            continue;
        }
        bw.tasklist.update(&s.clients, s.current_desktop, s.active, s.desktop_count, &s.desktop_names);
    }
}

/// The panel stylesheet. Added at APPLICATION priority so it layers over the
/// GTK theme without a resource bundle.
fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        ".opusdm-panel { background-color: #2e3436; color: #d3d7cf; }\n\
         .opusdm-panel button { padding: 2px 8px; min-height: 0; }\n\
         .opusdm-panel-quickapps button { padding: 2px 3px; }\n\
         .opusdm-panel-tasklist button.active { background-color: #3c4043; }\n\
         .opusdm-panel-clock-time { font-size: 15px; font-weight: bold; }\n\
         .opusdm-panel-clock-date { font-size: 10px; }\n\
         .opusdm-panel-meter { background-color: #14161a; border-radius: 4px; }\n\
         .opusdm-panel-meter-val { font-size: 9px; text-shadow: 0 1px 2px #000000; }\n\
         .opusdm-panel-net-in { color: #74d161; }\n\
         .opusdm-panel-net-out { color: #5c99e6; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
mod hidden_tests {
    use super::*;

    #[test]
    fn the_hidden_flag_can_be_set_and_cleared() {
        set_hidden(true);
        assert!(is_hidden());
        set_hidden(false);
        assert!(!is_hidden());
    }
}
