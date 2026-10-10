//! Borderless banks on Wayland. A Wayland client can neither place a normal
//! window nor read where it is, so a borderless bank becomes a layer surface
//! instead: anchored to the top left corner of its monitor, its position is
//! the pair of margins, which the bank sets and therefore always knows. The
//! title bar is gone anyway, so the bank is moved with Alt+drag; the new
//! position is applied when the button is released (while dragging, the
//! pointer is only known relative to the surface itself).
//!
//! Banks with a title bar stay normal windows placed by the compositor.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::ButtonBank;
use opusdm_ui::layer::{Anchors, Exclusive, KeyboardMode, Layer, LayerSpec, Margins};
use opusdm_ui::MonitorRect;

/// The layer of a borderless bank: above the desktop, below normal windows,
/// like the dock.
pub const BANK_LAYER: Layer = Layer::Bottom;

/// Whether the window of `bank` is a layer surface in this session.
pub fn wants_layer(bank: &ButtonBank) -> bool {
    bank.borderless && opusdm_ui::layer::available()
}

/// The monitor holding the point `pos`, else the first one.
pub fn monitor_of(pos: (i32, i32), monitors: &[MonitorRect]) -> Option<MonitorRect> {
    let (x, y) = pos;
    monitors.iter().copied().find(|&(mx, my, w, h)| x >= mx && x < mx + w && y >= my && y < my + h).or_else(|| monitors.first().copied())
}

/// The top left corner of a window of `size` centered on `monitor`.
pub fn centered(monitor: MonitorRect, size: (i32, i32)) -> (i32, i32) {
    let (mx, my, mw, mh) = monitor;
    (mx + (mw - size.0).max(0) / 2, my + (mh - size.1).max(0) / 2)
}

/// `pos` moved by `delta` and kept on `monitors`: the window's top left
/// corner stays inside the monitor it lands on (or the first one).
pub fn moved(pos: (i32, i32), delta: (i32, i32), monitors: &[MonitorRect]) -> (i32, i32) {
    let target = (pos.0 + delta.0, pos.1 + delta.1);
    let Some((mx, my, mw, mh)) = monitor_of(target, monitors) else { return target };
    (target.0.clamp(mx, mx + mw - 1), target.1.clamp(my, my + mh - 1))
}

/// Layer shell placement of a bank whose top left corner is at the screen
/// point `pos`, on `monitor`.
pub fn spec_at(pos: (i32, i32), monitor: MonitorRect) -> LayerSpec {
    let (mx, my, _, _) = monitor;
    LayerSpec {
        layer: BANK_LAYER,
        namespace: "opusdm-bank",
        anchors: Anchors { top: true, left: true, ..Anchors::default() },
        margins: Margins { top: pos.1 - my, left: pos.0 - mx, ..Margins::default() },
        exclusive: Exclusive::Ignore,
        keyboard: KeyboardMode::None,
        monitor: Some(monitor),
    }
}

/// Call `on_drop(dx, dy)` when the window is dragged with Alt held and the
/// button released, `dx`, `dy` being how far the pointer went. The drag is
/// claimed, so the buttons under it do not see the click.
pub fn connect_alt_drag(window: &gtk::Window, on_drop: impl Fn(i32, i32) + 'static) {
    let drag = gtk::GestureDrag::new();
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    drag.connect_drag_begin(|gesture, _, _| {
        if gesture.current_event_state().contains(gtk::gdk::ModifierType::ALT_MASK) {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(widget) = gesture.widget() { widget.set_cursor_from_name(Some("grabbing")); }
        } else {
            gesture.set_state(gtk::EventSequenceState::Denied);
        }
    });
    drag.connect_drag_end(move |gesture, dx, dy| {
        if let Some(widget) = gesture.widget() { widget.set_cursor(None); }
        if (dx, dy) != (0.0, 0.0) { on_drop(dx.round() as i32, dy.round() as i32); }
    });
    window.add_controller(drag);
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: MonitorRect = (0, 0, 1920, 1080);
    const RIGHT: MonitorRect = (1920, 0, 1280, 1024);

    #[test]
    fn a_point_belongs_to_the_monitor_holding_it() {
        assert_eq!(monitor_of((100, 100), &[LEFT, RIGHT]), Some(LEFT));
        assert_eq!(monitor_of((2000, 100), &[LEFT, RIGHT]), Some(RIGHT));
        assert_eq!(monitor_of((5000, 100), &[LEFT, RIGHT]), Some(LEFT));
        assert_eq!(monitor_of((0, 0), &[]), None);
    }

    #[test]
    fn the_margins_are_relative_to_the_monitor() {
        let spec = spec_at((2020, 50), RIGHT);
        assert_eq!((spec.margins.left, spec.margins.top), (100, 50));
        assert!(spec.anchors.top && spec.anchors.left && !spec.anchors.right && !spec.anchors.bottom);
        assert_eq!(spec.monitor, Some(RIGHT));
    }

    #[test]
    fn a_window_is_centered_on_its_monitor() {
        assert_eq!(centered(RIGHT, (280, 24)), (1920 + 500, 500));
        assert_eq!(centered(LEFT, (4000, 4000)), (0, 0));
    }

    #[test]
    fn a_move_stays_on_a_monitor() {
        assert_eq!(moved((100, 100), (50, -20), &[LEFT, RIGHT]), (150, 80));
        assert_eq!(moved((1900, 100), (100, 0), &[LEFT, RIGHT]), (2000, 100));
        assert_eq!(moved((100, 100), (-500, -500), &[LEFT, RIGHT]), (0, 0));
    }
}
