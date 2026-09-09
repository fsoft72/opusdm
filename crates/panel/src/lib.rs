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
mod window;
pub mod widgets;

pub use window::PANEL_HEIGHT;

use ewmh::EwmhEvent;
use widgets::pager::Pager;
use widgets::tasklist::Tasklist;

/// Callbacks the panel needs back into the hub for actions it cannot own
/// itself (the Settings dialog is coupled to the hub command channel).
pub struct PanelHooks {
    /// Open the hub Settings dialog.
    pub on_settings: Box<dyn Fn() + 'static>,
}

/// The slice of EWMH state the left-hand widgets render from.
#[derive(Default)]
struct PanelState {
    desktop_count: u32,
    current_desktop: u32,
    desktop_names: Vec<String>,
    clients: Vec<ewmh::ClientWin>,
    active: Option<u32>,
}

/// Build the panel window on `app`, wire every widget, and return it. The
/// window is presented before returning.
pub fn attach(app: &gtk::Application, hooks: PanelHooks) -> gtk::ApplicationWindow {
    let (mon_w, _mon_h) = gtk::gdk::Display::default()
        .as_ref()
        .map(window::monitor_geometry)
        .unwrap_or((1280, 800));

    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Panel")
        .decorated(false)
        .resizable(false)
        .default_width(mon_w)
        .default_height(window::PANEL_HEIGHT)
        .build();

    win.connect_realize(move |w| window::make_panel_window(w, mon_w));
    win.connect_map(move |w| {
        window::make_panel_window(w, mon_w);
        // Re-pin the geometry and re-assert the stacking a few times over the
        // first second: the window manager runs its placement policy and the
        // hub's full-screen desktop window settles its own geometry around the
        // same moment, either of which can bury a just-mapped dock.
        for delay in [80_u64, 400, 1000] {
            let w = w.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                window::pin_geometry(&w, mon_w);
                window::raise_above(&w);
            });
        }
    });

    install_css();

    let bar = gtk::CenterBox::new();
    bar.add_css_class("opusdm-panel");
    bar.set_height_request(window::PANEL_HEIGHT);

    // Left side: application menu, pager, window list.
    let pager = Rc::new(Pager::new());
    let tasklist = Rc::new(Tasklist::new());

    let left = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    left.set_margin_start(6);
    left.set_hexpand(true);
    left.append(&appmenu::build(hooks, &win));
    left.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    left.append(pager.widget());
    left.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    left.append(tasklist.widget());
    bar.set_start_widget(Some(&left));

    // Right side: CPU, network, clock.
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    right.set_margin_end(8);
    right.append(&widgets::sysmon::build());
    right.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    right.append(&widgets::clock::build());
    bar.set_end_widget(Some(&right));

    win.set_child(Some(&bar));

    // Drain the EWMH observer into the left-hand widgets.
    let state = Rc::new(RefCell::new(PanelState::default()));
    let rx = ewmh::spawn();
    {
        let (state, pager, tasklist) = (state.clone(), pager.clone(), tasklist.clone());
        gtk::glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                apply_ewmh(&state, &pager, &tasklist, ev);
            }
        });
    }

    win.present();
    win
}

/// Fold one observer snapshot into `state` and repaint whatever it touched.
fn apply_ewmh(
    state: &Rc<RefCell<PanelState>>,
    pager: &Pager,
    tasklist: &Tasklist,
    ev: EwmhEvent,
) {
    let mut s = state.borrow_mut();
    match ev {
        EwmhEvent::Desktops { count, current, names } => {
            s.desktop_count = count;
            s.current_desktop = current;
            s.desktop_names = names;
            pager.update(s.desktop_count, s.current_desktop, &s.desktop_names);
            tasklist.update(&s.clients, s.current_desktop, s.active);
        }
        EwmhEvent::CurrentDesktop(current) => {
            s.current_desktop = current;
            pager.update(s.desktop_count, s.current_desktop, &s.desktop_names);
            tasklist.update(&s.clients, s.current_desktop, s.active);
        }
        EwmhEvent::Clients(clients) => {
            s.clients = clients;
            tasklist.update(&s.clients, s.current_desktop, s.active);
        }
        EwmhEvent::ActiveWindow(active) => {
            s.active = active;
            tasklist.update(&s.clients, s.current_desktop, s.active);
        }
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
         .opusdm-panel-cpu-pct { font-size: 10px; text-shadow: 0 1px 2px #000000; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
