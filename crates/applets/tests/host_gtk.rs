//! GTK checks of the applets host. GTK may be used from one thread only, so
//! every check is a section of the single `#[test]` below (this file is its
//! own test process). Windows are built but never presented
//! (`present_windows: false`): nothing appears on the screen.

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gio, glib};
use opusdm_applets::host::{self, Host, HostConfig};
use opusdm_applets::protocol::{request_at, AppletStatus, Reply, Request};
use opusdm_applets::server::{self, Shared};
use opusdm_applets::state::{self, AppletState};
use std::path::Path;
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
    a_monitor_change_clamps_the_open_windows(&app);
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
