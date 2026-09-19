//! opusdm-panel: a KDE Plasma-style top panel for the OpusDM desktop.
//!
//! A separate top-docked window (owned by the hub's `gtk::Application`) with an
//! application menu, a virtual-desktop pager and per-desktop window list on the
//! left, and CPU / network / clock widgets on the right. Independent of the hub
//! socket server: it needs only X11/EWMH, `/proc` and the `.desktop` database.

use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

mod appmenu;
mod ewmh;
mod systray;
mod window;
pub mod widgets;

pub use window::PANEL_HEIGHT;

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
    pager: Rc<Pager>,
    tasklist: Rc<Tasklist>,
    /// `Some` only for the primary bar (the one built from the first
    /// monitor rect) - one tray manager total, not one per monitor.
    tray: Option<Rc<Tray>>,
}

/// Build one full bar (menu, pager, tasklist, sysmon, clock) on the monitor
/// at `(mon_x, mon_y)`, `mon_w` wide, and present it. `state` seeds the pager
/// and tasklist immediately, so a bar created after startup (monitor plugged
/// in later) doesn't show empty widgets until the next EWMH event.
fn build_bar_window(
    app: &gtk::Application,
    mon_x: i32,
    mon_y: i32,
    mon_w: i32,
    state: &Rc<RefCell<PanelState>>,
    is_primary: bool,
    tray_cmd_tx: std::sync::mpsc::Sender<systray::TrayCmd>,
) -> BarWindow {
    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Panel")
        .decorated(false)
        // Resizable on the GTK side so the dock follows the X geometry we pin
        // when the screen size changes; see the same note in opusdm-hub's
        // toolbar.rs. The WM is kept from resizing it by the min = max size
        // hints in `window.rs`.
        .resizable(true)
        .default_width(mon_w)
        .default_height(window::PANEL_HEIGHT)
        .build();

    // Built before `connect_map` so the reposition call below can capture it;
    // used again when assembling the right-side box further down.
    let tray = if is_primary {
        let icon_size = systray::icon_square_size(window::PANEL_HEIGHT);
        let tray = Tray::new(icon_size, tray_cmd_tx);
        tray.set_origin(mon_x, mon_y);
        tray.update(&state.borrow().tray_order);
        Some(tray)
    } else {
        None
    };

    win.connect_realize(move |w| window::make_panel_window(w, mon_x, mon_y, mon_w));
    {
        let tray_for_map = tray.clone();
        win.connect_map(move |w| {
            window::make_panel_window(w, mon_x, mon_y, mon_w);
            // Re-pin the geometry and re-assert the stacking a few times over
            // the first second: the window manager runs its placement policy
            // and the hub's full-screen desktop window settles its own
            // geometry around the same moment, either of which can bury a
            // just-mapped dock. A tray reposition rides along, since a
            // rebuilt primary bar's placeholders haven't been laid out yet
            // when this closure is first scheduled.
            for delay in [80_u64, 400, 1000] {
                let w = w.clone();
                let tray = tray_for_map.clone();
                gtk::glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                    window::pin_geometry(&w, mon_x, mon_y, mon_w);
                    window::raise_above(&w);
                    if let Some(tray) = &tray {
                        tray.reposition_now();
                    }
                });
            }
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
        pager.update(s.desktop_count, s.current_desktop, &s.desktop_names);
        tasklist.update(&s.clients, s.current_desktop, s.active);
    }

    let left = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    left.set_margin_start(6);
    left.set_hexpand(true);
    left.append(&appmenu::build(&win));
    left.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    left.append(pager.widget());
    left.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    left.append(tasklist.widget());
    bar.set_start_widget(Some(&left));

    // Right side: tray (primary bar only), CPU, network, clock.
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    right.set_margin_end(8);
    if let Some(tray) = &tray {
        right.append(tray.widget());
        right.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    }
    right.append(&widgets::sysmon::build());
    right.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    right.append(&widgets::clock::build());
    bar.set_end_widget(Some(&right));

    win.set_child(Some(&bar));
    win.present();

    BarWindow { window: win, pager, tasklist, tray }
}

/// Build one bar window per monitor, wire every widget, and keep them in sync
/// with monitor hotplug and mirror changes. Identical monitor rects (a
/// mirrored output) share a single bar, since X shows the same pixels twice
/// there - see `opusdm_ui::list_monitors`.
pub fn attach(app: &gtk::Application) {
    let state = Rc::new(RefCell::new(PanelState::default()));
    let windows: Rc<RefCell<Vec<BarWindow>>> = Rc::new(RefCell::new(Vec::new()));
    let (tray_cmd_tx, tray_evt_rx) = systray::spawn(window::PANEL_HEIGHT);

    let rebuild: Rc<dyn Fn(Vec<(i32, i32, i32, i32)>)> = {
        let app = app.clone();
        let state = state.clone();
        let windows = windows.clone();
        let tray_cmd_tx = tray_cmd_tx.clone();
        Rc::new(move |mut rects: Vec<(i32, i32, i32, i32)>| {
            for bw in windows.borrow_mut().drain(..) {
                bw.window.close();
            }
            if rects.is_empty() {
                rects.push((0, 0, 1280, 800));
            }
            let built = rects
                .into_iter()
                .enumerate()
                .map(|(i, (x, y, w, _h))| {
                    build_bar_window(&app, x, y, w, &state, i == 0, tray_cmd_tx.clone())
                })
                .collect();
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
}

/// Fold one observer snapshot into `state` and repaint every bar it touched.
fn apply_ewmh(state: &Rc<RefCell<PanelState>>, windows: &[BarWindow], ev: EwmhEvent) {
    let mut s = state.borrow_mut();
    match ev {
        EwmhEvent::Desktops { count, current, names } => {
            s.desktop_count = count;
            s.current_desktop = current;
            s.desktop_names = names;
        }
        EwmhEvent::CurrentDesktop(current) => s.current_desktop = current,
        EwmhEvent::Clients(clients) => s.clients = clients,
        EwmhEvent::ActiveWindow(active) => s.active = active,
    }
    for bw in windows {
        bw.pager.update(s.desktop_count, s.current_desktop, &s.desktop_names);
        bw.tasklist.update(&s.clients, s.current_desktop, s.active);
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
         .opusdm-panel-pager button.active { background-color: #4a90d9; color: #ffffff; }\n\
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
