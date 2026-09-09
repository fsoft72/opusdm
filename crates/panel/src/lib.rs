//! opusdm-panel: a KDE Plasma-style top panel for the OpusDM desktop.
//!
//! A separate top-docked window (owned by the hub's `gtk::Application`) with an
//! application menu, a virtual-desktop pager and per-desktop window list on the
//! left, and CPU / network / clock widgets on the right. Independent of the hub
//! socket server: it needs only X11/EWMH, `/proc` and the `.desktop` database.

use gtk4 as gtk;
use gtk::prelude::*;
use std::time::Duration;

mod window;
pub mod widgets;

pub use window::PANEL_HEIGHT;

/// Callbacks the panel needs back into the hub for actions it cannot own
/// itself (the Settings dialog is coupled to the hub command channel).
pub struct PanelHooks {
    /// Open the hub Settings dialog.
    pub on_settings: Box<dyn Fn()>,
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
        let w = w.clone();
        gtk::glib::timeout_add_local_once(Duration::from_millis(80), move || {
            window::pin_geometry(&w, mon_w);
        });
    });

    install_css();

    let bar = gtk::CenterBox::new();
    bar.add_css_class("opusdm-panel");
    bar.set_height_request(window::PANEL_HEIGHT);

    // Right side: CPU, network, clock.
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    right.set_margin_end(8);
    right.append(&widgets::sysmon::build());
    right.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    right.append(&widgets::clock::build());
    bar.set_end_widget(Some(&right));

    let _ = &hooks;

    win.set_child(Some(&bar));
    win.present();
    win
}

/// The panel stylesheet. Added at APPLICATION priority so it layers over the
/// GTK theme without a resource bundle.
fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        ".opusdm-panel { background-color: #2e3436; color: #d3d7cf; }\n\
         .opusdm-panel button { padding: 2px 8px; }\n\
         .opusdm-panel-clock-time { font-size: 15px; font-weight: bold; }\n\
         .opusdm-panel-clock-date { font-size: 10px; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
