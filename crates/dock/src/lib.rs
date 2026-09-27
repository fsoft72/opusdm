//! opusdm-dock: a launcher-only dock for the OpusDM desktop.
//!
//! A single opaque window with a row of application icons, docked to a screen
//! edge. It reserves no screen space: maximised windows cover it. See
//! `docs/dock.md`.

use gtk4 as gtk;
use gtk::prelude::*;
use std::time::Duration;

mod placement;
mod window;

pub use placement::{DockEdge, Placement};

/// Icon names shown until the pinned app list (`dock.json`) is implemented.
const PLACEHOLDER_ICONS: [&str; 3] = ["system-file-manager", "utilities-terminal", "web-browser"];

/// Build the dock on the primary monitor and present it. The window is owned
/// by `app`, so it lives as long as the application does.
pub fn attach(app: &gtk::Application) -> gtk::ApplicationWindow {
    install_css();

    let icons = PLACEHOLDER_ICONS;
    let count = icons.len();
    let rect = gtk::gdk::Display::default()
        .as_ref()
        .map(opusdm_ui::list_monitors)
        .and_then(|rects| rects.first().copied())
        .unwrap_or((0, 0, 1280, 800));
    let place = Placement { edge: DockEdge::default(), mon_x: rect.0, mon_y: rect.1, mon_w: rect.2, mon_h: rect.3 };
    let (_, _, width, height) = place.rect(count);

    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Dock")
        .decorated(false)
        // Resizable on the GTK side so the window follows the X geometry we
        // pin; the WM is kept from resizing it by the min = max size hints.
        .resizable(true)
        .default_width(width)
        .default_height(height)
        .build();

    {
        win.connect_realize(move |w| window::make_dock_window(w, place, count));
    }
    win.connect_map(move |w| {
        window::make_dock_window(w, place, count);
        window::set_below(w);
        // The window manager runs its placement policy around map time, so
        // re-pin the geometry a few times over the first second.
        for delay in [80_u64, 400, 1000] {
            let w = w.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                window::pin_geometry(&w, place, count);
            });
        }
    });

    let row = gtk::Box::new(orientation(place.edge), 0);
    row.add_css_class("opusdm-dock");
    row.set_margin_start(placement::DOCK_PAD);
    row.set_margin_end(placement::DOCK_PAD);
    row.set_margin_top(placement::DOCK_PAD);
    row.set_margin_bottom(placement::DOCK_PAD);
    for name in icons {
        row.append(&build_item(name));
    }
    win.set_child(Some(&row));
    win.present();
    win
}

/// Box orientation of the icon row for a dock docked to `edge`.
fn orientation(edge: DockEdge) -> gtk::Orientation {
    match edge {
        DockEdge::Top | DockEdge::Bottom => gtk::Orientation::Horizontal,
        DockEdge::Left | DockEdge::Right => gtk::Orientation::Vertical,
    }
}

/// One launcher slot showing the themed icon `icon_name`.
fn build_item(icon_name: &str) -> gtk::Image {
    let image = gtk::Image::from_icon_name(icon_name);
    image.set_pixel_size(placement::ICON_SIZE);
    image.set_margin_start(placement::ITEM_PAD);
    image.set_margin_end(placement::ITEM_PAD);
    image.set_margin_top(placement::ITEM_PAD);
    image.set_margin_bottom(placement::ITEM_PAD);
    image
}

/// The dock stylesheet: opaque background and a thin border.
fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "window { background-color: #2e3436; border: 1px solid #555753; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
