//! GTK checks of the Applet Browser window. GTK may be used from one thread
//! only, so every check is a section of the single `#[test]` below (this
//! file is its own test process). Windows are built but never presented, and
//! each check talks to a fake applets server on its own temp socket.

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gio, glib};
use opusdm_applet_browser::{model, window};
use opusdm_applets::protocol::{AppletStatus, Reply, Request};
use opusdm_ipc::frame::{recv, send};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a check waits for the window to react.
const WAIT: Duration = Duration::from_secs(5);

/// A fake applets server: answers every request and records it.
struct FakeServer {
    _dir: tempfile::TempDir,
    path: PathBuf,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeServer {
    /// A server that accepts every change.
    fn start(applets: Vec<AppletStatus>) -> FakeServer {
        FakeServer::start_with(applets, None)
    }

    /// Bind a socket under /tmp (short path) and answer on a thread: `List`
    /// gives `applets`, `GetEdit` false, `SetEdit(b)` `Edit(b)`, the rest `Ok`;
    /// with `refuse`, `SetEnabled`, `SetEdit` and `Reload` get `Error(refuse)`.
    fn start_with(applets: Vec<AppletStatus>, refuse: Option<&str>) -> FakeServer {
        FakeServer::spawn(applets, refuse, false)
    }

    /// Like a refusing [`FakeServer::start_with`], but a refused `SetEnabled`
    /// is applied anyway (a busy process that did the change after the
    /// client gave up): the next `List` shows the new `enabled`.
    fn start_applying_refused(applets: Vec<AppletStatus>, refuse: &str) -> FakeServer {
        FakeServer::spawn(applets, Some(refuse), true)
    }

    /// The server thread of the constructors above.
    fn spawn(applets: Vec<AppletStatus>, refuse: Option<&str>, apply_refused: bool) -> FakeServer {
        let refuse = refuse.map(str::to_string);
        let mut applets = applets;
        let dir = tempfile::Builder::new().prefix("oab15").tempdir_in("/tmp").unwrap();
        let path = dir.path().join("a.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        // Detached: it blocks in accept until the test process exits.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let Ok(Some(req)) = recv::<_, Request>(&mut s) else { continue };
                if let (true, Request::SetEnabled { id, enabled }) = (apply_refused, &req) {
                    for a in applets.iter_mut().filter(|a| &a.id == id) {
                        (a.enabled, a.running) = (*enabled, *enabled);
                    }
                }
                let reply = match (&req, &refuse) {
                    (Request::SetEnabled { .. } | Request::SetEdit(_) | Request::Reload { .. }, Some(msg)) => Reply::Error(msg.clone()),
                    _ => match &req {
                    Request::List => Reply::Applets(applets.clone()),
                    Request::GetEdit => Reply::Edit(false),
                    Request::SetEdit(b) => Reply::Edit(*b),
                    _ => Reply::Ok,
                    },
                };
                seen.lock().unwrap().push(req);
                let _ = send(&mut s, &reply);
            }
        });
        FakeServer { _dir: dir, path, requests }
    }

    /// A copy of the requests received so far.
    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

/// A status for applet `id`.
fn applet(id: &str, name: Option<&str>, enabled: bool, error: Option<&str>) -> AppletStatus {
    AppletStatus {
        id: id.into(),
        name: name.map(str::to_string),
        description: format!("about {id}"),
        version: "1.0".into(),
        enabled,
        running: enabled && error.is_none(),
        error: error.map(str::to_string),
    }
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

/// The first widget under `root` with widget name `name`, depth first.
fn named(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name { return Some(root.clone()); }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = named(&c, name) { return Some(found); }
        child = c.next_sibling();
    }
    None
}

/// Wait for the widget `name` of type `T` under `window`.
fn wait_named<T: IsA<gtk::Widget>>(window: &gtk::ApplicationWindow, name: &str) -> T {
    let root = window.clone().upcast::<gtk::Widget>();
    assert!(pump_until(WAIT, || named(&root, name).is_some()), "no widget named {name:?}");
    named(&root, name).unwrap().downcast::<T>().unwrap_or_else(|_| panic!("{name:?} has another type"))
}

/// Every label text under `root`.
fn label_texts(root: &gtk::Widget) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(l) = root.clone().downcast::<gtk::Label>() { out.push(l.text().to_string()); }
    let mut child = root.first_child();
    while let Some(c) = child {
        out.extend(label_texts(&c));
        child = c.next_sibling();
    }
    out
}

/// Wait until `server` received a request equal to `want`.
fn wait_request(server: &FakeServer, want: &Request) -> bool {
    pump_until(WAIT, || server.requests().contains(want))
}

/// Destroy every window of `app`, so the next check starts clean.
fn close_all(app: &gtk::Application) {
    for w in app.windows() { w.destroy(); }
    assert!(pump_until(WAIT, || app.windows().is_empty()), "windows left open");
}

/// One row per applet: the name or folder id, the description, the enabled
/// switch and the error in red.
fn the_list_shows_each_applet(app: &gtk::Application) {
    let server = FakeServer::start(vec![
        applet("clock", Some("Clock"), true, None),
        applet("no-name", None, false, None),
        applet("broken", Some("Broken"), true, Some("main.js: boom")),
    ]);
    let window = window::open_with(app, server.path.clone(), false);
    assert!(!window.is_visible(), "a test window must never be presented");
    let clock: gtk::Label = wait_named(&window, "title:clock");
    assert!(clock.text().contains("Clock"), "clock title: {}", clock.text());
    let no_name: gtk::Label = wait_named(&window, "title:no-name");
    assert!(no_name.text().contains("no-name"), "no-name title: {}", no_name.text());
    let on: gtk::Switch = wait_named(&window, "enabled:clock");
    assert!(on.is_active());
    let off: gtk::Switch = wait_named(&window, "enabled:no-name");
    assert!(!off.is_active());
    let _reload: gtk::Button = wait_named(&window, "reload:clock");
    let err: gtk::Label = wait_named(&window, "error:broken");
    assert!(err.text().contains("main.js: boom"));
    assert!(err.has_css_class("error"), "the error is red");
    assert!(named(&window.clone().upcast(), "error:clock").is_none(), "no error label for a working applet");
    let texts = label_texts(&window.clone().upcast());
    assert!(texts.iter().any(|t| t.contains("about clock")), "description shown: {texts:?}");
    close_all(app);
}

/// Flipping an enabled switch sends `SetEnabled`, and the window still
/// reacts after the first listing (it was not dropped).
fn toggling_enabled_sends_set_enabled(app: &gtk::Application) {
    let server = FakeServer::start(vec![applet("clock", Some("Clock"), false, None)]);
    let window = window::open_with(app, server.path.clone(), false);
    let sw: gtk::Switch = wait_named(&window, "enabled:clock");
    // Many idle iterations after the first listing: the state must survive.
    let ctx = glib::MainContext::default();
    for _ in 0..200 { ctx.iteration(false); }
    sw.set_active(true);
    assert!(wait_request(&server, &Request::SetEnabled { id: "clock".into(), enabled: true }), "got {:?}", server.requests());
    let reload: gtk::Button = wait_named(&window, "reload:clock");
    reload.emit_clicked();
    assert!(wait_request(&server, &Request::Reload { id: "clock".into() }), "got {:?}", server.requests());
    close_all(app);
}

/// The edit switch shows `GetEdit` and sends `SetEdit`.
fn the_edit_switch_sends_set_edit(app: &gtk::Application) {
    let server = FakeServer::start(vec![]);
    let window = window::open_with(app, server.path.clone(), false);
    let edit: gtk::Switch = wait_named(&window, "edit-switch");
    assert!(wait_request(&server, &Request::GetEdit));
    assert!(pump_until(WAIT, || edit.is_sensitive()), "the edit switch stays insensitive");
    edit.set_active(true);
    assert!(wait_request(&server, &Request::SetEdit(true)), "got {:?}", server.requests());
    assert!(pump_until(WAIT, || edit.is_sensitive() && edit.is_active()));
    edit.set_active(false);
    assert!(wait_request(&server, &Request::SetEdit(false)), "got {:?}", server.requests());
    // An empty folder shows a hint, not an empty list.
    let root = window.clone().upcast::<gtk::Widget>();
    assert!(pump_until(WAIT, || label_texts(&root).iter().any(|t| t.starts_with("No applets in "))), "no empty hint: {:?}", label_texts(&root));
    close_all(app);
}

/// With no server the window says so instead of showing an empty list, and
/// a failed toggle of the edit switch flips it back.
fn with_no_server_the_window_says_the_process_is_not_running(app: &gtk::Application) {
    let dir = tempfile::Builder::new().prefix("oab15").tempdir_in("/tmp").unwrap();
    let window = window::open_with(app, dir.path().join("none.sock"), false);
    let root = window.clone().upcast::<gtk::Widget>();
    let text = model::host_unreachable_text();
    assert!(pump_until(WAIT, || label_texts(&root).iter().any(|t| t == text)), "labels: {:?}", label_texts(&root));
    let refresh: gtk::Button = wait_named(&window, "refresh-button");
    assert!(pump_until(WAIT, || refresh.is_sensitive()), "refresh stays insensitive");
    refresh.emit_clicked();
    assert!(pump_until(WAIT, || refresh.is_sensitive() && label_texts(&root).iter().any(|t| t == text)));
    close_all(app);
}

/// A refused change flips its switch back, re-enables it and shows the error.
fn a_refused_change_flips_the_switch_back_and_shows_the_error(app: &gtk::Application) {
    let server = FakeServer::start_with(vec![applet("clock", Some("Clock"), false, None)], Some("no way"));
    let window = window::open_with(app, server.path.clone(), false);
    let root = window.clone().upcast::<gtk::Widget>();
    let sw: gtk::Switch = wait_named(&window, "enabled:clock");
    sw.set_active(true);
    assert!(wait_request(&server, &Request::SetEnabled { id: "clock".into(), enabled: true }));
    assert!(pump_until(WAIT, || sw.is_sensitive() && !sw.is_active()), "the switch did not flip back");
    let status: gtk::Label = wait_named(&window, "status");
    assert!(status.get_visible() && status.text().contains("no way"), "status: {:?}", status.text());
    let edit: gtk::Switch = wait_named(&window, "edit-switch");
    assert!(pump_until(WAIT, || edit.is_sensitive()));
    edit.set_active(true);
    assert!(wait_request(&server, &Request::SetEdit(true)));
    assert!(pump_until(WAIT, || edit.is_sensitive() && !edit.is_active()), "the edit switch did not flip back");
    // Only one SetEnabled: flipping back sent nothing.
    let sets = server.requests().iter().filter(|r| matches!(r, Request::SetEnabled { .. } | Request::SetEdit(_))).count();
    assert_eq!(sets, 2, "got {:?}", server.requests());
    assert!(label_texts(&root).iter().any(|t| t.contains("Clock")));
    // A refused Reload shows the error and lists again (the row shows a
    // script error), and the error stays after that refresh.
    let lists = || server.requests().iter().filter(|r| **r == Request::List).count();
    let before = lists();
    let reload: gtk::Button = wait_named(&window, "reload:clock");
    reload.emit_clicked();
    assert!(wait_request(&server, &Request::Reload { id: "clock".into() }));
    assert!(pump_until(WAIT, || lists() > before), "no List after a failed Reload");
    let reload: gtk::Button = wait_named(&window, "reload:clock");
    assert!(pump_until(WAIT, || reload.is_sensitive()));
    let status: gtk::Label = wait_named(&window, "status");
    assert!(pump_until(WAIT, || status.get_visible() && status.text().contains("Cannot reload Clock: no way")), "status: {:?}", status.text());
    close_all(app);
}

/// Final review (4): a `SetEnabled` answered with an error (the client
/// timed out) that the busy process applied anyway: the switch flips back
/// and the error is shown at once, then the list is asked again and the
/// switch shows the real state (on).
fn a_failed_set_enabled_lists_again_and_shows_the_real_state(app: &gtk::Application) {
    let server = FakeServer::start_applying_refused(vec![applet("clock", Some("Clock"), false, None)], "the applets process is busy");
    let window = window::open_with(app, server.path.clone(), false);
    let sw: gtk::Switch = wait_named(&window, "enabled:clock");
    let lists = || server.requests().iter().filter(|r| **r == Request::List).count();
    let before = lists();
    sw.set_active(true);
    assert!(wait_request(&server, &Request::SetEnabled { id: "clock".into(), enabled: true }));
    assert!(pump_until(WAIT, || lists() > before), "no List after a failed SetEnabled: {:?}", server.requests());
    let root = window.clone().upcast::<gtk::Widget>();
    let on = || named(&root, "enabled:clock").and_then(|w| w.downcast::<gtk::Switch>().ok()).is_some_and(|s| s.is_active() && s.is_sensitive());
    assert!(pump_until(WAIT, on), "the switch does not show the real state (on)");
    let status: gtk::Label = wait_named(&window, "status");
    assert!(status.get_visible() && status.text().contains("busy"), "the error stays after the refresh: {:?}", status.text());
    let sets = server.requests().iter().filter(|r| matches!(r, Request::SetEnabled { .. })).count();
    assert_eq!(sets, 1, "the refresh sent no SetEnabled: {:?}", server.requests());
    close_all(app);
}

/// Activating twice keeps one window.
fn a_second_launch_raises_the_window_and_does_not_open_another(app: &gtk::Application) {
    let server = FakeServer::start(vec![]);
    window::activate_with(app, server.path.clone(), false);
    window::activate_with(app, server.path.clone(), false);
    assert_eq!(app.windows().len(), 1, "a second activation opened another window");
    assert!(!app.windows()[0].is_visible());
    close_all(app);
}

/// A window closed while its requests are in flight drops the replies
/// without a panic, and a new window still works.
fn closing_the_window_with_requests_in_flight_is_safe(app: &gtk::Application) {
    let server = FakeServer::start(vec![applet("clock", Some("Clock"), true, None)]);
    let window = window::open_with(app, server.path.clone(), false);
    window.destroy();
    let ctx = glib::MainContext::default();
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until { if !ctx.iteration(false) { std::thread::sleep(Duration::from_millis(1)); } }
    let again = window::open_with(app, server.path.clone(), false);
    let _sw: gtk::Switch = wait_named(&again, "enabled:clock");
    close_all(app);
}

#[test]
fn browser_gtk_checks() {
    if let Err(e) = gtk::init() {
        let display = std::env::var_os("DISPLAY").filter(|d| !d.is_empty());
        let wayland = std::env::var_os("WAYLAND_DISPLAY").filter(|d| !d.is_empty());
        if display.is_some() || wayland.is_some() { panic!("gtk::init() failed although a display is set (DISPLAY={display:?}, WAYLAND_DISPLAY={wayland:?}): {e}"); }
        eprintln!("browser_gtk_checks: SKIPPED, no display (DISPLAY and WAYLAND_DISPLAY unset)");
        return;
    }
    let app = gtk::Application::builder().application_id("org.opusdm.AppletBrowserTest").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.register(None::<&gio::Cancellable>).expect("register the test application");
    a_second_launch_raises_the_window_and_does_not_open_another(&app);
    the_list_shows_each_applet(&app);
    toggling_enabled_sends_set_enabled(&app);
    the_edit_switch_sends_set_edit(&app);
    with_no_server_the_window_says_the_process_is_not_running(&app);
    a_refused_change_flips_the_switch_back_and_shows_the_error(&app);
    a_failed_set_enabled_lists_again_and_shows_the_real_state(&app);
    closing_the_window_with_requests_in_flight_is_safe(&app);
}
