//! The host of `opusdm-applets`: owns, for every enabled applet, its window
//! and its runtime, and answers the socket requests on the GTK main loop.
//!
//! Ownership for the whole run: `main` keeps the only strong `Rc<Host>` in a
//! local until `app.run()` returns. The host owns one `Rc<Applet>` per open
//! applet (in `applets`); the applet owns its [`Runtime`], its
//! [`AppletWindow`], its built widget tree and the runtime's dispatcher.
//! Everything that calls back holds only a `Weak`: the runtime's sink and the
//! canvas error listener hold a `Weak<Applet>`, the widgets hold the
//! runtime's dispatcher (a `Weak` of the runtime), the request loop holds a
//! `Weak<Host>`. A failed upgrade is logged with `err_log!` naming the
//! applet. So there is no `Rc` cycle: removing an applet from `applets`
//! drops it, which stops its runtime and destroys its window.
//!
//! Edit mode (Task 12): the host owns the [`EditMode`] for the whole run;
//! every window gets a clone and subscribes one listener (weak references to
//! its widgets only), removed when the window is dropped. The socket's
//! `SetEdit` reaches [`Host::set_edit`] through the request loop. The
//! window's gear calls [`Host::open_settings`] through a `Weak<Host>`.

use crate::applet_window::{self, AppletWindow, WindowHooks};
use crate::discovery::{self, Discovered};
use crate::edit_mode::EditMode;
use crate::placement::Rect;
use crate::protocol::{AppletStatus, Reply, Request};
use crate::runtime::{Event, Runtime, Sink};
use crate::server::HostMessage;
use crate::widgets::{self, BuiltTree, Dispatch, ErrorListener};
use crate::{canvas, state, ui_tree};
use gtk4 as gtk;
use gtk::glib;
use opusdm_ipc::{debug_log, err_log};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Where the host finds the applets and their state, and whether it shows
/// the windows it builds.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The folder with one subfolder per applet.
    pub applets_dir: PathBuf,
    /// The folder with the `<id>.json` state files.
    pub state_dir: PathBuf,
    /// Present the applet windows; tests build them without showing them.
    pub present_windows: bool,
}

impl HostConfig {
    /// The real folders of the session, windows presented.
    pub fn real() -> HostConfig {
        HostConfig { applets_dir: discovery::applets_dir(), state_dir: state::states_dir(), present_windows: true }
    }
}

/// Owns the open applets (see the module doc).
pub struct Host {
    app: gtk::Application,
    config: HostConfig,
    /// The open applets by id: enabled and with a valid manifest. The entry
    /// stays (window showing the error) when the runtime failed to start.
    applets: RefCell<BTreeMap<String, Rc<Applet>>>,
    /// The last runtime start error of each applet, for `List`.
    start_errors: RefCell<HashMap<String, String>>,
    /// The edit mode, as last told by the socket, shared with every window.
    edit: EditMode,
    /// This host, for the window close handlers (never upgraded to own it).
    me: Weak<Host>,
}

/// One open applet. Field order is drop order: the runtime stops before
/// the widgets and the window go.
struct Applet {
    id: String,
    /// `None` when the entry script failed.
    runtime: RefCell<Option<Runtime>>,
    /// The runtime's dispatcher for the widgets (holds a `Weak` of it).
    dispatcher: RefCell<Option<Rc<dyn Dispatch>>>,
    /// The tree shown in the window, if any.
    tree: RefCell<Option<BuiltTree>>,
    /// `Some` while the runtime starts: the `Ui` and `Redraw` events sent
    /// before the dispatcher exists, applied once it does.
    pending: RefCell<Option<Vec<Event>>>,
    window: AppletWindow,
    /// This applet, for the canvas error listeners.
    me: Weak<Applet>,
}

impl Host {
    /// A host over the session's applets folder and state folder. The
    /// caller keeps the returned `Rc` for the whole run (see the module doc).
    pub fn new(app: &gtk::Application) -> Rc<Host> {
        Host::with_config(app, HostConfig::real())
    }

    /// A host over the folders of `config`. Builds no widget: call
    /// [`Host::load_enabled`] once GTK is up (application `startup`).
    pub fn with_config(app: &gtk::Application, config: HostConfig) -> Rc<Host> {
        Rc::new_cyclic(|me| Host { app: app.clone(), config, applets: RefCell::default(), start_errors: RefCell::default(), edit: EditMode::new(), me: me.clone() })
    }

    /// Open every enabled applet. A broken one is logged (and listed with
    /// its error) and does not stop the others.
    pub fn load_enabled(&self) {
        for found in discovery::discover(&self.config.applets_dir) {
            if !state::load_in(&self.config.state_dir, &found.id).enabled { continue; }
            if let Err(e) = self.open_applet(&found) {
                err_log!(APP_TAG, "applet {}: not started: {e}", found.id);
            }
        }
    }

    /// Answer one socket request; every error is logged too.
    pub fn handle(&self, req: &Request) -> Reply {
        let result = match req {
            Request::List => return Reply::Applets(self.list()),
            Request::GetEdit => return Reply::Edit(self.edit()),
            Request::SetEdit(on) => {
                self.set_edit(*on);
                return Reply::Edit(*on);
            }
            Request::SetEnabled { id, enabled } => self.set_enabled(id, *enabled),
            Request::Reload { id } => self.reload(id),
        };
        match result {
            Ok(()) => Reply::Ok,
            Err(e) => {
                err_log!(APP_TAG, "request {req:?} failed: {e}");
                Reply::Error(e)
            }
        }
    }

    /// Turn the edit mode on or off: every open window shows or hides its
    /// border, handle and gear. Not saved (off after a restart).
    pub fn set_edit(&self, on: bool) {
        let changed = self.edit.set(on);
        debug_log!(APP_TAG, "edit mode {on}{}", if changed { "" } else { " (unchanged)" });
    }

    /// The edit mode last recorded.
    pub fn edit(&self) -> bool {
        self.edit.get()
    }

    /// The shared edit mode (one listener per open window).
    pub fn edit_mode(&self) -> &EditMode {
        &self.edit
    }

    /// Open the settings of applet `id` (the edit gear). KNOWN STUB until
    /// Task 13 adds the settings window: it only logs at debug level.
    pub fn open_settings(&self, id: &str) {
        debug_log!(APP_TAG, "applet {id}: settings requested; the settings window comes with Task 13");
    }

    /// Run `f` on the window of applet `id`, while it is open.
    pub fn with_window<R>(&self, id: &str, f: impl FnOnce(&AppletWindow) -> R) -> Option<R> {
        let applet = self.applets.borrow().get(id).cloned()?;
        Some(f(&applet.window))
    }

    /// True when applet `id` is open and its runtime started.
    pub fn is_running(&self, id: &str) -> bool {
        self.applets.borrow().get(id).is_some_and(|a| a.runtime.borrow().is_some())
    }

    /// The window of applet `id`, while it is open.
    pub fn window_of(&self, id: &str) -> Option<gtk::ApplicationWindow> {
        self.applets.borrow().get(id).map(|a| a.window.window().clone())
    }

    /// The error shown in the window of applet `id`, if any.
    pub fn error_text(&self, id: &str) -> Option<String> {
        self.applets.borrow().get(id).and_then(|a| a.window.error_text())
    }

    /// The root widget shown in the window of applet `id`, if any.
    pub fn content_root(&self, id: &str) -> Option<gtk::Widget> {
        self.applets.borrow().get(id).and_then(|a| a.window.content())
    }

    /// One status per applet folder.
    fn list(&self) -> Vec<AppletStatus> {
        let applets = discovery::discover(&self.config.applets_dir);
        let mut out = Vec::with_capacity(applets.len());
        for found in applets {
            let enabled = state::load_in(&self.config.state_dir, &found.id).enabled;
            let running = self.is_running(&found.id);
            let (name, description, version, error) = match &found.manifest {
                Ok(m) => (Some(m.name.clone()), m.description.clone(), m.version.clone(), self.start_errors.borrow().get(&found.id).cloned()),
                Err(e) => (None, String::new(), String::new(), Some(e.clone())),
            };
            out.push(AppletStatus { id: found.id, name, description, version, enabled, running, error });
        }
        out
    }

    /// The applet folder `id`, re-read from disk.
    fn find(&self, id: &str) -> Result<Discovered, String> {
        let dir = &self.config.applets_dir;
        let found = discovery::discover(dir).into_iter().find(|d| d.id == id);
        found.ok_or_else(|| format!("no applet \"{id}\" in {}", dir.display()))
    }

    /// Save `enabled` for applet `id`, then open or close it. Enabling an
    /// open applet does nothing (`Reload` restarts it).
    fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let found = self.find(id)?;
        let mut st = state::load_in(&self.config.state_dir, id);
        if st.enabled != enabled {
            st.enabled = enabled;
            state::save_in(&self.config.state_dir, id, &st).map_err(|e| format!("applet {id}: cannot save its state: {e}"))?;
        }
        if !enabled {
            self.close_applet(id);
            return Ok(());
        }
        if self.applets.borrow().contains_key(id) { return Ok(()); }
        self.open_applet(&found)
    }

    /// Close applet `id`, re-read its manifest and script, and open it
    /// again if it is enabled.
    fn reload(&self, id: &str) -> Result<(), String> {
        let found = self.find(id)?;
        self.close_applet(id);
        if !state::load_in(&self.config.state_dir, id).enabled { return Ok(()); }
        self.open_applet(&found)
    }

    /// What a window-manager close of applet `id`'s window does: disable the
    /// applet. Holds only a `Weak<Host>`; a failed upgrade is logged.
    fn close_handler(&self, id: &str) -> impl Fn() + 'static {
        let host = self.me.clone();
        let id = id.to_string();
        move || {
            let Some(host) = host.upgrade() else {
                err_log!(APP_TAG, "applet {id}: window close requested after the host was dropped, ignored");
                return;
            };
            host.disable_from_window(&id);
        }
    }

    /// What a click on the gear of applet `id`'s window does: open its
    /// settings. Holds only a `Weak<Host>`; a failed upgrade is logged.
    fn settings_handler(&self, id: &str) -> impl Fn() + 'static {
        let host = self.me.clone();
        let id = id.to_string();
        move || {
            let Some(host) = host.upgrade() else {
                err_log!(APP_TAG, "applet {id}: settings requested after the host was dropped, ignored");
                return;
            };
            host.open_settings(&id);
        }
    }

    /// What a move or resize of applet `id`'s window by the window manager
    /// (or a drag of its edit handle) does: save the new geometry. Holds only a `Weak<Host>`; a failed
    /// upgrade and a failed save are logged.
    fn geometry_handler(&self, id: &str) -> impl Fn(Rect) + 'static {
        let host = self.me.clone();
        let id = id.to_string();
        move |rect| {
            let Some(host) = host.upgrade() else {
                err_log!(APP_TAG, "applet {id}: window moved to {rect:?} after the host was dropped, not saved");
                return;
            };
            if let Err(e) = host.save_geometry(&id, rect) { err_log!(APP_TAG, "{e}"); }
        }
    }

    /// Save `rect` (x, y, width, height) as the geometry of the open applet
    /// `id`, keeping the rest of its state. `Ok(true)` when the state file
    /// was written, `Ok(false)` when it already had that geometry.
    pub fn save_geometry(&self, id: &str, rect: Rect) -> Result<bool, String> {
        if !self.applets.borrow().contains_key(id) { return Err(format!("applet {id}: geometry {rect:?} not saved, the applet is not open")); }
        let mut st = state::load_in(&self.config.state_dir, id);
        let (x, y, width, height) = rect;
        if (st.x, st.y, st.width, st.height) == (Some(x), Some(y), Some(width), Some(height)) { return Ok(false); }
        (st.x, st.y, st.width, st.height) = (Some(x), Some(y), Some(width), Some(height));
        state::save_in(&self.config.state_dir, id, &st).map_err(|e| format!("applet {id}: cannot save its geometry {rect:?}: {e}"))?;
        debug_log!(APP_TAG, "applet {id}: geometry {rect:?} saved");
        Ok(true)
    }

    /// The target geometry of the window of applet `id`, while it is open.
    pub fn target_of(&self, id: &str) -> Option<Rect> {
        self.applets.borrow().get(id).map(|a| a.window.target())
    }

    /// Clamp the windows of every open applet to `monitors` (see
    /// [`AppletWindow::reclamp`]; the saved geometry is not rewritten).
    pub fn reclamp_windows(&self, monitors: &[Rect]) {
        let applets: Vec<Rc<Applet>> = self.applets.borrow().values().cloned().collect();
        for applet in applets {
            applet.window.reclamp(monitors);
        }
    }

    /// Clamp the applet windows again whenever the monitors of the default
    /// display change (hotplug, resolution). Call once, at startup. The
    /// handlers are owned by GDK's monitor list (it lives as long as the
    /// display, the whole run) and hold only a `Weak<Host>`; a change after
    /// the host is gone is logged.
    pub fn watch_monitors(&self) {
        let Some(display) = gtk::gdk::Display::default() else {
            err_log!(APP_TAG, "no default display: applet windows are not clamped again when the monitors change");
            return;
        };
        let host = self.me.clone();
        opusdm_ui::watch_monitors(&display, move |monitors| {
            let Some(host) = host.upgrade() else {
                err_log!(APP_TAG, "the monitors changed after the host was dropped; applet windows not clamped");
                return;
            };
            debug_log!(APP_TAG, "monitors changed: {monitors:?}");
            host.reclamp_windows(&monitors);
        });
    }

    /// The user closed the window of applet `id`: save `enabled = false`
    /// and close it, as `SetEnabled { enabled: false }`. A save failure is
    /// logged and the applet still closes.
    fn disable_from_window(&self, id: &str) {
        debug_log!(APP_TAG, "applet {id}: window closed by the user, disabling the applet");
        let mut st = state::load_in(&self.config.state_dir, id);
        if st.enabled {
            st.enabled = false;
            if let Err(e) = state::save_in(&self.config.state_dir, id, &st) {
                err_log!(APP_TAG, "applet {id}: window closed, but enabled = false could not be saved (it opens again next run): {e}");
            }
        }
        self.close_applet(id);
    }

    /// Drop applet `id`: its runtime stops and its window is destroyed.
    fn close_applet(&self, id: &str) {
        let removed = self.applets.borrow_mut().remove(id);
        if removed.is_some() { debug_log!(APP_TAG, "applet {id}: closed"); }
        drop(removed);
    }

    /// Open the window of `found` and start its runtime. A start error
    /// leaves the window open showing it; it is the `Err` too.
    fn open_applet(&self, found: &Discovered) -> Result<(), String> {
        let id = &found.id;
        let manifest = match &found.manifest {
            Ok(m) => m,
            Err(e) => return Err(e.clone()),
        };
        let st = state::load_in(&self.config.state_dir, id);
        let hooks = WindowHooks { on_close: Box::new(self.close_handler(id)), on_moved: Rc::new(self.geometry_handler(id)), on_settings: Rc::new(self.settings_handler(id)) };
        let window = applet_window::open(&self.app, id, manifest, &st, self.config.present_windows, &self.edit, hooks);
        let applet = Rc::new_cyclic(|me| Applet {
            id: id.clone(),
            runtime: RefCell::new(None),
            dispatcher: RefCell::new(None),
            tree: RefCell::new(None),
            pending: RefCell::new(Some(Vec::new())),
            window,
            me: me.clone(),
        });
        // The local `applet` keeps it alive while the script runs; the map
        // owns it from here on, whatever the outcome.
        self.applets.borrow_mut().insert(id.clone(), applet.clone());
        let settings = state::effective_settings(&manifest.settings, &st);
        let started = Runtime::start(id, &found.dir, manifest, settings, sink_for(&applet));
        let pending = applet.pending.take().unwrap_or_default();
        match started {
            Ok(runtime) => {
                *applet.dispatcher.borrow_mut() = Some(runtime.dispatcher());
                *applet.runtime.borrow_mut() = Some(runtime);
                self.start_errors.borrow_mut().remove(id);
                for event in pending {
                    applet.on_event(event);
                }
                debug_log!(APP_TAG, "applet {id}: started");
                Ok(())
            }
            Err(e) => {
                if !pending.is_empty() { debug_log!(APP_TAG, "applet {id}: {} ui events of the failed start dropped", pending.len()); }
                self.start_errors.borrow_mut().insert(id.clone(), e.clone());
                Err(e)
            }
        }
    }
}

/// The sink of applet `applet`'s runtime: it holds only a `Weak`.
fn sink_for(applet: &Rc<Applet>) -> Sink {
    let weak = Rc::downgrade(applet);
    let id = applet.id.clone();
    Rc::new(move |event| {
        let Some(applet) = weak.upgrade() else {
            err_log!(APP_TAG, "applet {id}: {} after its window was closed, dropped", describe(&event));
            return;
        };
        applet.on_event(event);
    })
}

/// A short name of `event` for log lines (never the whole payload).
fn describe(event: &Event) -> String {
    match event {
        Event::Log(_) => "a log line".to_string(),
        Event::Error(m) => format!("the error \"{m}\""),
        Event::Ui(v) => format!("ui {}", v.get("op").and_then(Value::as_str).unwrap_or("?")),
        Event::Redraw { id, .. } => format!("a redraw of canvas \"{id}\""),
    }
}

impl Applet {
    /// Handle one event of the runtime.
    fn on_event(&self, event: Event) {
        let event = match event {
            Event::Log(line) => return debug_log!(APP_TAG, "applet {}: {line}", self.id),
            Event::Error(message) => return self.report(&message),
            other => other,
        };
        if let Some(pending) = self.pending.borrow_mut().as_mut() {
            pending.push(event);
            return;
        }
        match event {
            Event::Ui(value) => self.on_ui(&value),
            Event::Redraw { id, commands } => self.on_redraw(&id, &commands),
            Event::Log(_) | Event::Error(_) => {}
        }
    }

    /// Log `message` and show it in the window.
    fn report(&self, message: &str) {
        err_log!(APP_TAG, "applet {}: {message}", self.id);
        self.window.show_error(message);
    }

    /// `{ "op": "show", "tree" }` or `{ "op": "update", "id", "props" }`.
    fn on_ui(&self, value: &Value) {
        match value.get("op").and_then(Value::as_str) {
            Some("show") => self.show(value.get("tree").unwrap_or(&Value::Null)),
            Some("update") => {
                let Some(id) = value.get("id").and_then(Value::as_str) else { return self.report(&format!("ui update without an id: {value}")) };
                let result = self.with_tree(|tree| widgets::apply_update(tree, id, value.get("props").unwrap_or(&Value::Null)));
                if let Err(e) = result { self.report(&e); }
            }
            _ => self.report(&format!("internal: unknown ui event {value}")),
        }
    }

    /// Build and show a new tree. On failure the old widgets go too (the
    /// script has already forgotten their handlers) and the error is shown.
    fn show(&self, tree: &Value) {
        let Some(dispatcher) = self.dispatcher.borrow().clone() else { return self.report("internal: ui show without a runtime") };
        let built = ui_tree::parse(tree).and_then(|node| widgets::build(&node, dispatcher));
        let tree = match built {
            Ok(tree) => tree,
            Err(e) => {
                self.window.set_content(None);
                self.set_tree(None);
                return self.report(&format!("opus.ui.show: {e}"));
            }
        };
        widgets::set_error_listener(&tree, self.canvas_listener());
        self.window.set_content(Some(&tree.root));
        self.set_tree(Some(tree));
    }

    /// Store `tree` as the shown tree.
    fn set_tree(&self, tree: Option<BuiltTree>) {
        match self.tree.try_borrow_mut() {
            Ok(mut slot) => *slot = tree,
            Err(_) => err_log!(APP_TAG, "applet {}: internal: the widget tree is in use, the new tree is not stored", self.id),
        }
    }

    /// Run `f` on the shown tree; no tree is an error.
    fn with_tree(&self, f: impl FnOnce(&BuiltTree) -> Result<(), String>) -> Result<(), String> {
        let Ok(slot) = self.tree.try_borrow() else { return Err("internal: the widget tree is in use".to_string()) };
        match slot.as_ref() {
            Some(tree) => f(tree),
            None => Err("no widget tree is shown (opus.ui.show failed or was not called)".to_string()),
        }
    }

    /// Give canvas `id` the commands of its latest draw.
    fn on_redraw(&self, id: &str, commands: &Value) {
        let result = canvas::parse(commands).and_then(|commands| self.with_tree(|tree| widgets::set_canvas_commands(tree, id, commands)));
        if let Err(e) = result { self.report(&format!("redraw of canvas \"{id}\": {e}")); }
    }

    /// The listener of the canvas replay errors: shows them in the window
    /// from the main loop (not inside the draw function). Holds a `Weak`.
    fn canvas_listener(&self) -> ErrorListener {
        let weak = self.me.clone();
        let id = self.id.clone();
        Rc::new(move |message: String| {
            let (weak, id) = (weak.clone(), id.clone());
            glib::idle_add_local_once(move || {
                let Some(applet) = weak.upgrade() else {
                    err_log!(APP_TAG, "applet {id}: canvas error after its window was closed: {message}");
                    return;
                };
                applet.window.show_error(&message);
            });
        })
    }
}

/// Answer the requests of the connection threads on this thread's main
/// loop. The task holds only a `Weak<Host>`: a request that arrives after
/// the host is gone is answered with an `Error` and logged. When every
/// sender is gone the loop ends, and that is logged too.
pub fn spawn_request_loop(host: &Rc<Host>, rx: async_channel::Receiver<HostMessage>) {
    let weak = Rc::downgrade(host);
    glib::spawn_future_local(async move {
        while let Ok(message) = rx.recv().await {
            serve(&weak, message);
        }
        err_log!(APP_TAG, "the request channel is closed: the host answers no more requests");
    });
}

/// Answer one message of the request loop.
fn serve(host: &Weak<Host>, message: HostMessage) {
    let Some(host) = host.upgrade() else {
        err_log!(APP_TAG, "{message:?} arrived after the host was dropped");
        if let HostMessage::Request(pending) = message {
            pending.reply(Reply::Error(crate::server::HOST_GONE_MESSAGE.into()));
        }
        return;
    };
    match message {
        HostMessage::Edit(on) => host.set_edit(on),
        HostMessage::Request(pending) => {
            if pending.is_abandoned() {
                debug_log!(APP_TAG, "request {:?} skipped: its client was already answered busy", pending.request());
                return;
            }
            let answer = host.handle(pending.request());
            pending.reply(answer);
        }
    }
}
