//! The Applet Browser window: one row per applet (name, version,
//! description, an enabled switch, a Reload button and the error in red), an
//! "Edit mode" switch and a Refresh button.
//!
//! Every socket request runs on its own thread (`request_at` blocks for up
//! to 7 s when the applets process is busy) and its reply comes back to the
//! GTK main loop through an `async_channel`, so the window never waits.
//! While a request is in flight its control is insensitive; a failure
//! re-enables it, restores the previous value, shows the error in the window
//! and logs it with `err_log!`.
//!
//! Ownership for the whole run: the destroy handler of the window holds the
//! only strong `Rc<Browser>` and releases it when the window is destroyed.
//! Widget handlers and request completions hold only a `Weak<Browser>` (and
//! `glib::WeakRef`s to their widgets): when the window was closed while a
//! request was in flight, the failed upgrade is logged and the reply dropped.

use crate::{model, APP_CSS};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_applets::protocol::{self, AppletStatus, Reply, Request};
use opusdm_ipc::err_log;
use std::cell::{Cell, RefCell};
use std::io;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

/// Tag used in log lines.
const LOG_TAG: &str = "applet-browser";
/// Name of the settings and CSS of this program.
const APP_NAME: &str = "opusdm-applet-browser";
/// Default size of the window.
const WINDOW_WIDTH: i32 = 640;
const WINDOW_HEIGHT: i32 = 480;

/// The widgets the requests update, shared by the handlers of one window.
struct Browser {
    /// The applets socket every request goes to.
    socket: PathBuf,
    /// The applet rows.
    list: gtk::ListBox,
    /// Shown instead of the rows: "not running" or the empty-folder hint.
    placeholder: gtk::Label,
    /// The last error, in red; hidden when there is none.
    status: gtk::Label,
    /// The edit mode of the applets process.
    edit: gtk::Switch,
    /// Lists the applets again.
    refresh: gtk::Button,
    /// True while the code itself sets a switch: its handler sends nothing.
    quiet: Cell<bool>,
}

impl Browser {
    /// Set `sw` to `active` without sending a request.
    fn set_quietly(&self, sw: &gtk::Switch, active: bool) {
        self.quiet.set(true);
        sw.set_active(active);
        self.quiet.set(false);
    }

    /// Show `text` as the current error (it is logged by the caller).
    fn show_error(&self, text: &str) {
        self.status.set_text(text);
        self.status.set_visible(true);
    }

    /// Hide the error line: every user action starts with it, so an error
    /// stays visible until the next action, across the refresh that follows.
    fn clear_error(&self) {
        self.status.set_text("");
        self.status.set_visible(false);
    }

    /// Show `text` in place of the rows, and no row.
    fn show_placeholder(&self, text: &str) {
        self.clear_rows();
        self.placeholder.set_text(text);
        self.placeholder.set_visible(true);
    }

    /// Remove every applet row.
    fn clear_rows(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
    }
}

/// Open the browser window on the applets socket and present it.
pub fn open(app: &gtk::Application) -> gtk::ApplicationWindow {
    open_with(app, protocol::socket_path(), true)
}

/// Open the browser window on `socket` and list the applets; `present` is
/// false only in tests, where nothing may appear on the screen.
pub fn open_with(app: &gtk::Application, socket: PathBuf, present: bool) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder().application(app).title("Applets").default_width(WINDOW_WIDTH).default_height(WINDOW_HEIGHT).build();
    let edit = gtk::Switch::builder().name("edit-switch").valign(gtk::Align::Center).sensitive(false).build();
    let refresh = gtk::Button::builder().name("refresh-button").label("Refresh").build();
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.append(&gtk::Label::new(Some("Edit mode")));
    header.append(&edit);
    header.append(&gtk::Box::builder().hexpand(true).build());
    header.append(&refresh);
    let status = gtk::Label::builder().name("status").xalign(0.0).wrap(true).selectable(true).visible(false).build();
    status.add_css_class("error");
    let placeholder = gtk::Label::builder().name("placeholder").xalign(0.0).wrap(true).visible(false).build();
    let list = gtk::ListBox::builder().name("applet-list").selection_mode(gtk::SelectionMode::None).build();
    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).child(&list).build();
    let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(12).margin_bottom(12).margin_start(12).margin_end(12).build();
    content.append(&header);
    content.append(&status);
    content.append(&placeholder);
    content.append(&scrolled);
    window.set_child(Some(&content));

    let browser = Rc::new(Browser { socket, list, placeholder, status, edit, refresh, quiet: Cell::new(false) });
    connect_edit(&browser);
    connect_refresh(&browser);
    // OWNER: this destroy handler holds the only strong reference to the
    // browser state for the life of the window and releases it on destroy.
    let keep = RefCell::new(Some(Rc::clone(&browser)));
    window.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    refresh_list(&browser);
    if present { window.present(); }
    window
}

/// The activate handler on the real socket: open or raise the window.
pub fn activate(app: &gtk::Application) {
    activate_with(app, protocol::socket_path(), true);
}

/// The activate handler: a second launch only raises the open window
/// (`present` false only in tests); the first opens it on `socket`.
pub fn activate_with(app: &gtk::Application, socket: PathBuf, present: bool) {
    if let Some(window) = app.windows().first() {
        if present { window.present(); }
        return;
    }
    opusdm_ui::install_app_css(APP_NAME, APP_CSS);
    open_with(app, socket, present);
}

/// Send `req` to `socket` on a new thread and wait for the reply without
/// blocking the main loop.
async fn call(socket: PathBuf, req: Request) -> io::Result<Reply> {
    let (tx, rx) = async_channel::bounded::<io::Result<Reply>>(1);
    let what = format!("{req:?}");
    std::thread::Builder::new().name("applet-browser-request".into()).spawn(move || {
        let reply = protocol::request_at(&socket, &req);
        if tx.send_blocking(reply).is_err() {
            err_log!(LOG_TAG, "request {req:?}: nobody waits for the reply any more");
        }
    })?;
    match rx.recv().await {
        Ok(reply) => reply,
        Err(_) => Err(io::Error::other(format!("request {what}: the request thread ended without a reply"))),
    }
}

/// Why `reply` is not the success of a request (`None` when it is).
fn failure(reply: &io::Result<Reply>) -> Option<String> {
    match reply {
        Ok(Reply::Ok) | Ok(Reply::Edit(_)) => None,
        Ok(Reply::Error(msg)) => Some(msg.clone()),
        Ok(other) => Some(format!("unexpected reply {other:?}")),
        Err(e) => Some(e.to_string()),
    }
}

/// Upgrade the browser of a request completion; a closed window is logged.
fn upgrade(weak: &Weak<Browser>, what: &str) -> Option<Rc<Browser>> {
    let browser = weak.upgrade();
    if browser.is_none() { err_log!(LOG_TAG, "{what}: the window is gone, the reply is dropped"); }
    browser
}

/// Ask the edit mode and the applet list, and show them.
fn refresh_list(browser: &Rc<Browser>) {
    browser.refresh.set_sensitive(false);
    browser.edit.set_sensitive(false);
    let weak = Rc::downgrade(browser);
    let socket = browser.socket.clone();
    glib::spawn_future_local(async move {
        let edit = call(socket.clone(), Request::GetEdit).await;
        let list = match edit {
            Ok(_) => Some(call(socket, Request::List).await),
            Err(_) => None,
        };
        let Some(browser) = upgrade(&weak, "refresh") else { return };
        show_refresh(&browser, edit, list);
    });
}

/// Show the replies of a refresh: `edit` to `GetEdit` and `list` to `List`
/// (`None` when `GetEdit` got no answer, so `List` was not sent).
fn show_refresh(browser: &Rc<Browser>, edit: io::Result<Reply>, list: Option<io::Result<Reply>>) {
    browser.refresh.set_sensitive(true);
    match edit {
        Ok(Reply::Edit(on)) => {
            browser.set_quietly(&browser.edit, on);
            browser.edit.set_sensitive(true);
        }
        Err(e) => {
            err_log!(LOG_TAG, "the applets socket {} does not answer: {e}", browser.socket.display());
            browser.show_placeholder(model::host_unreachable_text());
            browser.show_error(&e.to_string());
            return;
        }
        other => {
            let msg = failure(&other).unwrap_or_else(|| format!("unexpected reply {other:?}"));
            err_log!(LOG_TAG, "cannot read the edit mode: {msg}");
            browser.show_error(&format!("Cannot read the edit mode: {msg}"));
        }
    }
    let Some(list) = list else {
        err_log!(LOG_TAG, "refresh: the applet list was not asked");
        return;
    };
    match list {
        Ok(Reply::Applets(applets)) => show_applets(browser, &applets),
        other => {
            let msg = failure(&other).unwrap_or_else(|| format!("unexpected reply {other:?}"));
            err_log!(LOG_TAG, "cannot list the applets: {msg}");
            browser.show_error(&format!("Cannot list the applets: {msg}"));
        }
    }
}

/// Replace the rows with one per entry of `applets`, or the empty hint.
fn show_applets(browser: &Rc<Browser>, applets: &[AppletStatus]) {
    if applets.is_empty() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        browser.show_placeholder(&model::empty_text(&opusdm_applets::discovery::applets_dir(), home.as_deref()));
        return;
    }
    browser.clear_rows();
    browser.placeholder.set_visible(false);
    for status in applets {
        browser.list.append(&applet_row(browser, status));
    }
}

/// The row of one applet.
fn applet_row(browser: &Rc<Browser>, status: &AppletStatus) -> gtk::Box {
    let id = status.id.as_str();
    let row = gtk::Box::builder().name(format!("row:{id}")).spacing(12).margin_top(6).margin_bottom(6).margin_start(6).margin_end(6).tooltip_text(model::describe(status)).build();
    let texts = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).build();
    let title = gtk::Label::builder().name(format!("title:{id}")).label(model::title(status)).xalign(0.0).build();
    title.add_css_class("heading");
    texts.append(&title);
    if !status.description.trim().is_empty() {
        let description = gtk::Label::builder().label(status.description.trim()).xalign(0.0).wrap(true).build();
        description.add_css_class("dim-label");
        texts.append(&description);
    }
    if let Some(error) = &status.error {
        let label = gtk::Label::builder().name(format!("error:{id}")).label(error).xalign(0.0).wrap(true).selectable(true).build();
        label.add_css_class("error");
        texts.append(&label);
    }
    let enabled = gtk::Switch::builder().name(format!("enabled:{id}")).valign(gtk::Align::Center).active(status.enabled).build();
    let reload = gtk::Button::builder().name(format!("reload:{id}")).label("Reload").valign(gtk::Align::Center).build();
    row.append(&texts);
    row.append(&enabled);
    row.append(&reload);
    connect_enabled(browser, &enabled, status);
    connect_reload(browser, &reload, status);
    row
}

/// A flip of `sw` by the user sends `SetEnabled` for `status`.
fn connect_enabled(browser: &Rc<Browser>, sw: &gtk::Switch, status: &AppletStatus) {
    let weak = Rc::downgrade(browser);
    let id = status.id.clone();
    let name = model::display_name(status).to_string();
    sw.connect_active_notify(move |sw| {
        let Some(browser) = upgrade(&weak, "enabled switch") else { return };
        if browser.quiet.get() { return; }
        browser.clear_error();
        set_enabled(&browser, sw, id.clone(), name.clone());
    });
}

/// Send `SetEnabled` with the state of `sw`; refresh on success. On a
/// failure flip the switch back and show the error at once, then refresh
/// too: a busy process may have done the change after the client gave up,
/// and the list then shows its real state.
fn set_enabled(browser: &Rc<Browser>, sw: &gtk::Switch, id: String, name: String) {
    let enabled = sw.is_active();
    sw.set_sensitive(false);
    let weak = Rc::downgrade(browser);
    let sw_ref = sw.downgrade();
    let socket = browser.socket.clone();
    glib::spawn_future_local(async move {
        let reply = call(socket, Request::SetEnabled { id: id.clone(), enabled }).await;
        let Some(browser) = upgrade(&weak, "SetEnabled") else { return };
        let sw = sw_ref.upgrade();
        if sw.is_none() { err_log!(LOG_TAG, "SetEnabled {id}: its switch is gone (the list was rebuilt)"); }
        if let Some(sw) = &sw { sw.set_sensitive(true); }
        let Some(msg) = failure(&reply) else {
            refresh_list(&browser);
            return;
        };
        let verb = if enabled { "enable" } else { "disable" };
        err_log!(LOG_TAG, "cannot {verb} the applet {id}: {msg}");
        browser.show_error(&format!("Cannot {verb} {name}: {msg}"));
        if let Some(sw) = &sw { browser.set_quietly(sw, !enabled); }
        refresh_list(&browser);
    });
}

/// A click on `button` sends `Reload` for `status`.
fn connect_reload(browser: &Rc<Browser>, button: &gtk::Button, status: &AppletStatus) {
    let weak = Rc::downgrade(browser);
    let id = status.id.clone();
    let name = model::display_name(status).to_string();
    button.connect_clicked(move |button| {
        let Some(browser) = upgrade(&weak, "reload button") else { return };
        browser.clear_error();
        reload(&browser, button, id.clone(), name.clone());
    });
}

/// Send `Reload` for `id` and refresh the list (which shows a script error
/// in the row); a failure is also shown in the error line.
fn reload(browser: &Rc<Browser>, button: &gtk::Button, id: String, name: String) {
    button.set_sensitive(false);
    let weak = Rc::downgrade(browser);
    let button_ref = button.downgrade();
    let socket = browser.socket.clone();
    glib::spawn_future_local(async move {
        let reply = call(socket, Request::Reload { id: id.clone() }).await;
        let Some(browser) = upgrade(&weak, "Reload") else { return };
        match button_ref.upgrade() {
            Some(button) => button.set_sensitive(true),
            None => err_log!(LOG_TAG, "Reload {id}: its button is gone (the list was rebuilt)"),
        }
        let Some(msg) = failure(&reply) else {
            refresh_list(&browser);
            return;
        };
        err_log!(LOG_TAG, "cannot reload the applet {id}: {msg}");
        browser.show_error(&format!("Cannot reload {name}: {msg}"));
        refresh_list(&browser);
    });
}

/// A flip of the edit switch by the user sends `SetEdit`.
fn connect_edit(browser: &Rc<Browser>) {
    let weak = Rc::downgrade(browser);
    browser.edit.connect_active_notify(move |sw| {
        let Some(browser) = upgrade(&weak, "edit switch") else { return };
        if browser.quiet.get() { return; }
        browser.clear_error();
        set_edit(&browser, sw.is_active());
    });
}

/// Send `SetEdit(on)`; show the state the process answers, or flip the
/// switch back on failure.
fn set_edit(browser: &Rc<Browser>, on: bool) {
    browser.edit.set_sensitive(false);
    let weak = Rc::downgrade(browser);
    let socket = browser.socket.clone();
    glib::spawn_future_local(async move {
        let reply = call(socket, Request::SetEdit(on)).await;
        let Some(browser) = upgrade(&weak, "SetEdit") else { return };
        browser.edit.set_sensitive(true);
        if let Ok(Reply::Edit(now)) = reply {
            browser.set_quietly(&browser.edit, now);
            return;
        }
        let Some(msg) = failure(&reply) else { return };
        let verb = if on { "on" } else { "off" };
        err_log!(LOG_TAG, "cannot turn the edit mode {verb}: {msg}");
        browser.show_error(&format!("Cannot turn the edit mode {verb}: {msg}"));
        browser.set_quietly(&browser.edit, !on);
    });
}

/// A click on Refresh lists the applets again.
fn connect_refresh(browser: &Rc<Browser>) {
    let weak = Rc::downgrade(browser);
    browser.refresh.connect_clicked(move |_| {
        let Some(browser) = upgrade(&weak, "refresh button") else { return };
        browser.clear_error();
        refresh_list(&browser);
    });
}

