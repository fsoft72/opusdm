//! Wayland layer shell (`wlr-layer-shell`) for the windows that are part of
//! the desktop itself: the desktop background, the panel, the dock and the
//! small overlays (Run window, device popup).
//!
//! On X11 these windows are placed with EWMH hints and absolute coordinates.
//! A Wayland client can do neither, so on a compositor that offers the layer
//! shell (wlroots based ones such as labwc and sway, KDE) they become layer
//! surfaces: anchored to monitor edges, stacked in a layer, optionally
//! reserving an exclusive zone. Everywhere else (X11, GNOME) nothing here
//! does anything and the callers keep their X11 path.
//!
//! [`init`] must run before the window is realized; [`update`] can change
//! the placement of a layer window at any time.

use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::LayerShell;

pub use gtk4_layer_shell::{KeyboardMode, Layer};

use crate::MonitorRect;

/// Monitor edges a layer surface is anchored to. Anchored to two opposite
/// edges, the surface stretches between them; anchored to one, it is
/// centered along it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Anchors {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl Anchors {
    /// Anchored to all four edges: the surface fills the monitor.
    pub const ALL: Anchors = Anchors { top: true, right: true, bottom: true, left: true };
}

/// Distance from each anchored edge, in pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Margins {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

/// How a layer surface treats the space other surfaces reserve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exclusive {
    /// Reserve the surface's own size along its anchored edge (a panel).
    Auto,
    /// Reserve nothing, but stay inside the space others reserved.
    None,
    /// Reserve nothing and ignore what others reserved (a background).
    Ignore,
}

/// Everything that places one layer surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerSpec {
    pub layer: Layer,
    /// Name the compositor may use in its rules (`opusdm-panel`...).
    pub namespace: &'static str,
    pub anchors: Anchors,
    pub margins: Margins,
    pub exclusive: Exclusive,
    pub keyboard: KeyboardMode,
    /// Monitor to show on, by its rectangle; `None` lets the compositor pick.
    pub monitor: Option<MonitorRect>,
}

/// Whether windows can become layer surfaces here: a Wayland session whose
/// compositor offers `wlr-layer-shell`.
pub fn available() -> bool {
    opusdm_ipc::session::is_wayland() && gtk4_layer_shell::is_supported()
}

/// Whether `window` is a layer surface (set up by [`init`]).
pub fn is_layer(window: &impl IsA<gtk::Window>) -> bool {
    available() && window.as_ref().is_layer_window()
}

/// Turn the not yet realized `window` into a layer surface placed by `spec`.
/// Returns `false`, leaving the window untouched, when the layer shell is not
/// [`available`] or the window is already realized (too late to convert).
pub fn init(window: &impl IsA<gtk::Window>, spec: &LayerSpec) -> bool {
    if !available() { return false; }
    let window = window.as_ref();
    if window.is_realized() {
        opusdm_ipc::err_log!("opusdm", "layer shell: '{}' is already realized", spec.namespace);
        return false;
    }
    window.init_layer_shell();
    window.set_namespace(Some(spec.namespace));
    apply(window, spec);
    true
}

/// Re-place a layer window by `spec`. Does nothing for other windows.
pub fn update(window: &impl IsA<gtk::Window>, spec: &LayerSpec) {
    if !is_layer(window) { return; }
    apply(window.as_ref(), spec);
}

/// Set every property of `spec` on the layer window `window`.
fn apply(window: &gtk::Window, spec: &LayerSpec) {
    use gtk4_layer_shell::Edge;
    window.set_layer(spec.layer);
    window.set_keyboard_mode(spec.keyboard);
    let edges = [
        (Edge::Top, spec.anchors.top, spec.margins.top),
        (Edge::Right, spec.anchors.right, spec.margins.right),
        (Edge::Bottom, spec.anchors.bottom, spec.margins.bottom),
        (Edge::Left, spec.anchors.left, spec.margins.left),
    ];
    for (edge, anchored, margin) in edges {
        window.set_anchor(edge, anchored);
        window.set_margin(edge, margin);
    }
    match spec.exclusive {
        Exclusive::Auto => window.auto_exclusive_zone_enable(),
        Exclusive::None => window.set_exclusive_zone(0),
        Exclusive::Ignore => window.set_exclusive_zone(-1),
    }
    let monitor = spec.monitor.and_then(monitor_at);
    window.set_monitor(monitor.as_ref());
}

/// The GDK monitor whose geometry is `rect`, if there is one.
fn monitor_at(rect: MonitorRect) -> Option<gdk::Monitor> {
    let monitors = gdk::Display::default()?.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()))
        .find(|m| {
            let g = m.geometry();
            (g.x(), g.y(), g.width(), g.height()) == rect
        })
}
