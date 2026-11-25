//! Integration tests of the private IPC on the headless backend.

mod common;

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

use common::ipc::{ipc_request, ipc_requests_then_half_close, ipc_send_raw, ipc_watch};
use common::{Compositor, HasSurface, TestClient};

/// Environment variable that turns `kill_helper_client` into a real client.
const HELPER_ENV: &str = "OPUSDM_COMPOSITOR_TEST_HELPER_SOCKET";

/// Line the helper prints once its window is mapped.
const HELPER_READY: &str = "HELPER_MAPPED";

/// Id of the only window in `windows/list`.
fn only_window_id(comp: &Compositor) -> u64 {
    let list = ipc_request(comp, "windows/list", json!({}));
    assert_eq!(list["windows"].as_array().map(Vec::len), Some(1), "one window expected: {list}");
    list["windows"][0]["id"].as_u64().expect("window id")
}

#[test]
fn the_ipc_lists_windows_and_minimizes_one() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let list = ipc_request(&comp, "windows/list", json!({}));
    let id = list["windows"][0]["id"].as_u64().unwrap();
    assert_eq!(list["windows"][0]["desktop"], 0);
    ipc_request(&comp, "window/minimize", json!({"id": id, "state": true}));
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert_eq!(list["windows"][0]["minimized"], true);
}

#[test]
fn a_window_moved_to_another_desktop_comes_back_when_activated() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let id = ipc_request(&comp, "windows/list", json!({}))["windows"][0]["id"].as_u64().unwrap();
    ipc_request(&comp, "window/move-to-desktop", json!({"id": id, "desktop": 2}));
    ipc_request(&comp, "window/activate", json!({"id": id}));
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["current"], 2);
}

#[test]
fn shrinking_the_desktops_keeps_every_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let id = ipc_request(&comp, "windows/list", json!({}))["windows"][0]["id"].as_u64().unwrap();
    ipc_request(&comp, "window/move-to-desktop", json!({"id": id, "desktop": 3}));
    assert_eq!(ipc_request(&comp, "desktops/set-count", json!({"count": 2}))["count"], 2);
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["desktop"], 1);
}

#[test]
fn a_watcher_sees_windows_come_and_go() {
    let comp = Compositor::start();
    let mut watch = ipc_watch(&comp);
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert_eq!(watch.next_event(Duration::from_secs(2)).unwrap()["event"], "window-added");
    drop(w);
    c.roundtrip();
    // window-changed events may come first; a window-removed must follow
    assert!(watch.wait_for("window-removed", Duration::from_secs(2)));
}

#[test]
fn a_connection_that_sends_garbage_is_dropped_and_the_server_goes_on() {
    let comp = Compositor::start();
    assert!(ipc_send_raw(&comp, &(u32::MAX).to_le_bytes()), "a length over IPC_MAX_MESSAGE closes the connection");
    assert!(ipc_send_raw(&comp, b"\x05\x00\x00\x00hello"), "a body that is not JSON closes the connection");
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
}

#[test]
fn session_quit_ends_the_compositor_cleanly() {
    let mut comp = Compositor::start();
    assert!(comp.ipc_socket.exists());
    assert_eq!(ipc_request(&comp, "session/quit", json!({}))["ok"], true);
    assert_eq!(comp.wait_exit(Duration::from_secs(2)), Some(0));
    assert!(!comp.ipc_socket.exists(), "the IPC socket file must go with the loop");
}

#[test]
fn sigterm_removes_the_ipc_socket() {
    let mut comp = Compositor::start();
    comp.signal(libc::SIGTERM);
    assert_eq!(comp.wait_exit(Duration::from_secs(2)), Some(0));
    assert!(!comp.ipc_socket.exists(), "the IPC socket file must go with the loop");
}

#[test]
fn a_window_on_another_desktop_is_hidden_and_comes_back_on_switch() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((300, 200));
    assert!(c.wait_keyboard_focus(&b, Duration::from_secs(2)));
    let list = ipc_request(&comp, "windows/list", json!({}));
    let windows = list["windows"].as_array().expect("windows");
    assert_eq!(windows.len(), 2);
    // Stack order: the last mapped window is the last one.
    let a_id = windows[0]["id"].as_u64().unwrap();
    let b_id = windows[1]["id"].as_u64().unwrap();
    assert_eq!(windows[1]["activated"], true);
    assert_eq!(windows[0]["activated"], false);

    assert_eq!(ipc_request(&comp, "window/move-to-desktop", json!({"id": b_id, "desktop": 1}))["ok"], true);
    assert!(c.wait_keyboard_focus(&a, Duration::from_secs(2)), "the shown window gets the keyboard");
    let list = ipc_request(&comp, "windows/list", json!({}));
    let windows = list["windows"].as_array().expect("windows");
    // Focusing `a` raised it: it is the last one of the stack now.
    assert_eq!(windows[1]["id"].as_u64(), Some(a_id));
    assert_eq!(windows[1]["activated"], true);
    let b_view = windows.iter().find(|w| w["id"] == b_id).expect("b is still listed");
    assert_eq!((b_view["desktop"].clone(), b_view["activated"].clone()), (json!(1), json!(false)));

    assert_eq!(ipc_request(&comp, "desktops/switch", json!({"index": 1}))["ok"], true);
    assert!(c.wait_keyboard_focus(&b, Duration::from_secs(2)), "the window of desktop 1 comes back with the keyboard");
    assert!(c.wait_frame(&b, Duration::from_secs(2)));

    assert_eq!(ipc_request(&comp, "desktops/switch", json!({"index": 0}))["ok"], true);
    assert!(c.wait_keyboard_focus(&a, Duration::from_secs(2)));
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
}

#[test]
fn a_watcher_sees_minimize_and_desktop_changes() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let id = only_window_id(&comp);
    let mut watch = ipc_watch(&comp);
    ipc_request(&comp, "window/minimize", json!({"id": id, "state": true}));
    let changed = watch.wait_for_event("window-changed", Duration::from_secs(2)).expect("window-changed");
    assert_eq!(changed["window"]["id"], id);
    assert_eq!(changed["window"]["minimized"], true);
    ipc_request(&comp, "desktops/switch", json!({"index": 3}));
    let desktops = watch.wait_for_event("desktops-changed", Duration::from_secs(2)).expect("desktops-changed");
    assert_eq!((desktops["count"].clone(), desktops["current"].clone()), (json!(4), json!(3)));
}

#[test]
fn the_window_view_has_the_documented_fields() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert_eq!(list["ok"], true);
    let w = &list["windows"][0];
    assert_eq!(w["title"], "opusdm-compositor test");
    assert_eq!(w["app_id"], "");
    assert_eq!(w["pid"], u64::from(std::process::id()), "the test client is this process");
    assert_eq!((w["geometry"]["width"].clone(), w["geometry"]["height"].clone()), (json!(400), json!(300)));
    assert!(w["geometry"]["x"].is_i64() && w["geometry"]["y"].is_i64());
    for flag in ["minimized", "maximized", "fullscreen"] {
        assert_eq!(w[flag], false, "{flag}");
    }
}

#[test]
fn refusals_are_error_replies() {
    let comp = Compositor::start();
    let cases = [
        ("no/such-method", json!({})),
        ("window/activate", json!({"id": 99})),
        ("window/activate", json!({})),
        ("window/minimize", json!({"id": 1})),
        ("window/close", json!({"id": "x"})),
        ("window/kill", json!({"id": 99})),
        ("window/move-to-desktop", json!({"id": 99, "desktop": 0})),
        ("desktops/switch", json!({"index": 4})),
        ("desktops/set-count", json!({})),
        ("debug/key", json!({"key": "Ctrl+Bogus+u"})),
        ("debug/key", json!({"key": "Ctrl+NoSuchKeysym"})),
    ];
    for (method, data) in cases {
        let reply = ipc_request(&comp, method, data.clone());
        assert!(reply["error"].is_string(), "{method} {data} must be refused: {reply}");
        assert!(reply.get("ok").is_none(), "{method} {data}: {reply}");
    }
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
}

#[test]
fn window_close_asks_the_client_to_close() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let id = only_window_id(&comp);
    assert_eq!(ipc_request(&comp, "window/close", json!({"id": id}))["ok"], true);
    assert!(c.wait_close_request(&w, Duration::from_secs(2)), "the client got xdg_toplevel.close");
}

#[test]
fn closed_connections_give_back_their_fds_and_the_server_keeps_answering() {
    let comp = Compositor::start();
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
    // Let the sources of that first connection go (a few loop iterations).
    std::thread::sleep(Duration::from_millis(200));
    let baseline = comp.open_fds();
    // More than IPC_MAX_CONNECTIONS (64): a leaked connection would make the server refuse the rest.
    for _ in 0..150 {
        assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
    }
    for _ in 0..30 {
        assert!(ipc_send_raw(&comp, &(u32::MAX).to_le_bytes()));
        assert!(ipc_send_raw(&comp, b"\x02\x00\x00\x00{]"));
    }
    let watch = ipc_watch(&comp);
    drop(watch);
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
    // The sources of the last connection go away on the next loop iterations.
    let deadline = Instant::now() + Duration::from_secs(2);
    while comp.open_fds() > baseline && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(comp.open_fds(), baseline, "every closed connection gave its fds back");
}

#[test]
fn debug_key_reaches_the_focused_window_through_the_keyboard() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    assert_eq!(ipc_request(&comp, "debug/key", json!({"key": "Ctrl+Alt+u"}))["ok"], true);
    assert!(c.wait_keys(2, Duration::from_secs(2)), "press and release of u, got {}", c.keys_received());
    c.roundtrip();
    assert_eq!(c.keys_received(), 2, "modifiers are not counted, nothing else is sent");
}

/// Kills (SIGKILL) and reaps the helper process this test spawned, when the
/// test ends early.
struct HelperGuard(Child);

impl Drop for HelperGuard {
    /// Kills and reaps the helper (a no-op error when it is already gone).
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn window_kill_kills_the_client_process_and_nothing_else() {
    let comp = Compositor::start();
    let exe = std::env::current_exe().expect("test binary path");
    let child = Command::new(exe)
        .args(["--exact", "kill_helper_client", "--ignored", "--nocapture", "--test-threads=1"])
        .env(HELPER_ENV, &comp.socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the helper client");
    let mut helper = HelperGuard(child);
    let stdout = helper.0.stdout.take().expect("helper stdout");
    let mut lines = BufReader::new(stdout).lines();
    // libtest prints "test kill_helper_client ... " on the same line first.
    let ready = lines.any(|l| l.is_ok_and(|l| l.trim_end().ends_with(HELPER_READY)));
    assert!(ready, "the helper never mapped its window");
    let helper_pid = helper.0.id();
    assert_ne!(helper_pid, std::process::id());
    let list = ipc_request(&comp, "windows/list", json!({}));
    let windows = list["windows"].as_array().expect("windows");
    let target = windows.iter().find(|w| w["pid"] == u64::from(helper_pid)).expect("the helper window, by pid");
    assert_eq!(ipc_request(&comp, "window/kill", json!({"id": target["id"]}))["ok"], true);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = helper.0.try_wait().expect("wait helper") { break status; }
        assert!(Instant::now() < deadline, "the helper was not killed");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert!(list["windows"].as_array().is_some_and(|w| w.iter().all(|w| w["pid"] != u64::from(helper_pid))));
}

/// Not a test of its own: the client process of
/// `window_kill_kills_the_client_process_and_nothing_else`. With the helper
/// variable set it maps a window, prints `HELPER_READY` and waits to be
/// killed (at most 30 s); without it it returns at once.
#[test]
#[ignore = "helper process of window_kill_kills_the_client_process_and_nothing_else"]
fn kill_helper_client() {
    let Some(socket) = std::env::var_os(HELPER_ENV) else { return };
    let mut c = TestClient::connect_to(&PathBuf::from(socket));
    let _w = c.map_toplevel((200, 100));
    println!("{HELPER_READY}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        c.roundtrip();
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_client_that_half_closes_after_its_request_gets_the_reply() {
    let comp = Compositor::start();
    let replies = ipc_requests_then_half_close(&comp, "desktops/list", 1, Duration::from_secs(2));
    assert_eq!(replies.len(), 1, "one reply after shutdown(Write)");
    assert_eq!(replies[0]["count"], 4);
}

#[test]
fn a_client_that_half_closes_gets_every_reply_even_past_the_socket_buffer() {
    let comp = Compositor::start();
    std::thread::sleep(Duration::from_millis(200));
    let baseline = comp.open_fds();
    // About 3 MiB of replies (under IPC_MAX_PENDING): far more than the socket buffer, so some are still pending at EOF.
    let count = 100_000;
    let replies = ipc_requests_then_half_close(&comp, "desktops/list", count, Duration::from_secs(10));
    assert_eq!(replies.len(), count, "every reply reaches a client that half-closed");
    assert!(replies.iter().all(|r| r["count"] == 4));
    // Once flushed the connection is removed and its fds go back.
    let deadline = Instant::now() + Duration::from_secs(2);
    while comp.open_fds() > baseline && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(comp.open_fds(), baseline, "the half-closed connection gave its fds back");
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
}

/// The `{"name","key"}` entries of a `bindings/list` reply.
fn listed_bindings(comp: &Compositor) -> Vec<serde_json::Value> {
    let reply = ipc_request(comp, "bindings/list", json!({}));
    assert_eq!(reply["ok"], true, "{reply}");
    reply["bindings"].as_array().cloned().unwrap_or_else(|| panic!("no bindings array: {reply}"))
}

#[test]
fn bindings_list_has_the_default_built_ins() {
    let comp = Compositor::start();
    let bindings = listed_bindings(&comp);
    assert!(bindings.contains(&json!({"name": "close", "key": "Alt+F4"})), "{bindings:?}");
    assert!(bindings.contains(&json!({"name": "quit", "key": "Ctrl+Alt+BackSpace"})), "{bindings:?}");
    assert_eq!(bindings.len(), 20, "{bindings:?}");
}

#[test]
fn reload_reads_compositor_json_and_desktop_json_again() {
    let comp = Compositor::start();
    comp.write_config("compositor.json", r#"{"close": "Super+q", "switcher_all": "", "lowerr": "x"}"#);
    comp.write_config("desktop.json", r#"{"virtual_screens": 6}"#);
    let reply = ipc_request(&comp, "reload", json!({}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["errors"], json!(["unknown key 'lowerr' ignored"]), "{reply}");
    let bindings = listed_bindings(&comp);
    assert!(bindings.contains(&json!({"name": "close", "key": "Super+q"})), "{bindings:?}");
    assert!(!bindings.iter().any(|b| b["name"] == "switcher_all"), "an empty key disables it: {bindings:?}");
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 6);
}

#[test]
fn reload_keeps_the_hub_hotkeys() {
    let comp = Compositor::start();
    let (mut hub, reply) = common::ipc::ipc_hub(&comp, json!({"hotkeys": [{"key": "Ctrl+backslash", "function": "terminal"}]}));
    assert_eq!(reply["errors"], json!([]), "{reply}");
    comp.write_config("compositor.json", r#"{"close": "Super+q"}"#);
    assert_eq!(ipc_request(&comp, "reload", json!({}))["errors"], json!([]));
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"}));
    assert!(hub.wait_for_hotkey("terminal", Duration::from_secs(2)), "the hub hotkey still fires after reload");
}

#[test]
fn reload_with_a_broken_compositor_json_reports_it_and_uses_the_defaults() {
    let comp = Compositor::start();
    comp.write_config("compositor.json", "not json");
    let reply = ipc_request(&comp, "reload", json!({}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["errors"].as_array().map(Vec::len), Some(1), "{reply}");
    assert!(listed_bindings(&comp).contains(&json!({"name": "close", "key": "Alt+F4"})));
}

#[test]
fn reload_reports_two_built_ins_on_the_same_key() {
    let comp = Compositor::start();
    comp.write_config("compositor.json", r#"{"lower": "Alt+F4"}"#);
    let reply = ipc_request(&comp, "reload", json!({}));
    let error = "compositor.json lower 'Alt+F4': same key as close, close wins";
    assert_eq!(reply["errors"], json!([error]), "{reply}");
    assert!(comp.wait_log(error, Duration::from_secs(2)));
}

#[test]
fn reload_reports_a_hub_hotkey_shadowed_by_a_built_in_and_gives_it_back_later() {
    let comp = Compositor::start();
    let (mut hub, reply) = common::ipc::ipc_hub(&comp, json!({"hotkeys": [{"key": "Super+q", "function": "run"}]}));
    assert_eq!(reply["errors"], json!([]), "{reply}");
    comp.write_config("compositor.json", r#"{"close": "Super+q"}"#);
    let reply = ipc_request(&comp, "reload", json!({}));
    assert_eq!(reply["errors"], json!(["hotkey Super+q: run conflicts with the built-in close, the built-in wins"]), "{reply}");
    assert!(comp.wait_log("hotkey Super+q: run conflicts with the built-in close, the built-in wins", Duration::from_secs(2)));
    ipc_request(&comp, "debug/key", json!({"key": "Super+q"}));
    assert!(!hub.wait_for_hotkey("run", Duration::from_millis(300)), "the built-in wins");
    comp.write_config("compositor.json", "{}");
    assert_eq!(ipc_request(&comp, "reload", json!({}))["errors"], json!([]));
    ipc_request(&comp, "debug/key", json!({"key": "Super+q"}));
    assert!(hub.wait_for_hotkey("run", Duration::from_secs(2)), "the hub entry is back once the built-in moved away");
}

#[test]
fn reload_with_a_broken_desktop_json_keeps_the_desktops_and_the_wm_settings() {
    let comp = Compositor::start();
    comp.write_config("desktop.json", r#"{"virtual_screens": 6, "wrap_desktops": false}"#);
    assert_eq!(ipc_request(&comp, "reload", json!({}))["errors"], json!([]));
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 6);
    comp.write_config("desktop.json", "not json");
    let reply = ipc_request(&comp, "reload", json!({}));
    assert_eq!(reply["ok"], true, "{reply}");
    let errors = reply["errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(errors.len(), 1, "{reply}");
    assert!(errors[0].as_str().is_some_and(|e| e.contains("desktop.json")), "{reply}");
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 6, "the desktop count is kept");
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+Alt+Left"}));
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["current"], 0, "wrap_desktops false is kept");
    // A desktop.json that exists but cannot be read (a directory) is kept as well.
    let path = comp.config_path("desktop.json");
    std::fs::remove_file(&path).expect("remove desktop.json");
    std::fs::create_dir(&path).expect("desktop.json as a directory");
    let reply = ipc_request(&comp, "reload", json!({}));
    assert_eq!(reply["errors"].as_array().map(Vec::len), Some(1), "{reply}");
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 6);
    // A missing desktop.json gives the defaults, as at startup.
    std::fs::remove_dir(&path).expect("remove the directory");
    assert_eq!(ipc_request(&comp, "reload", json!({}))["errors"], json!([]));
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+Alt+Left"}));
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["current"], 3, "the default wrap is back");
}

/// `(id, opacity)` of every window of `windows/list`, in stack order.
fn opacities(comp: &Compositor) -> Vec<(u64, u64)> {
    let list = ipc_request(comp, "windows/list", json!({}));
    let windows = list["windows"].as_array().cloned().unwrap_or_else(|| panic!("no windows array: {list}"));
    windows.iter().map(|w| (w["id"].as_u64().expect("id"), w["opacity"].as_u64().unwrap_or_else(|| panic!("no opacity: {w}")))).collect()
}

#[test]
fn the_inactive_window_reports_88_and_the_active_one_100() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&b, Duration::from_secs(2)));
    assert_eq!(opacities(&comp), vec![(1, 88), (2, 100)]);
}

#[test]
fn window_set_opacity_overrides_and_null_clears() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&b, Duration::from_secs(2)));
    assert_eq!(ipc_request(&comp, "window/set-opacity", json!({"id": 2, "opacity": 50})), json!({"ok": true}));
    assert_eq!(opacities(&comp), vec![(1, 88), (2, 50)], "the override wins on the active window");
    assert_eq!(ipc_request(&comp, "window/set-opacity", json!({"id": 2, "opacity": null})), json!({"ok": true}));
    assert_eq!(opacities(&comp), vec![(1, 88), (2, 100)]);
    for bad in [
        json!({"id": 2, "opacity": 19}),
        json!({"id": 2, "opacity": 101}),
        json!({"id": 2, "opacity": "50"}),
        json!({"id": 2}),
        json!({"id": 99, "opacity": 50}),
    ] {
        let reply = ipc_request(&comp, "window/set-opacity", bad.clone());
        assert!(reply["error"].is_string(), "{bad}: {reply}");
    }
    assert_eq!(opacities(&comp), vec![(1, 88), (2, 100)], "refused requests change nothing");
}

#[test]
fn a_watcher_sees_the_opacity_change() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _a = c.map_toplevel((400, 300));
    let mut watch = ipc_watch(&comp);
    ipc_request(&comp, "window/set-opacity", json!({"id": 1, "opacity": 40}));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = watch.wait_for_event("window-changed", left).expect("a window-changed event with opacity 40");
        if event["window"]["opacity"] == 40 { break; }
    }
}

#[test]
fn reload_with_another_inactive_opacity_changes_the_reported_opacity() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&b, Duration::from_secs(2)));
    assert_eq!(opacities(&comp), vec![(1, 88), (2, 100)]);
    comp.write_config("desktop.json", r#"{"inactive_opacity": 60}"#);
    assert_eq!(ipc_request(&comp, "reload", json!({}))["errors"], json!([]));
    assert_eq!(opacities(&comp), vec![(1, 60), (2, 100)]);
    comp.write_config("desktop.json", r#"{"transparency_enabled": false}"#);
    assert_eq!(ipc_request(&comp, "reload", json!({}))["errors"], json!([]));
    assert_eq!(opacities(&comp), vec![(1, 100), (2, 100)]);
}

#[test]
fn window_set_geometry_moves_and_resizes_the_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let reply = ipc_request(&comp, "window/set-geometry", json!({"id": 1, "x": 100, "y": 120, "width": 640, "height": 480}));
    assert_eq!(reply, json!({"ok": true}));
    assert!(c.wait_configure(&w, (640, 480), Duration::from_secs(2)), "configure {:?}", c.last_configure(&w));
    c.roundtrip();
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert_eq!(list["windows"][0]["geometry"], json!({"x": 100, "y": 120, "width": 640, "height": 480}), "{list}");
}

#[test]
fn window_set_geometry_unmaximizes_the_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_maximized();
    assert!(c.wait_configure(&w, (1920, 1080), Duration::from_secs(2)));
    c.roundtrip();
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["maximized"], true);
    ipc_request(&comp, "window/set-geometry", json!({"id": 1, "x": 10, "y": 20, "width": 500, "height": 400}));
    assert!(c.wait_configure(&w, (500, 400), Duration::from_secs(2)), "configure {:?}", c.last_configure(&w));
    c.roundtrip();
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert_eq!(list["windows"][0]["maximized"], false, "{list}");
    assert_eq!(list["windows"][0]["geometry"], json!({"x": 10, "y": 20, "width": 500, "height": 400}), "{list}");
}

#[test]
fn window_set_geometry_refuses_bad_requests() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    for bad in [
        json!({"id": 99, "x": 0, "y": 0, "width": 100, "height": 100}),
        json!({"id": 1, "x": 0, "y": 0, "width": 0, "height": 100}),
        json!({"id": 1, "x": 0, "y": 0, "width": 100}),
        json!({"id": 1, "x": "a", "y": 0, "width": 100, "height": 100}),
    ] {
        let reply = ipc_request(&comp, "window/set-geometry", bad.clone());
        assert!(reply["error"].is_string(), "{bad}: {reply}");
    }
}
