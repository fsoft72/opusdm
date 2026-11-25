//! Integration tests on the headless backend.

mod common;

use std::time::{Duration, Instant};

use serde_json::json;

use common::ipc::{ipc_hub, ipc_request, ipc_watch};
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
fn a_minimized_window_loses_the_keyboard_and_still_gets_frames() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    b.xdg().set_minimized();
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
    assert!(c.wait_frame(&b, Duration::from_secs(3)));
}

#[test]
fn a_minimized_window_whose_client_dies_leaves_nothing_behind() {
    let comp = Compositor::start();
    let mut app = TestClient::new(&comp);
    let mut other = TestClient::new(&comp);
    let a = app.map_toplevel((400, 300));
    let hidden = other.map_toplevel((400, 300));
    hidden.xdg().set_minimized();
    other.roundtrip();
    drop(hidden);
    drop(other); // killed while its window is minimized
    let b = app.map_toplevel((200, 100));
    assert!(app.wait_keyboard_focus(&b, Duration::from_secs(2)), "a new window must get the keyboard");
    assert!(app.wait_frame(&b, Duration::from_secs(2)));
    assert!(app.wait_frame(&a, Duration::from_secs(2)));
}

#[test]
fn a_minimized_window_unmapped_by_a_null_buffer_maps_again_and_takes_the_keyboard() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    b.xdg().set_minimized();
    c.roundtrip();
    c.unmap(&b);
    assert!(c.remap(&b), "no configure after the new initial commit");
    assert!(c.wait_keyboard_focus(&b, Duration::from_secs(2)), "the mapped window must get the keyboard");
    assert!(c.wait_frame(&b, Duration::from_secs(2)));
}

#[test]
fn minimizing_a_window_with_an_open_menu_gives_the_keyboard_to_the_shown_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    let menu = c.map_popup_with_grab(&b, Rect { x: 10, y: 10, w: 1, h: 1 }, (200, 150));
    assert_eq!(c.keyboard_focus(), Some(menu.wl_surface().clone()));
    b.xdg().set_minimized();
    c.roundtrip();
    assert!(c.popup_done(&menu), "the menu of a minimized window must be dismissed");
    assert!(c.wait_keyboard_focus(&a, Duration::from_secs(2)), "the keyboard must go to the shown window");
    c.destroy_popup(menu);
    // A few housekeeping ticks: the end of the grab must not give the keyboard back to the hidden window.
    assert!(c.wait_frame(&a, Duration::from_secs(2)));
    assert!(c.wait_frame(&a, Duration::from_secs(2)));
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
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

#[test]
fn a_window_menu_does_not_take_the_keyboard_from_an_exclusive_layer() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
    let menu = c.map_popup_grabbed(&w, Rect { x: 10, y: 10, w: 1, h: 1 }, (200, 150));
    assert!(c.popup_dismissed(&menu), "the grab of a window menu must be refused");
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
    c.destroy_popup(menu);
    // A few housekeeping ticks of the compositor go by.
    assert!(c.wait_frame(&run, Duration::from_secs(2)));
    assert!(c.wait_frame(&run, Duration::from_secs(2)));
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
}

#[test]
fn an_exclusive_layer_mapped_over_an_open_menu_keeps_the_keyboard() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let menu = c.map_popup_with_grab(&w, Rect { x: 10, y: 10, w: 1, h: 1 }, (200, 150));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    assert!(c.popup_done(&menu));
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
}

#[test]
fn a_layer_that_becomes_exclusive_over_an_open_menu_closes_it() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::None);
    let menu = c.map_popup_with_grab(&w, Rect { x: 10, y: 10, w: 1, h: 1 }, (200, 150));
    assert!(!c.popup_done(&menu), "the menu must be open before the layer turns exclusive");
    c.set_layer_keyboard(&run, KeyboardInteractivity::Exclusive);
    assert!(c.popup_done(&menu));
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
}

#[test]
fn a_menu_of_the_exclusive_layer_gets_its_grab() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    let menu = c.map_popup_grabbed(&run, Rect { x: 10, y: 10, w: 1, h: 1 }, (200, 150));
    assert!(!c.popup_dismissed(&menu), "the grab of the layer's own menu must be accepted");
    assert_eq!(c.keyboard_focus(), Some(menu.wl_surface().clone()));
    c.destroy_popup(menu);
    // The housekeeping tick moves the keyboard off the dead menu.
    assert!(c.wait_keyboard_focus(&run, Duration::from_secs(2)), "the keyboard did not go back to the layer");
}

#[test]
fn a_maximized_window_grows_when_the_panel_client_is_killed() {
    let comp = Compositor::start();
    let mut app = TestClient::new(&comp);
    let mut panel_client = TestClient::new(&comp);
    let _panel =
        panel_client.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    let w = app.map_toplevel((400, 300));
    w.xdg().set_maximized();
    app.roundtrip();
    assert_eq!(app.last_configure(&w), (1920, 1050));
    drop(panel_client); // socket closed with the panel still mapped, like a killed panel
    assert!(app.wait_configure(&w, (1920, 1080), Duration::from_secs(2)), "got {:?}", app.last_configure(&w));
    assert!(app.wait_frame(&w, Duration::from_secs(2)));
}

#[test]
#[should_panic(expected = "roundtrip: no answer from the compositor")]
fn a_hung_compositor_fails_the_roundtrip_instead_of_hanging() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    comp.signal(libc::SIGSTOP); // the drop of `comp` kills it, stopped or not
    c.roundtrip();
}

#[test]
fn a_panel_hidden_by_a_null_buffer_gives_back_its_zone_and_maps_again() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let panel = c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_maximized();
    c.roundtrip();
    assert_eq!(c.last_configure(&w), (1920, 1050));
    c.hide_layer(&panel);
    assert!(c.wait_configure(&w, (1920, 1080), Duration::from_secs(2)), "got {:?}", c.last_configure(&w));
    assert!(c.show_layer_again(&panel), "the hidden panel got no configure after its new initial commit");
    assert_eq!(c.layer_configure(&panel), (1920, 30));
    assert!(c.wait_configure(&w, (1920, 1050), Duration::from_secs(2)), "got {:?}", c.last_configure(&w));
    assert!(c.wait_frame(&panel, Duration::from_secs(2)));
}

#[test]
fn an_exclusive_layer_hidden_by_a_null_buffer_gives_back_the_keyboard_and_takes_it_again() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    assert!(c.wait_keyboard_focus(&run, Duration::from_secs(2)));
    c.hide_layer(&run);
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)), "the keyboard did not go back to the window");
    assert!(c.show_layer_again(&run), "the hidden layer got no configure after its new initial commit");
    assert!(c.wait_keyboard_focus(&run, Duration::from_secs(2)), "the layer did not get the keyboard again");
}

#[test]
fn a_hidden_exclusive_layer_whose_client_dies_leaves_nothing_behind() {
    let comp = Compositor::start();
    let mut app = TestClient::new(&comp);
    let mut run_client = TestClient::new(&comp);
    let _a = app.map_toplevel((400, 300));
    let run = run_client.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    run_client.hide_layer(&run);
    drop(run);
    drop(run_client); // killed while its layer is hidden
    let b = app.map_toplevel((200, 100));
    assert!(app.wait_keyboard_focus(&b, Duration::from_secs(2)), "a new window must get the keyboard");
    assert!(app.wait_frame(&b, Duration::from_secs(2)));
}

#[test]
fn foreign_toplevels_list_title_app_id_and_states() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_title("Lister".into());
    w.xdg().set_app_id("opusdm-lister".into());
    w.wl_surface().commit();
    let t = c.toplevels();
    assert_eq!((t[0].title.as_str(), t[0].app_id.as_str()), ("Lister", "opusdm-lister"));
    assert!(t[0].states.contains(&2)); // activated
    assert_eq!(t[0].outputs, 1);
}

#[test]
fn a_foreign_toplevel_minimize_and_activate_round_trip() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let t = c.toplevels().remove(0);
    c.minimize(&t);
    assert!(c.toplevels()[0].states.contains(&1));
    c.activate(&t);
    let states = &c.toplevels()[0].states;
    assert!(!states.contains(&1) && states.contains(&2));
}

#[test]
fn a_fullscreen_window_is_reported_fullscreen() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_fullscreen(None);
    c.roundtrip();
    assert!(c.toplevels()[0].states.contains(&3));
}

#[test]
fn a_destroyed_window_is_closed_for_foreign_toplevel_clients() {
    let comp = Compositor::start();
    let mut watcher = TestClient::new(&comp);
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert_eq!(watcher.toplevels().len(), 1);
    drop(w);
    c.roundtrip();
    assert_eq!(watcher.toplevels().len(), 0);
}

/// `(managers, handles)` the compositor keeps for foreign-toplevel clients
/// (`debug/foreign-toplevels`).
fn foreign_counts(comp: &Compositor) -> (u64, u64) {
    let reply = ipc_request(comp, "debug/foreign-toplevels", json!({}));
    assert_eq!(reply["ok"], true, "debug/foreign-toplevels refused: {reply}");
    (reply["managers"].as_u64().expect("managers"), reply["handles"].as_u64().expect("handles"))
}

/// Polls `foreign_counts` until it is `want` or 2 s pass; the last value.
fn wait_foreign_counts(comp: &Compositor, want: (u64, u64)) -> (u64, u64) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let counts = foreign_counts(comp);
        if counts == want || Instant::now() >= deadline {
            return counts;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_foreign_toplevel_client_that_dies_leaves_no_handles_behind() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let mut watcher = TestClient::new(&comp);
    assert_eq!(watcher.toplevels().len(), 1);
    assert_eq!(foreign_counts(&comp), (2, 2));
    drop(watcher); // socket closed with the manager and the handle alive, like kill -9
    assert_eq!(wait_foreign_counts(&comp, (1, 1)), (1, 1));
    drop(c);
    assert_eq!(wait_foreign_counts(&comp, (0, 0)), (0, 0));
}

#[test]
fn a_minimized_window_stays_listed_and_an_unmapped_one_is_closed() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    b.xdg().set_minimized();
    c.roundtrip();
    let t = c.toplevels();
    assert_eq!(t.len(), 2);
    assert!(t[1].states.contains(&1) && !t[1].states.contains(&2));
    c.unmap(&a);
    assert_eq!(c.toplevels().len(), 1);
    assert!(c.remap(&a));
    assert_eq!(c.toplevels().len(), 2);
}

#[test]
fn foreign_toplevel_maximize_and_close_requests_reach_the_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let t = c.toplevels().remove(0);
    t.handle.set_maximized();
    c.roundtrip();
    assert!(c.toplevels()[0].states.contains(&0));
    assert!(c.wait_configure(&w, (1920, 1080), Duration::from_secs(2)));
    t.handle.unset_maximized();
    c.roundtrip();
    assert!(!c.toplevels()[0].states.contains(&0));
    t.handle.close();
    assert!(c.wait_close_request(&w, Duration::from_secs(2)));
}

#[test]
fn four_workspaces_with_the_first_active() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let ws = c.workspaces();
    assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "4"]);
    assert!(ws[0].active && !ws[1].active);
}

#[test]
fn activating_a_workspace_switches_the_desktop() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    c.activate_workspace(2);
    assert!(c.workspaces()[2].active);
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["current"], 2);
}

#[test]
fn a_new_desktop_count_reaches_workspace_clients() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    assert_eq!(c.workspaces().len(), 4);
    ipc_request(&comp, "desktops/set-count", json!({"count": 6}));
    assert_eq!(c.workspaces().len(), 6);
}

#[test]
fn workspaces_carry_one_coordinate_and_one_group() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let coordinates: Vec<_> = c.workspaces().into_iter().map(|w| w.coordinates).collect();
    assert_eq!(coordinates, [vec![0], vec![1], vec![2], vec![3]]);
    assert_eq!(c.workspace_groups(), 1);
}

#[test]
fn a_desktop_switched_over_ipc_is_the_active_workspace() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    ipc_request(&comp, "desktops/switch", json!({"index": 3}));
    let active: Vec<_> = c.workspaces().into_iter().map(|w| w.active).collect();
    assert_eq!(active, [false, false, false, true]);
}

/// `(managers, workspaces)` the compositor keeps for workspace clients
/// (`debug/workspaces`).
fn workspace_counts(comp: &Compositor) -> (u64, u64) {
    let reply = ipc_request(comp, "debug/workspaces", json!({}));
    assert_eq!(reply["ok"], true, "debug/workspaces refused: {reply}");
    (reply["managers"].as_u64().expect("managers"), reply["workspaces"].as_u64().expect("workspaces"))
}

/// Polls `workspace_counts` until it is `want` or 2 s pass; the last value.
fn wait_workspace_counts(comp: &Compositor, want: (u64, u64)) -> (u64, u64) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let counts = workspace_counts(comp);
        if counts == want || Instant::now() >= deadline {
            return counts;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn shrinking_the_desktops_removes_workspaces_and_keeps_the_current_one_active() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    c.activate_workspace(3);
    ipc_request(&comp, "desktops/set-count", json!({"count": 2}));
    let ws = c.workspaces();
    assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["1", "2"]);
    assert!(!ws[0].active && ws[1].active);
    assert_eq!(workspace_counts(&comp), (1, 2));
}

#[test]
fn a_workspace_client_that_dies_leaves_no_workspaces_behind() {
    let comp = Compositor::start();
    let mut a = TestClient::new(&comp);
    let mut b = TestClient::new(&comp);
    assert_eq!(a.workspaces().len(), 4);
    assert_eq!(b.workspaces().len(), 4);
    assert_eq!(workspace_counts(&comp), (2, 8));
    drop(b); // socket closed with the manager and the workspaces alive, like kill -9
    assert_eq!(wait_workspace_counts(&comp, (1, 4)), (1, 4));
    a.activate_workspace(1);
    assert!(a.workspaces()[1].active);
    drop(a);
    assert_eq!(wait_workspace_counts(&comp, (0, 0)), (0, 0));
}

/// The hub table used by the hotkey tests.
fn terminal_table() -> serde_json::Value {
    json!({"hotkeys": [{"key": "Ctrl+backslash", "function": "terminal"}]})
}

#[test]
fn a_hub_hotkey_reaches_the_hub_and_not_the_client_nor_other_watchers() {
    let comp = Compositor::start();
    let mut panel = ipc_watch(&comp);
    let (mut hub, _) = ipc_hub(&comp, terminal_table());
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"}));
    assert!(hub.wait_for_hotkey("terminal", Duration::from_secs(2)));
    assert!(!panel.wait_for("hotkey", Duration::from_millis(300)), "a plain watcher gets no hotkey event");
    c.roundtrip();
    assert_eq!(c.keys_received(), 0, "neither the press nor the release of backslash reach the client");
}

#[test]
fn hotkeys_work_again_after_the_hub_reconnects() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    let (mut hub_a, _) = ipc_hub(&comp, terminal_table());
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"}));
    assert!(hub_a.wait_for_hotkey("terminal", Duration::from_secs(2)), "hub A gets the event");
    let mut panel = ipc_watch(&comp); // a plain watcher, like the panel
    drop(hub_a); // the hub crashes
    assert!(comp.wait_log("hub disconnected", Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"}));
    assert!(comp.wait_log("hotkey Ctrl+backslash: no hub connected", Duration::from_secs(2)));
    assert!(!panel.wait_for("hotkey", Duration::from_millis(300)), "the plain watcher gets no hotkey event");
    c.roundtrip();
    assert_eq!(c.keys_received(), 0, "the key is swallowed with no hub");
    let (mut hub_b, _) = ipc_hub(&comp, terminal_table());
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"}));
    assert!(hub_b.wait_for_hotkey("terminal", Duration::from_secs(2)), "hub B gets the event");
    assert_eq!(comp.log_count("hub disconnected"), 1);
}

#[test]
fn a_one_shot_hotkeys_set_replaces_the_table_without_a_hub() {
    let comp = Compositor::start();
    let mut watch = ipc_watch(&comp);
    let reply = ipc_request(&comp, "hotkeys/set", terminal_table());
    assert_eq!(reply["ok"], true, "{reply}");
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"}));
    assert!(comp.wait_log("hotkey Ctrl+backslash: no hub connected", Duration::from_secs(2)));
    assert!(!watch.wait_for("hotkey", Duration::from_millis(300)));
    assert_eq!(comp.log_count("hub disconnected"), 0, "a one-shot client was never the hub");
}

#[test]
fn a_hub_hotkey_written_with_the_shifted_symbol_matches() {
    let comp = Compositor::start();
    let (mut hub, set) = ipc_hub(&comp, json!({"hotkeys": [{"key": "Ctrl+Shift+exclam", "function": "one"}]}));
    assert_eq!(set["errors"], json!([]), "{set}");
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+Shift+1"}));
    assert!(hub.wait_for_hotkey("one", Duration::from_secs(2)));
}

#[test]
fn the_quit_binding_ends_the_compositor() {
    let mut comp = Compositor::start();
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+Alt+BackSpace"}));
    assert_eq!(comp.wait_exit(Duration::from_secs(2)), Some(0));
}

#[test]
fn an_unbound_key_still_reaches_the_client() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "a"}));
    c.roundtrip();
    assert_eq!(c.keys_received(), 2); // press and release
}

#[test]
fn a_hub_hotkey_that_conflicts_with_a_built_in_is_reported_and_the_built_in_wins() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    let (mut hub, reply) = ipc_hub(&comp, json!({"hotkeys": [{"key": "Alt+space", "function": "run"}]}));
    assert_eq!(reply["errors"].as_array().map(Vec::len), Some(1), "{reply}");
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    assert!(!hub.wait_for_hotkey("run", Duration::from_millis(300)), "the window menu binding wins");
    c.roundtrip();
    assert_eq!(c.keys_received(), 0, "the window menu binding swallows the key");
}

#[test]
fn a_valid_activation_token_raises_and_focuses_a_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let _b = c.map_toplevel((400, 300)); // b has the keyboard
    let token = c.activation_token();
    c.request_activation(&a, &token);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
}

#[test]
fn activation_restores_a_window_on_another_desktop() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let id = ipc_request(&comp, "windows/list", json!({}))["windows"][0]["id"].as_u64().unwrap();
    ipc_request(&comp, "window/move-to-desktop", json!({"id": id, "desktop": 1}));
    let token = c.activation_token();
    c.request_activation(&a, &token);
    c.roundtrip();
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["current"], 1);
}

#[test]
fn an_unknown_token_changes_nothing() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    c.request_activation(&a, "not-a-token");
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(b.wl_surface().clone()));
    assert!(comp.wait_log("refused: unknown or expired token", Duration::from_secs(2)), "an unknown token is refused with a log line");
}

#[test]
fn activating_a_layer_surface_is_not_an_error() {
    // GTK asks xdg-activation for every surface it shows, layer surfaces
    // (panel, notification popups, Run window) too: nothing happens and no
    // error line is written (no OPUSDM_DEBUG here, so only err_log lines).
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let panel = c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    let token = c.activation_token();
    c.request_activation(&panel, &token);
    c.roundtrip();
    // An unknown token is an error line: when it shows, the layer request before it was handled.
    c.request_activation(&a, "not-a-token");
    c.roundtrip();
    assert!(comp.wait_log("refused: unknown or expired token", Duration::from_secs(2)));
    assert_eq!(comp.log_count("activation of"), 1, "only the unknown token is an error line");
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
}

#[test]
fn activating_a_layer_surface_is_a_debug_trace() {
    let comp = Compositor::start_debug();
    let mut c = TestClient::new(&comp);
    c.map_toplevel((400, 300));
    let panel = c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    let token = c.activation_token();
    c.request_activation(&panel, &token);
    c.roundtrip();
    assert!(comp.wait_log("ignored: a zwlr_layer_surface_v1 is not a window", Duration::from_secs(2)));
}

#[test]
fn an_activation_token_works_only_once() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    let token = c.activation_token();
    c.request_activation(&a, &token);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
    c.request_activation(&b, &token);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()), "a used token is removed");
    assert!(comp.wait_log("refused: unknown or expired token", Duration::from_secs(2)));
}

#[test]
fn an_activation_before_the_window_maps_is_applied_when_it_maps() {
    let comp = Compositor::start_debug();
    let mut c = TestClient::new(&comp);
    let _b = c.map_toplevel((400, 300));
    let token = c.activation_token();
    let a = c.map_toplevel_activated((400, 300), &token);
    assert!(comp.wait_log("pending activation of window 2 applied", Duration::from_secs(2)));
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
}

#[test]
fn a_pending_activation_goes_away_with_its_window() {
    let comp = Compositor::start_debug();
    let mut c = TestClient::new(&comp);
    let b = c.map_toplevel((400, 300));
    let token = c.activation_token();
    c.activate_toplevel_destroyed_before_map(&token);
    assert!(comp.wait_log("pending activation of window 2 discarded", Duration::from_secs(2)));
    assert_eq!(c.keyboard_focus(), Some(b.wl_surface().clone()));
}

#[test]
fn the_window_menu_minimizes_the_focused_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    for key in ["Alt+space", "Down", "Return"] {
        ipc_request(&comp, "debug/key", json!({"key": key}));
    }
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["minimized"], true);
    assert_eq!(c.keys_received(), 0);
}

#[test]
fn the_window_menu_closes_when_its_window_goes() {
    let mut comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    drop(w);
    c.roundtrip();
    ipc_request(&comp, "debug/key", json!({"key": "Return"})); // no window: nothing runs
    let mut d = TestClient::new(&comp);
    let _x = d.map_toplevel((400, 300));
    ipc_request(&comp, "debug/key", json!({"key": "a"}));
    d.roundtrip();
    assert_eq!(d.keys_received(), 2); // the menu is gone, keys reach clients again
    assert!(comp.is_running());
}

#[test]
fn the_first_window_menu_item_toggles_maximize() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    for key in ["Alt+space", "Return"] {
        ipc_request(&comp, "debug/key", json!({"key": key}));
    }
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["maximized"], true);
    for key in ["Alt+space", "Return"] {
        ipc_request(&comp, "debug/key", json!({"key": key}));
    }
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["maximized"], false, "Restore");
    c.roundtrip();
    assert_eq!(c.keys_received(), 0);
}

#[test]
fn the_window_menu_moves_the_window_to_another_desktop() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    // Maximize, Minimize, Move to desktop; Right opens it on "1" (this desktop, disabled); Down: "2".
    for key in ["Alt+space", "Down", "Down", "Right", "Down", "Return"] {
        ipc_request(&comp, "debug/key", json!({"key": key}));
    }
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["desktop"], 1);
    c.roundtrip();
    assert_eq!(c.keys_received(), 0);
}

#[test]
fn escape_closes_the_window_menu_and_keys_reach_the_client_again() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    for key in ["Alt+space", "a", "Escape"] {
        ipc_request(&comp, "debug/key", json!({"key": key}));
    }
    c.roundtrip();
    assert_eq!(c.keys_received(), 0, "keys go to the open menu, never to the client");
    ipc_request(&comp, "debug/key", json!({"key": "a"}));
    c.roundtrip();
    assert_eq!(c.keys_received(), 2);
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert_eq!(list["windows"][0]["minimized"], false, "Escape runs no action: {list}");
    assert_eq!(list["windows"][0]["maximized"], false, "Escape runs no action: {list}");
}

#[test]
fn an_exclusive_layer_mapped_over_the_window_menu_closes_it() {
    let comp = Compositor::start_debug();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    assert!(comp.wait_log("window menu closed: an exclusive keyboard layer mapped", Duration::from_secs(2)));
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
    ipc_request(&comp, "debug/key", json!({"key": "Return"}));
    c.roundtrip();
    assert_eq!(c.keys_received(), 2, "the layer gets the keys, the menu is closed");
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["maximized"], false);
}

#[test]
fn the_window_menu_needs_a_focused_window() {
    let comp = Compositor::start_debug();
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    assert!(comp.wait_log("window menu: no focused window", Duration::from_secs(2)));
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "a"}));
    c.roundtrip();
    assert_eq!(c.keys_received(), 2, "no menu was left open");
}

#[test]
fn the_window_menu_ends_an_open_client_menu_first() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let menu = c.map_popup_with_grab(&w, Rect { x: 10, y: 10, w: 1, h: 1 }, (200, 150));
    assert!(!c.popup_done(&menu), "the client menu must be open first");
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    c.roundtrip();
    assert!(c.popup_done(&menu), "the client menu is dismissed");
    for key in ["Down", "Return"] {
        ipc_request(&comp, "debug/key", json!({"key": key}));
    }
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["minimized"], true);
    c.roundtrip();
    assert_eq!(c.keys_received(), 0);
}

#[test]
fn the_window_menu_closes_when_its_client_dies() {
    let mut comp = Compositor::start_debug();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    assert!(comp.wait_log("window menu opened on window 1", Duration::from_secs(2)));
    std::mem::forget(w); // no xdg_toplevel.destroy queued: the client death destroys it
    drop(c); // socket closed with the window alive, like kill -9
    assert!(comp.wait_log("window menu closed: its window was destroyed", Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "Return"}));
    assert_eq!(comp.log_count("window menu: ToggleMaximize"), 0, "no action on a dead window");
    let mut d = TestClient::new(&comp);
    let x = d.map_toplevel((400, 300));
    assert!(d.wait_keyboard_focus(&x, Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "a"}));
    d.roundtrip();
    assert_eq!(d.keys_received(), 2);
    assert!(comp.is_running());
}

#[test]
fn a_hub_hotkey_is_swallowed_and_logged_while_the_window_menu_is_open() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    let (mut hub, _) = ipc_hub(&comp, terminal_table());
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"}));
    assert!(comp.wait_log("hotkey Ctrl+backslash: swallowed, the window menu is open", Duration::from_secs(2)));
    assert!(!hub.wait_for_hotkey("terminal", Duration::from_millis(300)), "the hub gets no event");
    ipc_request(&comp, "debug/key", json!({"key": "Return"}));
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["maximized"], true, "the menu stayed open");
    c.roundtrip();
    assert_eq!(c.keys_received(), 0);
}

#[test]
fn a_second_window_menu_key_closes_the_menu() {
    let comp = Compositor::start_debug();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    assert!(comp.wait_log("window menu closed: the window menu key was pressed again", Duration::from_secs(2)));
    c.roundtrip();
    assert_eq!(c.keys_received(), 0, "both Alt+Space presses are swallowed");
    ipc_request(&comp, "debug/key", json!({"key": "Return"}));
    c.roundtrip();
    assert_eq!(c.keys_received(), 2, "the menu is closed, Return reaches the client");
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["maximized"], false);
}

/// `(id, activated)` of every window in `windows/list`, in stacking order.
fn activated_ids(comp: &Compositor) -> Vec<(u64, bool)> {
    let reply = ipc_request(comp, "windows/list", json!({}));
    let windows = reply["windows"].as_array().cloned().unwrap_or_default();
    windows.iter().map(|w| (w["id"].as_u64().unwrap_or(0), w["activated"].as_bool().unwrap_or(false))).collect()
}

/// Ids of the windows `windows/list` reports activated.
fn active_ids(comp: &Compositor) -> Vec<u64> {
    activated_ids(comp).into_iter().filter(|(_, a)| *a).map(|(id, _)| id).collect()
}

/// Titles of the foreign toplevels with the `activated` state (2).
fn foreign_active_titles(c: &mut TestClient) -> Vec<String> {
    c.toplevels().into_iter().filter(|t| t.states.contains(&2)).map(|t| t.title).collect()
}

/// Clicks at `(x, y)` through `debug/click`.
fn click(comp: &Compositor, x: i64, y: i64) {
    let reply = ipc_request(comp, "debug/click", json!({"x": x, "y": y}));
    assert_eq!(reply["ok"], true, "debug/click refused: {reply}");
}

/// Top-left corner of window `id` from `windows/list`.
fn window_geometry(comp: &Compositor, id: u64) -> (i64, i64) {
    let reply = ipc_request(comp, "windows/list", json!({}));
    let windows = reply["windows"].as_array().expect("windows array");
    let w = windows.iter().find(|w| w["id"] == id).expect("window in the list");
    (w["geometry"]["x"].as_i64().expect("x"), w["geometry"]["y"].as_i64().expect("y"))
}

/// A panel like the OpusDM bar: TOP layer across the top, 40 px tall,
/// exclusive zone 40, keyboard on demand.
fn map_panel(c: &mut TestClient) -> common::TestLayer {
    c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 40), 40, KeyboardInteractivity::OnDemand)
}

/// Maps a toplevel titled `title`.
fn map_titled(c: &mut TestClient, title: &str) -> common::TestWindow {
    let w = c.map_toplevel((400, 300));
    w.xdg().set_title(title.into());
    w.wl_surface().commit();
    c.roundtrip();
    w
}

#[test]
fn the_first_click_after_a_menu_closed_goes_where_the_pointer_is() {
    // The tasklist menu case: a menu of the panel holds a popup grab, an
    // item is clicked, the client destroys the menu; the next pointer
    // motion (one event, as xdotool sends) and click land elsewhere. The
    // ended grab must not eat that motion: the click is hit-tested where
    // the pointer now is.
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let panel = map_panel(&mut c);
    let a = c.map_toplevel((200, 150));
    click(&comp, 960, 20);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(panel.wl_surface().clone()), "the panel has the keyboard for its menu");
    let menu = c.map_popup_grabbed(&panel, Rect { x: 1000, y: 10, w: 1, h: 1 }, (200, 200));
    click(&comp, 1050, 100);
    c.roundtrip();
    c.destroy_popup(menu);
    // As a user does: some time passes, the compositor's housekeeping
    // (every 16 ms here) cleans the dead menu up and the grab has ended.
    std::thread::sleep(Duration::from_millis(200));
    // Window A is at the cascade origin, far from the menu.
    let before = c.pointer_presses().len();
    click(&comp, 50, 100);
    c.roundtrip();
    let presses = c.pointer_presses();
    assert_eq!(presses.len(), before + 1, "one press reached the client");
    let (surface, x, y) = &presses[before];
    assert_eq!(surface, a.wl_surface(), "the click after the menu reaches the window under the pointer");
    let geometry = window_geometry(&comp, 1);
    assert_eq!((*x, *y), (50.0 - geometry.0 as f64, 100.0 - geometry.1 as f64), "at the pointer's position, not where the menu was");
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()), "and gives it the keyboard");
}

#[test]
fn a_click_on_an_on_demand_layer_keeps_the_window_active() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let panel = map_panel(&mut c);
    let _a = map_titled(&mut c, "A");
    assert_eq!(active_ids(&comp), vec![1]);
    click(&comp, 960, 20);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(panel.wl_surface().clone()), "the panel takes the keyboard");
    assert_eq!(active_ids(&comp), vec![1], "windows/list: A stays activated");
    assert_eq!(foreign_active_titles(&mut c), vec!["A".to_string()], "foreign toplevel: A stays activated");
}

#[test]
fn an_exclusive_layer_keeps_the_window_active() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _a = map_titled(&mut c, "A");
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()), "the exclusive layer takes the keyboard");
    assert_eq!(active_ids(&comp), vec![1], "windows/list: A stays activated");
    assert_eq!(foreign_active_titles(&mut c), vec!["A".to_string()], "foreign toplevel: A stays activated");
    // A window mapped under the exclusive layer does not take the active state.
    let _b = map_titled(&mut c, "B");
    assert_eq!(active_ids(&comp), vec![1], "B mapped under the exclusive layer is not activated");
    c.destroy_layer(run);
    c.roundtrip();
    assert_eq!(active_ids(&comp), vec![2], "the keyboard goes to the topmost window, which becomes active");
}

#[test]
fn a_minimized_active_window_is_no_longer_active_and_the_next_one_is() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _a = map_titled(&mut c, "A");
    let _b = map_titled(&mut c, "B");
    assert_eq!(active_ids(&comp), vec![2]);
    let reply = ipc_request(&comp, "window/minimize", json!({"id": 2, "state": true}));
    assert_eq!(reply["ok"], true, "{reply}");
    c.roundtrip();
    assert_eq!(active_ids(&comp), vec![1], "B minimized: A, which gets the keyboard, is the active one");
    assert_eq!(foreign_active_titles(&mut c), vec!["A".to_string()]);
}

#[test]
fn a_window_minimized_while_the_panel_has_the_keyboard_is_no_longer_active() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let panel = map_panel(&mut c);
    let _a = map_titled(&mut c, "A");
    let _b = map_titled(&mut c, "B");
    click(&comp, 960, 20);
    c.roundtrip();
    assert_eq!(active_ids(&comp), vec![2], "B stays active after the panel click");
    let reply = ipc_request(&comp, "window/minimize", json!({"id": 2, "state": true}));
    assert_eq!(reply["ok"], true, "{reply}");
    c.roundtrip();
    assert!(!active_ids(&comp).contains(&2), "minimized B is no longer active");
    assert!(!foreign_active_titles(&mut c).contains(&"B".to_string()));
    // The existing focus logic leaves the keyboard on the panel, so no window takes over.
    assert_eq!(c.keyboard_focus(), Some(panel.wl_surface().clone()));
    let reply = ipc_request(&comp, "window/activate", json!({"id": 2}));
    assert_eq!(reply["ok"], true, "{reply}");
    c.roundtrip();
    assert_eq!(active_ids(&comp), vec![2], "restored B is the only active window");
}

#[test]
fn focusing_another_window_after_a_panel_click_makes_only_it_active() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _panel = map_panel(&mut c);
    let a = map_titled(&mut c, "A");
    let _b = map_titled(&mut c, "B");
    click(&comp, 960, 20);
    c.roundtrip();
    assert_eq!(active_ids(&comp), vec![2]);
    // Click on a part of A that B does not cover (B is cascaded below right of A).
    let reply = ipc_request(&comp, "windows/list", json!({}));
    let ga = reply["windows"].as_array().unwrap().iter().find(|w| w["id"] == 1).unwrap()["geometry"].clone();
    click(&comp, ga["x"].as_i64().unwrap() + 5, ga["y"].as_i64().unwrap() + 5);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
    assert_eq!(active_ids(&comp), vec![1], "only A is active");
    assert_eq!(foreign_active_titles(&mut c), vec!["A".to_string()]);
}

/// Switches to desktop `index` through the IPC and waits for the client.
fn switch_to(comp: &Compositor, c: &mut TestClient, index: u32) {
    let reply = ipc_request(comp, "desktops/switch", json!({"index": index}));
    assert_eq!(reply["ok"], true, "desktops/switch refused: {reply}");
    c.roundtrip();
}

#[test]
fn the_active_window_destroyed_while_the_panel_has_the_keyboard_leaves_no_active_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let panel = map_panel(&mut c);
    let _a = map_titled(&mut c, "A");
    let mut other = TestClient::new(&comp);
    let b = other.map_toplevel((400, 300));
    assert_eq!(active_ids(&comp), vec![2]);
    click(&comp, 960, 20);
    c.roundtrip();
    assert_eq!(active_ids(&comp), vec![2], "B stays active after the panel click");
    drop(b);
    drop(other); // the client of the active window dies
    let deadline = Instant::now() + Duration::from_secs(2);
    while activated_ids(&comp).len() != 1 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    c.roundtrip();
    assert_eq!(activated_ids(&comp), vec![(1, false)], "B is gone and A was not focused: no active window");
    assert!(foreign_active_titles(&mut c).is_empty());
    assert_eq!(c.keyboard_focus(), Some(panel.wl_surface().clone()), "the panel keeps the keyboard");
}

#[test]
fn a_desktop_switch_away_and_back_gives_the_top_window_the_keyboard() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _panel = map_panel(&mut c);
    let _a = map_titled(&mut c, "A");
    let b = map_titled(&mut c, "B");
    click(&comp, 960, 20);
    c.roundtrip();
    switch_to(&comp, &mut c, 1);
    assert!(active_ids(&comp).is_empty(), "nothing is shown on desktop 2");
    switch_to(&comp, &mut c, 0);
    assert_eq!(c.keyboard_focus(), Some(b.wl_surface().clone()), "back on desktop 1, the top window has the keyboard");
    assert_eq!(active_ids(&comp), vec![2]);
    assert_eq!(foreign_active_titles(&mut c), vec!["B".to_string()]);
}

#[test]
fn a_desktop_switch_activates_the_top_window_of_the_new_desktop() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _panel = map_panel(&mut c);
    let _a = map_titled(&mut c, "A");
    let b = map_titled(&mut c, "B");
    let reply = ipc_request(&comp, "window/move-to-desktop", json!({"id": 2, "desktop": 1}));
    assert_eq!(reply["ok"], true, "{reply}");
    click(&comp, 960, 20);
    c.roundtrip();
    switch_to(&comp, &mut c, 1);
    assert_eq!(c.keyboard_focus(), Some(b.wl_surface().clone()));
    assert_eq!(active_ids(&comp), vec![2]);
}

#[test]
fn a_desktop_switch_under_an_exclusive_layer_only_changes_the_active_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _a = map_titled(&mut c, "A");
    let _b = map_titled(&mut c, "B");
    let reply = ipc_request(&comp, "window/move-to-desktop", json!({"id": 2, "desktop": 1}));
    assert_eq!(reply["ok"], true, "{reply}");
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    switch_to(&comp, &mut c, 1);
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()), "the exclusive layer keeps the keyboard");
    assert_eq!(active_ids(&comp), vec![2], "B, the top window of desktop 2, is the active window");
    assert_eq!(foreign_active_titles(&mut c), vec!["B".to_string()]);
}

#[test]
fn a_switch_to_the_current_desktop_changes_nothing() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let panel = map_panel(&mut c);
    let _a = map_titled(&mut c, "A");
    let _b = map_titled(&mut c, "B");
    click(&comp, 960, 20);
    c.roundtrip();
    switch_to(&comp, &mut c, 0);
    assert_eq!(c.keyboard_focus(), Some(panel.wl_surface().clone()), "the panel keeps the keyboard");
    assert_eq!(active_ids(&comp), vec![2]);
}
