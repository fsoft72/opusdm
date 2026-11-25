//! Integration tests on the headless backend.

mod common;

use std::time::Duration;

use common::{Anchor, Compositor, HasSurface, KeyboardInteractivity, Layer, Rect, TestClient};

#[test]
fn a_toplevel_gets_configured_and_frame_callbacks() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_frame(&w, Duration::from_secs(2)));
}

#[test]
fn a_client_that_disconnects_abruptly_does_not_take_the_compositor_down() {
    let mut comp = Compositor::start();
    let mut a = TestClient::new(&comp);
    let _w = a.map_toplevel((400, 300));
    drop(a); // socket closed with the objects still alive, like kill -9
    let mut b = TestClient::new(&comp);
    let w = b.map_toplevel((400, 300));
    assert!(b.wait_frame(&w, Duration::from_secs(2)));
    assert!(comp.is_running());
}

#[test]
fn minimize_is_ignored_and_the_window_keeps_getting_frames() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_minimized();
    c.roundtrip();
    assert!(c.wait_frame(&w, Duration::from_secs(2)));
}

#[test]
fn sigterm_ends_the_compositor_cleanly() {
    let mut comp = Compositor::start();
    comp.signal(libc::SIGTERM); // harness helper, `libc` dev-dependency (workspace)
    assert_eq!(comp.wait_exit(Duration::from_secs(2)), Some(0));
}

#[test]
fn a_toplevel_unmapped_by_a_null_buffer_maps_again() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    c.unmap(&w);
    assert!(c.remap(&w), "no configure after the new initial commit");
    assert!(c.wait_frame(&w, Duration::from_secs(2)));
}

#[test]
fn a_popup_gets_configured_and_shown() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let p = c.map_popup(&w, Rect { x: 10, y: 10, w: 1, h: 1 }, (200, 150));
    assert_eq!(c.popup_configure(&p).w, 200);
    assert!(c.wait_frame(&p, Duration::from_secs(2)));
}

#[test]
fn a_top_panel_spans_the_output_width() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let p = c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    assert_eq!(c.layer_configure(&p), (1920, 30));
}

#[test]
fn a_maximized_window_leaves_room_for_an_exclusive_zone() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_maximized();
    c.roundtrip();
    assert_eq!(c.last_configure(&w), (1920, 1050));
    w.xdg().unset_maximized();
    c.roundtrip();
    assert_eq!(c.last_configure(&w), (400, 300));
}

#[test]
fn a_fullscreen_window_covers_the_whole_output() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_fullscreen(None);
    c.roundtrip();
    assert_eq!(c.last_configure(&w), (1920, 1080));
}

#[test]
fn an_exclusive_keyboard_layer_gets_the_keyboard() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
}

#[test]
fn a_popup_at_the_bottom_right_corner_stays_inside_the_output() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    // 100x100 surface in the bottom-right corner: its origin is (1820, 980).
    let corner = c.map_layer(Layer::Top, Anchor::BOTTOM | Anchor::RIGHT, (100, 100), 0, KeyboardInteractivity::None);
    let p = c.map_popup(&corner, Rect { x: 99, y: 99, w: 1, h: 1 }, (300, 400));
    let g = c.popup_configure(&p);
    assert!(1820 + g.x >= 0 && 1820 + g.x + 300 <= 1920, "x out of the output: {g:?}");
    assert!(980 + g.y >= 0 && 980 + g.y + 400 <= 1080, "y out of the output: {g:?}");
}

#[test]
fn the_keyboard_goes_back_to_the_window_when_the_exclusive_layer_goes() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
    c.destroy_layer(run);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(w.wl_surface().clone()));
}

#[test]
fn a_window_going_away_does_not_steal_the_keyboard_from_an_exclusive_layer() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.unmap(&b);
    let _late = c.map_toplevel((200, 100));
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
}

#[test]
fn a_layer_surface_hidden_by_destroying_its_role_keeps_the_client_alive() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let note = c.map_layer(Layer::Overlay, Anchor::TOP | Anchor::RIGHT, (300, 80), 0, KeyboardInteractivity::None);
    assert_eq!(c.hide_layer_keeping_surface(&note), Ok(()));
    let w = c.map_toplevel((200, 100));
    assert!(c.wait_frame(&w, Duration::from_secs(2)));
}

#[test]
fn a_layer_surface_created_again_on_the_same_surface_gets_its_own_size() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let note = c.map_layer(Layer::Overlay, Anchor::TOP | Anchor::RIGHT, (300, 80), 0, KeyboardInteractivity::None);
    assert_eq!(c.hide_layer_keeping_surface(&note), Ok(()));
    // The role-less anchors set after the destroy must not leak into the new
    // layer surface: anchored to TOP only, it keeps the asked width.
    let again = c.map_layer_again(&note, Layer::Overlay, Anchor::TOP, (200, 60));
    assert_eq!(c.layer_configure(&again), (200, 60));
    c.roundtrip();
}

#[test]
fn a_layer_surface_created_again_does_not_inherit_the_roleless_anchors() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let note = c.map_layer(Layer::Overlay, Anchor::TOP | Anchor::RIGHT, (300, 80), 0, KeyboardInteractivity::None);
    assert_eq!(c.hide_layer_keeping_surface(&note), Ok(()));
    // No anchor and a width of 0 is a protocol error on a fresh surface; it
    // must be one here too (with the anchors left from the destroy it would
    // silently stretch over the whole output).
    assert!(c.layer_again_without_anchor(&note, (0, 60)).is_err());
    assert!(comp_alive(&comp));
}

/// The compositor still accepts clients.
fn comp_alive(comp: &Compositor) -> bool {
    let mut other = TestClient::new(comp);
    let w = other.map_toplevel((100, 100));
    other.wait_frame(&w, Duration::from_secs(2))
}

#[test]
fn a_maximized_window_grows_when_the_panel_goes_away() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let panel = c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_maximized();
    c.roundtrip();
    assert_eq!(c.last_configure(&w), (1920, 1050));
    c.destroy_layer(panel);
    c.roundtrip();
    assert_eq!(c.last_configure(&w), (1920, 1080));
    assert!(c.wait_frame(&w, Duration::from_secs(2)));
}
