//! GTK checks of the applets host. GTK may be used from one thread only, so
//! every check is a section of the single `#[test]` below (this file is its
//! own test process). Windows are built but never presented
//! (`present_windows: false`): nothing appears on the screen.

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gio, glib};
use opusdm_applets::applet_window::{self, AppletWindow, WindowHooks};
use opusdm_applets::edit_mode::EditMode;
use opusdm_applets::host::{self, Host, HostConfig};
use opusdm_applets::placement::Rect;
use opusdm_applets::protocol::{request_at, AppletStatus, Reply, Request};
use opusdm_applets::server::{self, Shared};
use opusdm_applets::settings_form;
use opusdm_applets::state::{self, AppletState};
use std::path::Path;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Temp folders of one host: applets and state.
struct Dirs {
    applets: tempfile::TempDir,
    states: tempfile::TempDir,
}

impl Dirs {
    /// Fresh, empty folders.
    fn new() -> Dirs {
        Dirs { applets: tempfile::tempdir().unwrap(), states: tempfile::tempdir().unwrap() }
    }

    /// Write applet `id` with the manifest `manifest` and the script `code`.
    fn applet(&self, id: &str, manifest: &str, code: &str) {
        let dir = self.applets.path().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("applet.json"), manifest).unwrap();
        std::fs::write(dir.join("main.js"), code).unwrap();
    }

    /// Mark applet `id` enabled in its state file.
    fn enable(&self, id: &str) {
        state::save_in(self.states.path(), id, &AppletState { enabled: true, ..Default::default() }).unwrap();
    }

    /// A host over these folders that never presents a window.
    fn host(&self, app: &gtk::Application) -> Rc<Host> {
        let config = HostConfig { applets_dir: self.applets.path().to_path_buf(), state_dir: self.states.path().to_path_buf(), present_windows: false };
        Host::with_config(app, config)
    }
}

/// A manifest named `name`.
fn manifest(name: &str) -> String {
    format!(r#"{{ "name": "{name}", "version": "1.0", "description": "test applet", "window": {{ "width": 210, "height": 120 }} }}"#)
}

/// A script that shows one label with `text`.
fn label_script(text: &str) -> String {
    format!(r#"opus.ui.show({{ type: "label", id: "l", text: "{text}" }});"#)
}

/// The `List` entries of `host`.
fn list(host: &Host) -> Vec<AppletStatus> {
    match host.handle(&Request::List) {
        Reply::Applets(list) => list,
        other => panic!("List answered {other:?}"),
    }
}

/// The `List` entry of `id`.
fn status(host: &Host, id: &str) -> AppletStatus {
    list(host).into_iter().find(|s| s.id == id).unwrap_or_else(|| panic!("{id} is not listed"))
}

/// Run pending main-loop work without blocking.
fn pump() {
    let ctx = glib::MainContext::default();
    while ctx.iteration(false) {}
}

/// Iterate the main loop until `done` holds, at most `limit`.
fn pump_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let ctx = glib::MainContext::default();
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if done() { return true; }
        if !ctx.iteration(false) { std::thread::sleep(Duration::from_millis(1)); }
    }
    done()
}

/// The first widget of type `T` under `root`, depth first.
fn find<T: IsA<gtk::Widget>>(root: &gtk::Widget) -> Option<T> {
    if let Ok(found) = root.clone().downcast::<T>() { return Some(found); }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find::<T>(&c) { return Some(found); }
        child = c.next_sibling();
    }
    None
}

/// The text of the first label of applet `id`'s content.
fn label_text(host: &Host, id: &str) -> Option<String> {
    let root = host.content_root(id)?;
    find::<gtk::Label>(&root).map(|l| l.text().to_string())
}

/// The window of `id` exists and was never presented.
fn assert_hidden_window(host: &Host, id: &str) {
    let window = host.window_of(id).unwrap_or_else(|| panic!("{id} has no window"));
    assert!(!window.is_visible(), "{id}: a test window must never be presented");
    assert!(!window.is_decorated(), "{id}: the applet window is undecorated");
}

#[test]
fn host_gtk_checks() {
    if let Err(e) = gtk::init() {
        let display = std::env::var_os("DISPLAY").filter(|d| !d.is_empty());
        let wayland = std::env::var_os("WAYLAND_DISPLAY").filter(|d| !d.is_empty());
        if display.is_some() || wayland.is_some() { panic!("gtk::init() failed although a display is set (DISPLAY={display:?}, WAYLAND_DISPLAY={wayland:?}): {e}"); }
        eprintln!("host_gtk_checks: SKIPPED, no display (DISPLAY and WAYLAND_DISPLAY unset)");
        return;
    }
    let app = gtk::Application::builder().application_id("org.opusdm.AppletsTest").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.register(None::<&gio::Cancellable>).expect("register the test application");
    set_enabled_starts_and_stops_the_runtime(&app);
    a_broken_applet_shows_up_in_list_with_its_error_and_others_run(&app);
    the_applet_window_shows_the_script_error(&app);
    a_bad_show_clears_the_old_widgets_and_shows_the_error(&app);
    reload_restarts_a_changed_script(&app);
    the_host_still_answers_after_many_main_loop_iterations(&app);
    a_disabled_applet_stops_its_timers(&app);
    a_click_that_shows_a_new_tree_replaces_it(&app);
    a_window_close_disables_the_applet(&app);
    prepare_on_a_window_without_an_x11_surface_does_not_panic(&app);
    the_host_saves_a_changed_geometry_once(&app);
    an_unreadable_state_file_is_never_overwritten(&app);
    a_monitor_change_clamps_the_open_windows(&app);
    the_icons_are_hidden_outside_edit_mode(&app);
    edit_mode_shows_the_border_and_the_icons(&app);
    setting_edit_off_hides_them_again(&app);
    the_handle_drag_moves_the_window_like_alt_drag(&app);
    a_window_opened_while_edit_mode_is_on_starts_in_edit_mode(&app);
    the_form_shows_the_current_values(&app);
    apply_saves_the_state_and_calls_on_settings(&app);
    an_invalid_value_is_shown_in_the_window_and_not_saved(&app);
    apply_with_an_unreadable_state_file_writes_nothing(&app);
    an_applet_with_no_settings_says_so_in_the_window(&app);
    the_gear_twice_opens_one_window(&app);
    disabling_the_applet_closes_its_settings_window(&app);
    eprintln!("host_gtk_checks: all sections ran");
}

/// set_enabled_starts_and_stops_the_runtime
fn set_enabled_starts_and_stops_the_runtime(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("clock", &manifest("Clock"), &label_script("tick"));
    let host = dirs.host(app);
    host.load_enabled();
    let st = status(&host, "clock");
    assert_eq!((st.name.as_deref(), st.enabled, st.running, st.error.as_deref()), (Some("Clock"), false, false, None));
    assert_eq!((st.version.as_str(), st.description.as_str()), ("1.0", "test applet"));
    assert!(host.window_of("clock").is_none(), "a disabled applet has no window");

    assert_eq!(host.handle(&Request::SetEnabled { id: "clock".into(), enabled: true }), Reply::Ok);
    assert!(state::load_in(dirs.states.path(), "clock").enabled, "enabled is saved");
    let st = status(&host, "clock");
    assert!(st.enabled && st.running, "{st:?}");
    assert_hidden_window(&host, "clock");
    let window = host.window_of("clock").unwrap();
    assert_eq!(window.title().as_deref(), Some("Clock"));
    assert_eq!(window.default_size(), (210, 120));
    assert!(!window.is_resizable());
    assert_eq!(label_text(&host, "clock").as_deref(), Some("tick"));
    assert!(app.windows().contains(&window.clone().upcast()), "the window belongs to the application");

    // Enabling again does not restart it.
    assert_eq!(host.handle(&Request::SetEnabled { id: "clock".into(), enabled: true }), Reply::Ok);
    assert_eq!(host.window_of("clock").as_ref(), Some(&window));

    assert_eq!(host.handle(&Request::SetEnabled { id: "clock".into(), enabled: false }), Reply::Ok);
    assert!(!state::load_in(dirs.states.path(), "clock").enabled, "disabled is saved");
    let st = status(&host, "clock");
    assert!(!st.enabled && !st.running, "{st:?}");
    assert!(host.window_of("clock").is_none());
    assert!(!app.windows().contains(&window.upcast()), "the window is destroyed");

    match host.handle(&Request::SetEnabled { id: "nope".into(), enabled: true }) {
        Reply::Error(e) => assert!(e.contains("nope"), "{e}"),
        other => panic!("unknown id answered {other:?}"),
    }
    match host.handle(&Request::Reload { id: "nope".into() }) {
        Reply::Error(e) => assert!(e.contains("nope"), "{e}"),
        other => panic!("unknown id answered {other:?}"),
    }
    assert!(state::load_in(dirs.states.path(), "nope") == AppletState::default(), "no state written for an unknown id");

    // Saved geometry wins over the manifest size.
    state::save_in(dirs.states.path(), "clock", &AppletState { enabled: true, width: Some(333), height: Some(77), ..Default::default() }).unwrap();
    assert_eq!(host.handle(&Request::Reload { id: "clock".into() }), Reply::Ok);
    assert_eq!(host.window_of("clock").unwrap().default_size(), (333, 77));
    // The host owns the windows: dropping it destroys them.
    let window = host.window_of("clock").unwrap();
    assert!(Rc::strong_count(&host) == 1, "nothing else owns the host");
    drop(host);
    assert!(!app.windows().contains(&window.upcast()), "dropping the host destroys its windows");
    eprintln!("section set_enabled_starts_and_stops_the_runtime: ok");
}

/// a_broken_applet_shows_up_in_list_with_its_error_and_others_run
fn a_broken_applet_shows_up_in_list_with_its_error_and_others_run(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("bad", "{ not json", "");
    dirs.applet("good", &manifest("Good"), &label_script("fine"));
    dirs.enable("bad");
    dirs.enable("good");
    let host = dirs.host(app);
    host.load_enabled();
    let all = list(&host);
    assert_eq!(all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["bad", "good"]);
    let bad = &all[0];
    assert_eq!((bad.name.as_deref(), bad.enabled, bad.running), (None, true, false));
    assert!(bad.error.as_deref().is_some_and(|e| e.contains("applet.json")), "{bad:?}");
    assert!(host.window_of("bad").is_none(), "no window without a manifest");
    let good = &all[1];
    assert!(good.running && good.error.is_none(), "{good:?}");
    assert_eq!(label_text(&host, "good").as_deref(), Some("fine"));
    match host.handle(&Request::Reload { id: "bad".into() }) {
        Reply::Error(e) => assert!(e.contains("applet.json"), "{e}"),
        other => panic!("reload of a broken applet answered {other:?}"),
    }
    eprintln!("section a_broken_applet_shows_up_in_list_with_its_error_and_others_run: ok");
}

/// the_applet_window_shows_the_script_error
fn the_applet_window_shows_the_script_error(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("boom", &manifest("Boom"), r#"throw new Error("kaboom at start");"#);
    dirs.applet("later", &manifest("Later"), r#"opus.ui.show({ type: "label", id: "l", text: "x" }); setTimeout(() => opus.ui.update("missing", { text: "y" }), 1);"#);
    dirs.enable("boom");
    dirs.enable("later");
    let host = dirs.host(app);
    host.load_enabled();
    assert_hidden_window(&host, "boom");
    let shown = host.error_text("boom").expect("the start error is shown");
    assert!(shown.contains("kaboom at start"), "{shown}");
    let st = status(&host, "boom");
    assert!(!st.running && st.enabled, "{st:?}");
    assert!(st.error.as_deref().is_some_and(|e| e.contains("kaboom at start")), "{st:?}");
    assert!(host.error_text("later").is_none(), "no error before the timer");
    assert!(pump_until(Duration::from_secs(3), || host.error_text("later").is_some()), "the update error reaches the window");
    assert!(host.error_text("later").unwrap().contains("missing"));
    assert_eq!(label_text(&host, "later").as_deref(), Some("x"), "a bad update keeps the tree");
    match host.handle(&Request::SetEnabled { id: "boom".into(), enabled: true }) {
        Reply::Ok => {}
        other => panic!("enabling an already open applet answered {other:?}"),
    }
    eprintln!("section the_applet_window_shows_the_script_error: ok");
}

/// Task 6 review: a show that fails must not leave the old (forgotten)
/// widgets clickable.
fn a_bad_show_clears_the_old_widgets_and_shows_the_error(app: &gtk::Application) {
    let dirs = Dirs::new();
    let code = r#"opus.ui.show({ type: "button", id: "b", label: "go", onClick: () => {} });
setTimeout(() => opus.ui.show({ type: "label", id: "a", text: "1", nonsense: true }), 1);"#;
    dirs.applet("swap", &manifest("Swap"), code);
    dirs.enable("swap");
    let host = dirs.host(app);
    host.load_enabled();
    let root = host.content_root("swap").expect("the first tree is shown");
    assert!(find::<gtk::Button>(&root).is_some());
    assert!(pump_until(Duration::from_secs(3), || host.error_text("swap").is_some()), "the bad show is reported");
    assert!(host.content_root("swap").is_none(), "the old widgets are removed");
    assert!(host.error_text("swap").unwrap().contains("nonsense"));
    assert!(status(&host, "swap").running, "a bad show does not stop the runtime");
    eprintln!("section a_bad_show_clears_the_old_widgets_and_shows_the_error: ok");
}

/// reload_restarts_a_changed_script
fn reload_restarts_a_changed_script(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("note", &manifest("Note"), &label_script("one"));
    dirs.enable("note");
    let host = dirs.host(app);
    host.load_enabled();
    assert_eq!(label_text(&host, "note").as_deref(), Some("one"));
    std::fs::write(dirs.applets.path().join("note/main.js"), label_script("two")).unwrap();
    std::fs::write(dirs.applets.path().join("note/applet.json"), manifest("Note Two")).unwrap();
    assert_eq!(host.handle(&Request::Reload { id: "note".into() }), Reply::Ok);
    assert_eq!(label_text(&host, "note").as_deref(), Some("two"));
    assert_eq!(host.window_of("note").unwrap().title().as_deref(), Some("Note Two"));
    assert_eq!(status(&host, "note").name.as_deref(), Some("Note Two"));
    // A reload that now throws shows the error and reports it.
    std::fs::write(dirs.applets.path().join("note/main.js"), "throw new Error('broken now');").unwrap();
    match host.handle(&Request::Reload { id: "note".into() }) {
        Reply::Error(e) => assert!(e.contains("broken now"), "{e}"),
        other => panic!("a failing reload answered {other:?}"),
    }
    assert!(host.error_text("note").unwrap().contains("broken now"));
    assert!(!status(&host, "note").running);
    // A reload of a disabled applet does not start it.
    assert_eq!(host.handle(&Request::SetEnabled { id: "note".into(), enabled: false }), Reply::Ok);
    std::fs::write(dirs.applets.path().join("note/main.js"), label_script("three")).unwrap();
    assert_eq!(host.handle(&Request::Reload { id: "note".into() }), Reply::Ok);
    assert!(host.window_of("note").is_none());
    eprintln!("section reload_restarts_a_changed_script: ok");
}

/// Rule 5: the host, the window and the handlers are still alive after the
/// first applet loaded and after many main-context iterations, and requests
/// through the real socket still get answers.
fn the_host_still_answers_after_many_main_loop_iterations(app: &gtk::Application) {
    let dirs = Dirs::new();
    let code = r##"let n = 0, ticks = 0;
opus.ui.show({ type: "box", children: [
  { type: "label", id: "l", text: "n=0" },
  { type: "button", id: "b", label: "inc", onClick: () => { n++; opus.ui.update("l", { text: "n=" + n }); } },
  { type: "label", id: "t", text: "ticks=0" },
  { type: "canvas", id: "c", width: 20, height: 20, draw: (ctx) => { ctx.fillStyle = "#0f0"; ctx.fillRect(0, 0, 5, 5); } },
] });
setInterval(() => { ticks++; opus.ui.update("t", { text: "ticks=" + ticks }); }, 4);"##;
    dirs.applet("live", &manifest("Live"), code);
    dirs.enable("live");
    let host = dirs.host(app);
    let sock_dir = tempfile::Builder::new().prefix("oa8c").tempdir_in("/tmp").unwrap();
    let sock = sock_dir.path().join("a.sock");
    let (tx, rx) = async_channel::unbounded();
    let edit = Arc::new(AtomicBool::new(false));
    let _accept = server::spawn_accept(std::os::unix::net::UnixListener::bind(&sock).unwrap(), Shared::new(edit, tx)).unwrap();
    host::spawn_request_loop(&host, rx);
    host.load_enabled();

    let ticks = |host: &Host| -> u32 {
        let root = host.content_root("live").unwrap();
        let mut labels = Vec::new();
        collect_labels(&root, &mut labels);
        labels.iter().find_map(|t| t.strip_prefix("ticks=").map(|n| n.parse().unwrap())).unwrap()
    };
    let ctx = glib::MainContext::default();
    let mut iterations = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    while (iterations < 500 || ticks(&host) < 20) && Instant::now() < deadline {
        ctx.iteration(true);
        iterations += 1;
    }
    assert!(ticks(&host) >= 20, "the interval keeps updating the window ({} ticks after {iterations} iterations)", ticks(&host));
    let root = host.content_root("live").unwrap();
    let button = find::<gtk::Button>(&root).unwrap();
    button.emit_clicked();
    assert!(pump_until(Duration::from_secs(3), || label_text(&host, "live").as_deref() == Some("n=1")), "the click handler still fires");
    button.emit_clicked();
    assert!(pump_until(Duration::from_secs(3), || label_text(&host, "live").as_deref() == Some("n=2")));

    // Through the socket: answered by the host on this main loop.
    let reply = socket_request(&sock, Request::List);
    let Reply::Applets(all) = reply else { panic!("List over the socket answered {reply:?}") };
    assert!(all.iter().any(|s| s.id == "live" && s.running), "{all:?}");
    assert_eq!(socket_request(&sock, Request::SetEdit(true)), Reply::Edit(true));
    assert!(pump_until(Duration::from_secs(3), || host.edit()), "the host records the edit mode");
    assert_eq!(socket_request(&sock, Request::SetEnabled { id: "live".into(), enabled: false }), Reply::Ok);
    assert!(!host.is_running("live"));
    pump();
    eprintln!("section the_host_still_answers_after_many_main_loop_iterations: ok ({iterations} iterations)");
}

/// The texts of every label under `root`.
fn collect_labels(root: &gtk::Widget, out: &mut Vec<String>) {
    if let Ok(label) = root.clone().downcast::<gtk::Label>() { out.push(label.text().to_string()); }
    let mut child = root.first_child();
    while let Some(c) = child {
        collect_labels(&c, out);
        child = c.next_sibling();
    }
}

/// Send `req` to `sock` from another thread while this thread runs the main loop.
fn socket_request(sock: &Path, req: Request) -> Reply {
    let (tx, rx) = std::sync::mpsc::channel();
    let path = sock.to_path_buf();
    std::thread::spawn(move || tx.send(request_at(&path, &req)).unwrap());
    let mut got = None;
    assert!(pump_until(Duration::from_secs(3), || {
        if got.is_none() { got = rx.try_recv().ok(); }
        got.is_some()
    }), "no reply over the socket");
    got.unwrap().expect("the request over the socket")
}

/// Fix round 1: a window-manager close of an applet window means "disable
/// this applet", as `SetEnabled { enabled: false }`.
fn a_window_close_disables_the_applet(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("shut", &manifest("Shut"), &label_script("bye"));
    dirs.enable("shut");
    let host = dirs.host(app);
    host.load_enabled();
    let window = host.window_of("shut").expect("the enabled applet has a window");
    // `close()` does nothing on an unrealized window; realize creates the
    // surface without mapping it, so nothing appears on the screen.
    WidgetExt::realize(&window);
    assert!(!window.is_visible() && !window.is_mapped(), "a test window must never be shown");
    window.close();
    pump();
    assert!(!state::load_in(dirs.states.path(), "shut").enabled, "the close saved enabled = false");
    assert!(!host.is_running("shut"), "the runtime is gone");
    assert!(host.window_of("shut").is_none(), "the host no longer has the window");
    assert!(!app.windows().contains(&window.upcast()), "the window is destroyed");
    let st = status(&host, "shut");
    assert!(!st.enabled && !st.running, "{st:?}");
    eprintln!("section a_window_close_disables_the_applet: ok");
}

/// Fix round 1 (3a): after `SetEnabled false` the applet's `setInterval`
/// no longer fires. Each tick makes an HTTP request to a local server, a
/// side effect outside the applet that a dropped sink cannot hide.
fn a_disabled_applet_stops_its_timers(app: &gtk::Application) {
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for conn in server.incoming() {
            let Ok(mut conn) = conn else { continue };
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut conn, &mut buf);
            let _ = std::io::Write::write_all(&mut conn, b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n");
        }
    });
    let dirs = Dirs::new();
    let code = format!(r#"setInterval(() => {{ opus.http.get("http://127.0.0.1:{port}/").catch(() => {{}}); }}, 20);"#);
    dirs.applet("tick", &manifest("Tick"), &code);
    dirs.enable("tick");
    let host = dirs.host(app);
    host.load_enabled();
    let count = || hits.load(std::sync::atomic::Ordering::SeqCst);
    assert!(pump_until(Duration::from_secs(5), || count() >= 5), "the interval runs while enabled ({} hits)", count());
    assert_eq!(host.handle(&Request::SetEnabled { id: "tick".into(), enabled: false }), Reply::Ok);
    // Let the requests already in flight land, then nothing may follow.
    let _ = pump_until(Duration::from_millis(300), || false);
    let after_disable = count();
    let _ = pump_until(Duration::from_millis(500), || false);
    assert_eq!(count(), after_disable, "the interval still fires after the applet was disabled");
    eprintln!("section a_disabled_applet_stops_its_timers: ok ({after_disable} hits, none after the disable)");
}

/// Fix round 1 (3b): a button whose `onClick` shows a new tree replaces the
/// tree from inside its own signal emission: no panic, the new tree is
/// shown, and the new button works.
fn a_click_that_shows_a_new_tree_replaces_it(app: &gtk::Application) {
    let dirs = Dirs::new();
    let code = r#"let n = 0;
function render() {
  opus.ui.show({ type: "box", children: [
    { type: "label", id: "l", text: "gen " + n },
    { type: "button", id: "b", label: "next", onClick: () => { n++; render(); } },
  ] });
}
render();"#;
    dirs.applet("swapper", &manifest("Swapper"), code);
    dirs.enable("swapper");
    let host = dirs.host(app);
    host.load_enabled();
    assert_eq!(label_text(&host, "swapper").as_deref(), Some("gen 0"));
    let first = find::<gtk::Button>(&host.content_root("swapper").unwrap()).unwrap();
    first.emit_clicked();
    assert!(pump_until(Duration::from_secs(3), || label_text(&host, "swapper").as_deref() == Some("gen 1")), "the new tree is shown");
    let second = find::<gtk::Button>(&host.content_root("swapper").unwrap()).unwrap();
    assert_ne!(first, second, "the button was replaced");
    second.emit_clicked();
    assert!(pump_until(Duration::from_secs(3), || label_text(&host, "swapper").as_deref() == Some("gen 2")), "the new button works");
    assert!(host.error_text("swapper").is_none(), "no error: {:?}", host.error_text("swapper"));
    eprintln!("section a_click_that_shows_a_new_tree_replaces_it: ok");
}

/// Task 10: `x11_window::prepare` on a window that has no X11 surface (not
/// realized here; Wayland behaves the same) logs and returns without a panic
/// and without realizing or showing the window. A realized window is never
/// prepared here: that would send hints to the window manager of the test
/// display (the spike checks it in a private Xephyr instead).
fn prepare_on_a_window_without_an_x11_surface_does_not_panic(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder().application(app).decorated(false).build();
    opusdm_applets::x11_window::prepare(&window, 10, 20, 200, 100);
    opusdm_applets::x11_window::prepare(&window, -5, -5, 0, 0);
    assert!(!window.is_realized(), "prepare must not realize the window");
    assert!(!window.is_visible(), "prepare must not show the window");
    window.destroy();
    eprintln!("section prepare_on_a_window_without_an_x11_surface_does_not_panic: ok");
}

/// Task 11: the entry point the window calls after a move or resize by the
/// window manager writes `x, y, width, height` into the state file (keeping
/// `enabled` and the settings), and an unchanged geometry writes nothing.
fn the_host_saves_a_changed_geometry_once(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("pos", &manifest("Pos"), &label_script("here"));
    let mut saved = AppletState { enabled: true, x: Some(40), y: Some(50), ..Default::default() };
    saved.settings.insert("kept".into(), serde_json::json!("yes"));
    state::save_in(dirs.states.path(), "pos", &saved).unwrap();
    let host = dirs.host(app);
    host.load_enabled();
    assert_eq!(host.save_geometry("pos", (300, 200, 250, 150)), Ok(true), "a changed geometry is written");
    let st = state::load_in(dirs.states.path(), "pos");
    assert_eq!((st.x, st.y, st.width, st.height), (Some(300), Some(200), Some(250), Some(150)));
    assert!(st.enabled, "enabled is kept");
    assert_eq!(st.settings.get("kept"), Some(&serde_json::json!("yes")), "the settings are kept");
    assert_eq!(host.save_geometry("pos", (300, 200, 250, 150)), Ok(false), "the same geometry is not written again");
    assert!(host.save_geometry("nothere", (1, 2, 3, 4)).is_err(), "an applet that is not open is an error");
    assert!(!dirs.states.path().join("nothere.json").exists(), "no state file for an applet that is not open");
    assert!(host.is_running("pos"), "saving the geometry does not touch the runtime");
    eprintln!("section the_host_saves_a_changed_geometry_once: ok");
}

/// Make the state file of applet `id` exist but not readable: a saved
/// state (enabled, with a position and a setting) with mode 000. Running as
/// root reads it anyway, so then a folder takes its place. Gives the path.
fn make_state_unreadable(dirs: &Dirs, id: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dirs.states.path().join(format!("{id}.json"));
    if path.is_dir() { std::fs::remove_dir(&path).unwrap(); }
    let mut saved = AppletState { enabled: true, x: Some(77), y: Some(88), ..Default::default() };
    saved.settings.insert("kept".into(), serde_json::json!("yes"));
    state::save_in(dirs.states.path(), id, &saved).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&path).is_ok() {
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
    }
    path
}

/// True when the state file `path` made by [`make_state_unreadable`] was not
/// replaced (still the folder, or still the saved state once readable again).
fn still_unreadable_original(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if path.is_dir() { return true; }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
    text.contains("\"kept\"") && text.contains("77")
}

/// Remove the state file `path` made by [`make_state_unreadable`].
fn remove_state(path: &Path) {
    if path.is_dir() { std::fs::remove_dir(path).unwrap() } else { std::fs::remove_file(path).unwrap() }
}

/// Final review (1): a state file that exists but cannot be read is never
/// replaced by a default state (which would drop the settings and the
/// position) by a geometry save, a `SetEnabled` or a window close; a missing
/// state file is still created, and a geometry save of an open applet writes
/// `enabled = true`.
fn an_unreadable_state_file_is_never_overwritten(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("geo", &manifest("Geo"), &label_script("g"));
    dirs.applet("sw", &manifest("Sw"), &label_script("s"));
    dirs.applet("win", &manifest("Win"), &label_script("w"));
    dirs.enable("geo");
    dirs.enable("win");
    let host = dirs.host(app);
    host.load_enabled();

    // save_geometry: an unreadable file is an error naming the applet.
    let path = make_state_unreadable(&dirs, "geo");
    let err = host.save_geometry("geo", (1, 2, 210, 120)).expect_err("an unreadable state is not overwritten");
    assert!(err.contains("geo"), "{err}");
    assert!(still_unreadable_original(&path), "the unreadable state was not replaced");
    // A missing file is created, with enabled = true (the applet is open).
    remove_state(&path);
    assert_eq!(host.save_geometry("geo", (1, 2, 210, 120)), Ok(true));
    let st = state::load_in(dirs.states.path(), "geo");
    assert_eq!((st.enabled, st.x, st.y), (true, Some(1), Some(2)), "{st:?}");
    // A malformed file (backed up, default state) still keeps enabled.
    std::fs::write(&path, "{ nope").unwrap();
    assert_eq!(host.save_geometry("geo", (3, 4, 210, 120)), Ok(true));
    let st = state::load_in(dirs.states.path(), "geo");
    assert_eq!((st.enabled, st.x, st.y), (true, Some(3), Some(4)), "{st:?}");

    // SetEnabled: an unreadable file is an error, nothing written or opened.
    let path = make_state_unreadable(&dirs, "sw");
    match host.handle(&Request::SetEnabled { id: "sw".into(), enabled: true }) {
        Reply::Error(e) => assert!(e.contains("sw"), "{e}"),
        other => panic!("SetEnabled over an unreadable state answered {other:?}"),
    }
    assert!(still_unreadable_original(&path), "the unreadable state was not replaced");
    assert!(!host.is_running("sw") && host.window_of("sw").is_none(), "not opened");
    // A missing file is created.
    remove_state(&path);
    assert_eq!(host.handle(&Request::SetEnabled { id: "sw".into(), enabled: true }), Reply::Ok);
    assert!(state::load_in(dirs.states.path(), "sw").enabled, "a missing state file is created");
    assert!(host.is_running("sw"));

    // Window close: an unreadable file is not replaced, the applet still closes.
    let path = make_state_unreadable(&dirs, "win");
    let window = host.window_of("win").expect("win is open");
    WidgetExt::realize(&window);
    assert!(!window.is_mapped(), "a test window must never be shown");
    window.close();
    pump();
    assert!(still_unreadable_original(&path), "the unreadable state was not replaced");
    assert!(!host.is_running("win") && host.window_of("win").is_none(), "the window close still closes the applet");
    eprintln!("section an_unreadable_state_file_is_never_overwritten: ok");
}

/// Task 11: a saved position off every monitor opens inside the first
/// monitor, and a monitor change clamps the target of every open window
/// without rewriting the saved position.
fn a_monitor_change_clamps_the_open_windows(app: &gtk::Application) {
    let monitors = opusdm_ui::list_monitors(&gtk::gdk::Display::default().expect("a display"));
    assert!(!monitors.is_empty(), "the test display lists a monitor");
    let dirs = Dirs::new();
    dirs.applet("far", &manifest("Far"), &label_script("far"));
    dirs.applet("near", &manifest("Near"), &label_script("near"));
    state::save_in(dirs.states.path(), "far", &AppletState { enabled: true, x: Some(5000), y: Some(40), ..Default::default() }).unwrap();
    state::save_in(dirs.states.path(), "near", &AppletState { enabled: true, x: Some(500), y: Some(400), width: Some(210), height: Some(120), ..Default::default() }).unwrap();
    let host = dirs.host(app);
    host.load_enabled();
    let (x, y, w, h) = host.target_of("far").expect("far is open");
    assert!(monitors.iter().any(|&(mx, my, mw, mh)| x >= mx && y >= my && x + w <= mx + mw && y + h <= my + mh), "x 5000 opens inside a monitor: {:?} on {monitors:?}", (x, y, w, h));
    assert_eq!((w, h), (210, 120), "the manifest size");
    assert_eq!(host.target_of("near"), Some((500, 400, 210, 120)), "a position inside the screen is kept at open");

    // The monitors change to one 640x480 screen.
    host.reclamp_windows(&[(0, 0, 640, 480)]);
    assert_eq!(host.target_of("near"), Some((430, 360, 210, 120)), "clamped into the new monitor");
    let (x, y, _, _) = host.target_of("far").unwrap();
    assert!(x + 210 <= 640 && y + 120 <= 480, "far clamped too: {x},{y}");
    assert_eq!(state::load_in(dirs.states.path(), "near").x, Some(500), "the saved position is not rewritten by a clamp");
    // No monitor listed (a hotplug in progress): the targets stay.
    host.reclamp_windows(&[]);
    assert_eq!(host.target_of("near"), Some((430, 360, 210, 120)));
    assert!(host.is_running("near") && host.is_running("far"));
    eprintln!("section a_monitor_change_clamps_the_open_windows: ok");
}

/// What the edit widgets of `window` show now: (icons visible, frame
/// visible, edit CSS class on the outer widget). `get_visible` is the
/// widget's own flag: `is_visible` would also need the window, never shown
/// in tests, to be visible.
fn edit_look(window: &AppletWindow) -> (bool, bool, bool) {
    (window.edit_icons().get_visible(), window.edit_frame().get_visible(), window.edit_outer().has_css_class(applet_window::EDIT_CLASS))
}

/// The edit look of the window of applet `id` opened by `host`.
fn host_edit_look(host: &Host, id: &str) -> (bool, bool, bool) {
    host.with_window(id, edit_look).unwrap_or_else(|| panic!("{id} has no window"))
}

/// A bare window, its settings request count and the geometries it saved.
type BareWindow = (AppletWindow, Rc<Cell<usize>>, Rc<RefCell<Vec<Rect>>>);

/// An applet window built directly (no host, never presented) at `(40,
/// 50)`, 210 x 120, over `edit`; it counts the settings requests and
/// records the geometries handed to `on_moved`.
fn bare_window(app: &gtk::Application, edit: &EditMode) -> BareWindow {
    let manifest = opusdm_applets::manifest::parse(&manifest("Bare")).unwrap();
    let st = AppletState { enabled: true, x: Some(40), y: Some(50), ..Default::default() };
    let settings = Rc::new(Cell::new(0));
    let moved = Rc::new(RefCell::new(Vec::new()));
    let (s, m) = (settings.clone(), moved.clone());
    let hooks = WindowHooks {
        on_close: Box::new(|| {}),
        on_moved: Rc::new(move |rect| m.borrow_mut().push(rect)),
        on_settings: Rc::new(move || s.set(s.get() + 1)),
    };
    (applet_window::open(app, "bare", &manifest, &st, false, edit, hooks), settings, moved)
}

/// Task 12: outside edit mode the handle and the gear are hidden (not just
/// transparent) and the border is not drawn; activating the hidden gear
/// does not ask for the settings.
fn the_icons_are_hidden_outside_edit_mode(app: &gtk::Application) {
    let edit = EditMode::new();
    let (window, settings, _) = bare_window(app, &edit);
    assert_eq!(edit_look(&window), (false, false, false));
    assert!(!window.edit_icons().get_visible(), "the box holding the handle and the gear is not drawn");
    assert!(!window.edit_frame().can_target(), "the border never takes clicks");
    window.gear().emit_clicked();
    assert_eq!(settings.get(), 0, "a hidden gear does nothing");
    let dirs = Dirs::new();
    dirs.applet("calm", &manifest("Calm"), &label_script("calm"));
    dirs.enable("calm");
    let host = dirs.host(app);
    host.load_enabled();
    assert!(!host.edit());
    assert_eq!(host_edit_look(&host, "calm"), (false, false, false));
    eprintln!("section the_icons_are_hidden_outside_edit_mode: ok");
}

/// Task 12: turning edit mode on shows the dashed border and the two icons
/// on every open window; the gear then asks for the settings once per
/// click; `SetEdit` from the socket side reaches the windows through the
/// request loop.
fn edit_mode_shows_the_border_and_the_icons(app: &gtk::Application) {
    let edit = EditMode::new();
    let (window, settings, _) = bare_window(app, &edit);
    assert!(edit.set(true));
    assert_eq!(edit_look(&window), (true, true, true));
    assert!(window.handle().get_visible() && window.gear().get_visible());
    assert!(window.handle().tooltip_text().is_some_and(|t| t == "Move"), "{:?}", window.handle().tooltip_text());
    assert!(window.gear().tooltip_text().is_some_and(|t| t == "Settings"), "{:?}", window.gear().tooltip_text());
    window.gear().emit_clicked();
    assert_eq!(settings.get(), 1, "the gear asks for the settings");

    let dirs = Dirs::new();
    dirs.applet("one", &manifest("One"), &label_script("1"));
    dirs.applet("two", &manifest("Two"), &label_script("2"));
    dirs.enable("one");
    dirs.enable("two");
    let host = dirs.host(app);
    let (tx, rx) = async_channel::unbounded();
    host::spawn_request_loop(&host, rx);
    host.load_enabled();
    assert_eq!(host.edit_mode().listener_count(), 2, "one listener per open window");
    tx.send_blocking(server::HostMessage::Edit(true)).unwrap();
    assert!(pump_until(Duration::from_secs(3), || host.edit()), "the socket's SetEdit reaches the host");
    assert_eq!(host_edit_look(&host, "one"), (true, true, true));
    assert_eq!(host_edit_look(&host, "two"), (true, true, true));
    // The gear of a host window goes to `Host::open_settings`: it opens the
    // settings window and does not close anything.
    host.with_window("one", |w| w.gear().emit_clicked()).unwrap();
    assert!(host.is_running("one"));
    assert!(host.settings_window("one").is_some(), "the gear opened the settings window");
    eprintln!("section edit_mode_shows_the_border_and_the_icons: ok");
}

/// Task 12: turning edit mode off hides the icons and the border again; a
/// closed window no longer listens.
fn setting_edit_off_hides_them_again(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("flip", &manifest("Flip"), &label_script("f"));
    dirs.enable("flip");
    let host = dirs.host(app);
    host.load_enabled();
    host.set_edit(true);
    assert_eq!(host_edit_look(&host, "flip"), (true, true, true));
    host.set_edit(false);
    assert!(!host.edit());
    assert_eq!(host_edit_look(&host, "flip"), (false, false, false));
    host.set_edit(true);
    assert_eq!(host_edit_look(&host, "flip"), (true, true, true), "on again after off");
    assert_eq!(host.handle(&Request::SetEnabled { id: "flip".into(), enabled: false }), Reply::Ok);
    assert_eq!(host.edit_mode().listener_count(), 0, "a closed window unsubscribed its listener");
    host.set_edit(false);
    eprintln!("section setting_edit_off_hides_them_again: ok");
}

/// Task 12: the handle drag moves the window by the pointer delta (root
/// coordinates), clamped to the monitors, updates the target on each step
/// and saves once, at the end. A real `GestureDrag` needs a real pointer, so
/// the steps the gesture calls are driven here with pointer values.
fn the_handle_drag_moves_the_window_like_alt_drag(app: &gtk::Application) {
    let edit = EditMode::new();
    edit.set(true);
    let (window, _, moved) = bare_window(app, &edit);
    let screen = [(0, 0, 640, 480)];
    assert_eq!(window.target(), (40, 50, 210, 120));
    window.begin_drag((100, 100), &screen);
    assert_eq!(window.drag_to((150, 130)), Some((90, 80, 210, 120)));
    assert_eq!(window.target(), (90, 80, 210, 120), "each step updates the target");
    assert_eq!(window.drag_to((2000, 2000)), Some((430, 360, 210, 120)), "clamped to the monitors");
    assert_eq!(window.drag_to((160, 140)), Some((100, 90, 210, 120)));
    assert!(moved.borrow().is_empty(), "nothing is saved during the drag");
    assert_eq!(window.end_drag(), Some((100, 90, 210, 120)));
    assert_eq!(*moved.borrow(), [(100, 90, 210, 120)], "saved once, at the end");
    assert_eq!(window.end_drag(), None, "a second end does nothing");
    assert_eq!(window.drag_to((300, 300)), None, "no step without a begin");
    assert_eq!(moved.borrow().len(), 1);
    // A press without a move saves nothing.
    window.begin_drag((10, 10), &screen);
    assert_eq!(window.end_drag(), None);
    assert_eq!(moved.borrow().len(), 1);

    // Through the host: the end of the drag writes the state file.
    let dirs = Dirs::new();
    dirs.applet("drag", &manifest("Drag"), &label_script("d"));
    state::save_in(dirs.states.path(), "drag", &AppletState { enabled: true, x: Some(40), y: Some(50), ..Default::default() }).unwrap();
    let host = dirs.host(app);
    host.load_enabled();
    host.set_edit(true);
    host.with_window("drag", |w| {
        w.begin_drag((100, 100), &screen);
        w.drag_to((130, 160));
        w.end_drag();
    }).unwrap();
    let st = state::load_in(dirs.states.path(), "drag");
    assert_eq!((st.x, st.y, st.width, st.height), (Some(70), Some(110), Some(210), Some(120)));
    assert_eq!(host.target_of("drag"), Some((70, 110, 210, 120)));
    eprintln!("section the_handle_drag_moves_the_window_like_alt_drag: ok");
}

/// Task 12: a window opened (or reloaded) while edit mode is on starts in
/// edit mode.
fn a_window_opened_while_edit_mode_is_on_starts_in_edit_mode(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("late", &manifest("Late"), &label_script("l"));
    let host = dirs.host(app);
    host.load_enabled();
    host.set_edit(true);
    assert_eq!(host.handle(&Request::SetEnabled { id: "late".into(), enabled: true }), Reply::Ok);
    assert_eq!(host_edit_look(&host, "late"), (true, true, true));
    assert_eq!(host.handle(&Request::Reload { id: "late".into() }), Reply::Ok);
    assert_eq!(host_edit_look(&host, "late"), (true, true, true), "a reload keeps edit mode");
    assert_eq!(host.edit_mode().listener_count(), 1, "the reloaded window replaced the old listener");
    eprintln!("section a_window_opened_while_edit_mode_is_on_starts_in_edit_mode: ok");
}

/// The manifest of a weather-like applet with one setting of each kind.
fn settings_manifest(name: &str) -> String {
    format!(r#"{{ "name": "{name}", "window": {{ "width": 210, "height": 120 }}, "settings": [
        {{ "key": "city", "type": "string", "label": "City", "default": "Rome" }},
        {{ "key": "units", "type": "choice", "label": "Units", "default": "C", "choices": ["C", "F"] }},
        {{ "key": "refreshMinutes", "type": "number", "label": "Refresh every (minutes)", "default": 30 }},
        {{ "key": "seconds", "type": "bool", "label": "Show seconds", "default": true }}
    ] }}"#)
}

/// A script that shows the city in label `l` and, on new settings, shows
/// `city/units/refreshMinutes/seconds` there and logs it.
const SETTINGS_SCRIPT: &str = r#"
opus.ui.show({ type: "label", id: "l", text: String(opus.applet.settings.city) });
opus.applet.onSettings((s) => {
    const text = s.city + "/" + s.units + "/" + s.refreshMinutes + "/" + s.seconds;
    console.log("new settings " + text);
    opus.ui.update("l", { text });
});
"#;

/// The first widget under `root` named `name`, depth first.
fn find_named(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name { return Some(root.clone()); }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_named(&c, name) { return Some(found); }
        child = c.next_sibling();
    }
    None
}

/// The field of setting `key` in settings window `window`, as a `T`.
fn field<T: IsA<gtk::Widget>>(window: &gtk::Window, key: &str) -> T {
    let root = window.child().expect("the settings window has a child");
    let widget = find_named(&root, &settings_form::field_name(key)).unwrap_or_else(|| panic!("no field for {key}"));
    widget.downcast::<T>().unwrap_or_else(|w| panic!("{key}: unexpected widget {}", w.type_()))
}

/// The button named `name` in `window`.
fn button(window: &gtk::Window, name: &str) -> gtk::Button {
    let root = window.child().expect("the settings window has a child");
    find_named(&root, name).and_then(|w| w.downcast::<gtk::Button>().ok()).unwrap_or_else(|| panic!("no button {name}"))
}

/// True when `window` was destroyed: GTK dropped it from its toplevels
/// (it is disposed when the last reference, this one, goes).
fn destroyed(window: &gtk::Window) -> bool {
    !gtk::Window::list_toplevels().contains(window.upcast_ref::<gtk::Widget>())
}

/// Every label text of `window`.
fn window_labels(window: &gtk::Window) -> Vec<String> {
    let mut out = Vec::new();
    collect_labels(&window.child().expect("the settings window has a child"), &mut out);
    out
}

/// A host with applet `id` (the settings manifest and script) enabled and
/// its settings window open; the state file has `saved`.
fn settings_host(app: &gtk::Application, dirs: &Dirs, id: &str, saved: AppletState) -> (Rc<Host>, gtk::Window) {
    dirs.applet(id, &settings_manifest("Weather"), SETTINGS_SCRIPT);
    state::save_in(dirs.states.path(), id, &saved).unwrap();
    let host = dirs.host(app);
    host.load_enabled();
    assert!(host.is_running(id));
    host.set_edit(true);
    host.with_window(id, |w| w.gear().emit_clicked()).unwrap();
    let window = host.settings_window(id).expect("the gear opens the settings window");
    (host, window)
}

/// Task 13: the settings window is a normal decorated window titled
/// "<name> settings", never presented in tests, and its fields show the
/// saved values (the defaults where nothing valid is saved).
fn the_form_shows_the_current_values(app: &gtk::Application) {
    let dirs = Dirs::new();
    let mut saved = AppletState { enabled: true, ..Default::default() };
    saved.settings.insert("city".into(), "Paris".into());
    saved.settings.insert("units".into(), "F".into());
    saved.settings.insert("refreshMinutes".into(), "bad".into());
    let (_host, window) = settings_host(app, &dirs, "show", saved);
    assert_eq!(window.title().as_deref(), Some("Weather settings"));
    assert!(window.is_decorated(), "the settings window is a normal decorated window");
    assert!(!window.is_visible(), "a test window must never be presented");
    assert!(window.transient_for().is_none());
    assert_eq!(field::<gtk::Entry>(&window, "city").text(), "Paris");
    let units = field::<gtk::DropDown>(&window, "units");
    assert_eq!(units.selected(), 1, "F is the second choice");
    let spin = field::<gtk::SpinButton>(&window, "refreshMinutes");
    assert_eq!(spin.value(), 30.0, "a saved value of the wrong type shows the default");
    assert_eq!(spin.digits(), 0);
    assert!(spin.is_numeric(), "letters cannot be typed in a number field");
    assert!(field::<gtk::Switch>(&window, "seconds").is_active());
    let labels = window_labels(&window);
    for label in ["City", "Units", "Refresh every (minutes)", "Show seconds"] {
        assert!(labels.iter().any(|l| l == label), "{label} in {labels:?}");
    }
    assert!(button(&window, settings_form::APPLY_NAME).get_visible());
    assert!(button(&window, settings_form::CLOSE_NAME).get_visible());
    eprintln!("section the_form_shows_the_current_values: ok");
}

/// Task 13: Apply writes the new settings to the state file keeping
/// `enabled` and the geometry, and the running applet's `onSettings` sees
/// them; the window stays open. Close then closes it and the host forgets it.
fn apply_saves_the_state_and_calls_on_settings(app: &gtk::Application) {
    let dirs = Dirs::new();
    let mut saved = AppletState { enabled: true, x: Some(11), y: Some(22), width: Some(210), height: Some(120), ..Default::default() };
    saved.settings.insert("stale".into(), 7.into());
    let (host, window) = settings_host(app, &dirs, "apply", saved);
    assert_eq!(label_text(&host, "apply").as_deref(), Some("Rome"));
    field::<gtk::Entry>(&window, "city").set_text("Milan");
    field::<gtk::DropDown>(&window, "units").set_selected(1);
    field::<gtk::SpinButton>(&window, "refreshMinutes").set_value(15.0);
    field::<gtk::Switch>(&window, "seconds").set_active(false);
    button(&window, settings_form::APPLY_NAME).emit_clicked();
    assert!(pump_until(Duration::from_secs(3), || label_text(&host, "apply").as_deref() == Some("Milan/F/15/false")), "onSettings saw the new values: {:?}", label_text(&host, "apply"));
    let st = state::load_in(dirs.states.path(), "apply");
    assert!(st.enabled, "Apply keeps enabled");
    assert_eq!((st.x, st.y, st.width, st.height), (Some(11), Some(22), Some(210), Some(120)), "Apply keeps the geometry");
    assert_eq!(st.settings["city"], "Milan");
    assert_eq!(st.settings["units"], "F");
    assert_eq!(st.settings["refreshMinutes"], 15);
    assert_eq!(st.settings["seconds"], false);
    assert_eq!(st.settings["stale"], 7, "a saved key that is not declared stays in the file");
    assert_eq!(settings_form::status_text(&window), None, "no error after a good Apply");
    assert_eq!(host.settings_window("apply").as_ref(), Some(&window), "Apply keeps the window open");
    button(&window, settings_form::CLOSE_NAME).emit_clicked();
    assert!(host.settings_window("apply").is_none(), "Close closes it and the host forgets it");
    assert!(destroyed(&window), "the closed window was destroyed");
    // The gear opens a new one with the saved values.
    host.with_window("apply", |w| w.gear().emit_clicked()).unwrap();
    let again = host.settings_window("apply").expect("a new settings window");
    assert_ne!(again, window);
    assert_eq!(field::<gtk::Entry>(&again, "city").text(), "Milan");
    eprintln!("section apply_saves_the_state_and_calls_on_settings: ok");
}

/// Task 13: a number that does not parse is shown in the settings window,
/// nothing is saved and `onSettings` is not called.
fn an_invalid_value_is_shown_in_the_window_and_not_saved(app: &gtk::Application) {
    let dirs = Dirs::new();
    let (host, window) = settings_host(app, &dirs, "invalid", AppletState { enabled: true, ..Default::default() });
    let before = std::fs::read_to_string(dirs.states.path().join("invalid.json")).unwrap();
    let spin = field::<gtk::SpinButton>(&window, "refreshMinutes");
    spin.set_text("soon");
    button(&window, settings_form::APPLY_NAME).emit_clicked();
    pump();
    let status = settings_form::status_text(&window).expect("the error is shown in the window");
    assert!(status.contains("refreshMinutes"), "{status}");
    assert_eq!(std::fs::read_to_string(dirs.states.path().join("invalid.json")).unwrap(), before, "nothing saved");
    assert_eq!(label_text(&host, "invalid").as_deref(), Some("Rome"), "onSettings not called");
    assert!(host.settings_window("invalid").is_some(), "the window stays open");
    eprintln!("section an_invalid_value_is_shown_in_the_window_and_not_saved: ok");
}

/// Task 13: when the state file exists but cannot be read, Apply writes
/// nothing over it (no defaults), shows the error in the window and keeps
/// it open; the applet is not told.
fn apply_with_an_unreadable_state_file_writes_nothing(app: &gtk::Application) {
    let dirs = Dirs::new();
    let (host, window) = settings_host(app, &dirs, "unread", AppletState { enabled: true, ..Default::default() });
    let path = dirs.states.path().join("unread.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    field::<gtk::Entry>(&window, "city").set_text("Milan");
    button(&window, settings_form::APPLY_NAME).emit_clicked();
    pump();
    let status = settings_form::status_text(&window).expect("the read error is shown in the window");
    assert!(status.contains("unread"), "{status}");
    assert!(path.is_dir(), "the unreadable state was not replaced");
    assert_eq!(label_text(&host, "unread").as_deref(), Some("Rome"), "onSettings not called");
    assert!(host.settings_window("unread").is_some(), "the window stays open");
    eprintln!("section apply_with_an_unreadable_state_file_writes_nothing: ok");
}

/// Task 13: an applet without settings gets a window saying so, with only
/// a Close button.
fn an_applet_with_no_settings_says_so_in_the_window(app: &gtk::Application) {
    let dirs = Dirs::new();
    dirs.applet("plain", &manifest("Plain"), &label_script("p"));
    dirs.enable("plain");
    let host = dirs.host(app);
    host.load_enabled();
    host.open_settings("plain");
    let window = host.settings_window("plain").expect("a settings window");
    assert_eq!(window.title().as_deref(), Some("Plain settings"));
    let labels = window_labels(&window);
    assert!(labels.iter().any(|l| l == "This applet has no settings"), "{labels:?}");
    let root = window.child().unwrap();
    assert!(find_named(&root, settings_form::APPLY_NAME).is_none(), "no Apply button");
    assert!(find::<gtk::Entry>(&root).is_none());
    button(&window, settings_form::CLOSE_NAME).emit_clicked();
    assert!(host.settings_window("plain").is_none());
    assert!(destroyed(&window));
    eprintln!("section an_applet_with_no_settings_says_so_in_the_window: ok");
}

/// Task 13: the gear of an applet whose settings window is open gives
/// that window again, never a second one; a closed or unknown applet gets
/// none.
fn the_gear_twice_opens_one_window(app: &gtk::Application) {
    let dirs = Dirs::new();
    let (host, window) = settings_host(app, &dirs, "twice", AppletState { enabled: true, ..Default::default() });
    let toplevels = gtk::Window::list_toplevels().len();
    host.with_window("twice", |w| w.gear().emit_clicked()).unwrap();
    host.open_settings("twice");
    assert_eq!(host.settings_window("twice").as_ref(), Some(&window), "the same window");
    assert_eq!(gtk::Window::list_toplevels().len(), toplevels, "no second window was built");
    host.open_settings("nobody");
    assert!(host.settings_window("nobody").is_none(), "no window for an applet that is not open");
    // A window-manager close (title bar) of the settings window: the host
    // forgets it, the applet keeps running and the gear opens a new one.
    WidgetExt::realize(&window);
    assert!(!window.is_mapped(), "a test window must never be shown");
    window.close();
    pump();
    assert!(host.settings_window("twice").is_none(), "the host forgot the closed window");
    assert!(destroyed(&window), "the title bar close destroyed it");
    assert!(host.is_running("twice"), "closing the settings does not touch the applet");
    host.open_settings("twice");
    assert!(host.settings_window("twice").is_some_and(|w| w != window), "a new window after the close");
    eprintln!("section the_gear_twice_opens_one_window: ok");
}

/// Task 13: disabling, reloading or closing the window of an applet closes
/// its settings window too, and the host forgets it.
fn disabling_the_applet_closes_its_settings_window(app: &gtk::Application) {
    let dirs = Dirs::new();
    let (host, window) = settings_host(app, &dirs, "gone", AppletState { enabled: true, ..Default::default() });
    assert_eq!(host.handle(&Request::SetEnabled { id: "gone".into(), enabled: false }), Reply::Ok);
    assert!(host.settings_window("gone").is_none(), "disable closes the settings window");
    assert!(destroyed(&window), "the settings window was destroyed");

    assert_eq!(host.handle(&Request::SetEnabled { id: "gone".into(), enabled: true }), Reply::Ok);
    host.open_settings("gone");
    let window = host.settings_window("gone").expect("open again");
    assert_eq!(host.handle(&Request::Reload { id: "gone".into() }), Reply::Ok);
    assert!(host.settings_window("gone").is_none(), "reload closes the settings window");
    assert!(destroyed(&window));

    host.open_settings("gone");
    let window = host.settings_window("gone").expect("open after the reload");
    let applet_window = host.window_of("gone").unwrap();
    WidgetExt::realize(&applet_window);
    applet_window.close();
    pump();
    assert!(!host.is_running("gone"), "the window close disabled the applet");
    assert!(host.settings_window("gone").is_none(), "the window close closes the settings window");
    assert!(destroyed(&window));
    eprintln!("section disabling_the_applet_closes_its_settings_window: ok");
}
