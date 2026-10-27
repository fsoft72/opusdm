//! The Applet Browser window: one row per applet (name, version,
//! description, an enabled switch, Reload, Export and Delete icon buttons and
//! the error in red), an "Edit mode" switch, an Import button and a Refresh
//! button.
//!
//! Every socket request runs on its own thread (`request_at` blocks for up
//! to 7 s when the applets process is busy) and its reply comes back to the
//! GTK main loop through an `async_channel`, so the window never waits.
//! While a request is in flight its control is insensitive; a failure
//! re-enables it, restores the previous value, shows the error in the window
//! and logs it with `err_log!`.
//!
//! Export and Import ask the user with `gtk::FileDialog` / `gtk::AlertDialog`
//! and do the file work (`package`) on a thread. Import unpacks and checks the
//! archive first, then asks for confirmation with the permissions the archive
//! requests; only a yes installs it (disabled). A cancel is not an error and
//! shows nothing. Delete asks first too; it closes the applet through the
//! applets process (`SetEnabled` false), then removes the folder.
//!
//! Ownership for the whole run: the destroy handler of the window holds the
//! only strong `Rc<Browser>` and releases it when the window is destroyed.
//! Widget handlers and request completions hold only a `Weak<Browser>` (and
//! `glib::WeakRef`s to their widgets): when the window was closed while a
//! request was in flight, the failed upgrade is logged and the reply dropped.
//! The window itself is held by the browser only as a `glib::WeakRef` (it is
//! the parent of the dialogs), so the window and the browser do not keep each
//! other alive.

use crate::{model, package, APP_CSS};
use gtk4 as gtk;
use gtk::{gio, glib};
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

/// Canned answers for the dialogs, so a test can click Export and Import
/// without a person: no dialog is shown when a browser has them.
#[derive(Clone, Debug, Default)]
pub struct Answers {
    /// Where Export saves the archive (`None` = the user cancelled).
    pub save_to: Option<PathBuf>,
    /// The archive Import opens (`None` = the user cancelled).
    pub open_from: Option<PathBuf>,
    /// The answer to the confirmations (import and delete).
    pub confirm: bool,
}

/// Icons of the row buttons (symbolic, so they follow the theme colors).
const RELOAD_ICON: &str = "view-refresh-symbolic";
const EXPORT_ICON: &str = "document-save-symbolic";
const DELETE_ICON: &str = "user-trash-symbolic";

/// An icon-only button named `name`: `tooltip` is shown on hover and is also
/// its accessible name, so a screen reader does not read an unnamed button.
fn icon_button(name: &str, icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder().name(name).icon_name(icon).tooltip_text(tooltip).valign(gtk::Align::Center).build();
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

/// The widgets the requests update, shared by the handlers of one window.
struct Browser {
    /// The applets socket every request goes to.
    socket: PathBuf,
    /// The folder the applets live in: Export reads it, Import writes it.
    applets_dir: PathBuf,
    /// The window, parent of the dialogs; weak, see the module docs.
    window: glib::WeakRef<gtk::ApplicationWindow>,
    /// Canned dialog answers (tests only).
    answers: Option<Answers>,
    /// The applet rows.
    list: gtk::ListBox,
    /// Shown instead of the rows: "not running" or the empty-folder hint.
    placeholder: gtk::Label,
    /// The last error, in red; hidden when there is none.
    status: gtk::Label,
    /// The last good news ("Exported ..."); hidden when there is none.
    notice: gtk::Label,
    /// Imports an applet from an archive.
    import: gtk::Button,
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

    /// Show `text` as the result of an action that worked.
    fn show_notice(&self, text: &str) {
        self.notice.set_text(text);
        self.notice.set_visible(true);
    }

    /// Hide the error and notice lines: every user action starts with it, so
    /// a message stays visible until the next action, across the refresh that
    /// follows.
    fn clear_error(&self) {
        self.status.set_text("");
        self.status.set_visible(false);
        self.notice.set_text("");
        self.notice.set_visible(false);
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
    open_in(app, socket, opusdm_applets::discovery::applets_dir(), present, None)
}

/// Like [`open_with`] on the applets folder `applets_dir`, with canned
/// dialog `answers` (tests only; `None` shows the real dialogs).
pub fn open_in(app: &gtk::Application, socket: PathBuf, applets_dir: PathBuf, present: bool, answers: Option<Answers>) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder().application(app).title("Applets").default_width(WINDOW_WIDTH).default_height(WINDOW_HEIGHT).build();
    let edit = gtk::Switch::builder().name("edit-switch").valign(gtk::Align::Center).sensitive(false).build();
    let refresh = gtk::Button::builder().name("refresh-button").label("Refresh").build();
    let import = gtk::Button::builder().name("import-button").label("Import").tooltip_text("Install an applet from a .tar.gz").build();
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.append(&gtk::Label::new(Some("Edit mode")));
    header.append(&edit);
    header.append(&gtk::Box::builder().hexpand(true).build());
    header.append(&import);
    header.append(&refresh);
    let status = gtk::Label::builder().name("status").xalign(0.0).wrap(true).selectable(true).visible(false).build();
    status.add_css_class("error");
    let notice = gtk::Label::builder().name("notice").xalign(0.0).wrap(true).selectable(true).visible(false).build();
    notice.add_css_class("dim-label");
    let placeholder = gtk::Label::builder().name("placeholder").xalign(0.0).wrap(true).visible(false).build();
    let list = gtk::ListBox::builder().name("applet-list").selection_mode(gtk::SelectionMode::None).build();
    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).child(&list).build();
    let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(12).margin_bottom(12).margin_start(12).margin_end(12).build();
    content.append(&header);
    content.append(&status);
    content.append(&notice);
    content.append(&placeholder);
    content.append(&scrolled);
    window.set_child(Some(&content));

    let browser = Rc::new(Browser { socket, applets_dir, window: window.downgrade(), answers, list, placeholder, status, notice, import, edit, refresh, quiet: Cell::new(false) });
    connect_edit(&browser);
    connect_refresh(&browser);
    connect_import(&browser);
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
    let reload = icon_button(&format!("reload:{id}"), RELOAD_ICON, "Reload this applet");
    let export = icon_button(&format!("export:{id}"), EXPORT_ICON, "Export this applet as a .tar.gz");
    row.append(&texts);
    row.append(&enabled);
    row.append(&reload);
    let delete = icon_button(&format!("delete:{id}"), DELETE_ICON, "Delete this applet");
    row.append(&export);
    row.append(&delete);
    connect_enabled(browser, &enabled, status);
    connect_reload(browser, &reload, status);
    connect_export(browser, &export, status);
    connect_delete(browser, &delete, status);
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

/// Run `work` on its own thread and wait for its result without blocking the
/// main loop. `what` names the job in errors and in the thread name.
async fn blocking<T: Send + 'static>(what: &'static str, work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    let (tx, rx) = async_channel::bounded::<T>(1);
    std::thread::Builder::new().name(format!("applet-browser-{what}")).spawn(move || {
        if tx.send_blocking(work()).is_err() {
            err_log!(LOG_TAG, "{what}: nobody waits for the result any more");
        }
    }).map_err(|e| format!("cannot start the {what} thread: {e}"))?;
    rx.recv().await.map_err(|_| format!("the {what} thread ended without a result"))
}

/// True if the user closed or cancelled a dialog: not an error.
fn cancelled(e: &glib::Error) -> bool {
    e.matches(gtk::DialogError::Dismissed) || e.matches(gtk::DialogError::Cancelled)
}

/// The window to parent a dialog to; a window already closed is an error.
fn dialog_parent(window: &glib::WeakRef<gtk::ApplicationWindow>) -> Result<gtk::ApplicationWindow, String> {
    window.upgrade().ok_or_else(|| "the window was closed".to_string())
}

/// Ask where to save the archive of applet `name`; `Ok(None)` = cancelled.
async fn ask_save_path(window: &glib::WeakRef<gtk::ApplicationWindow>, answers: &Option<Answers>, id: &str, name: &str) -> Result<Option<PathBuf>, String> {
    if let Some(a) = answers { return Ok(a.save_to.clone()); }
    let parent = dialog_parent(window)?;
    let dialog = gtk::FileDialog::builder().title(format!("Export {name}")).accept_label("Export").initial_name(package::archive_name(id)).modal(true).build();
    match dialog.save_future(Some(&parent)).await {
        Ok(file) => file.path().map(Some).ok_or_else(|| "the chosen place is not a local file".to_string()),
        Err(e) if cancelled(&e) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Ask which archive to import; `Ok(None)` = cancelled.
async fn ask_open_path(window: &glib::WeakRef<gtk::ApplicationWindow>, answers: &Option<Answers>) -> Result<Option<PathBuf>, String> {
    if let Some(a) = answers { return Ok(a.open_from.clone()); }
    let parent = dialog_parent(window)?;
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Applet archives (.tar.gz)"));
    filter.add_pattern("*.tar.gz");
    filter.add_pattern("*.tgz");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder().title("Import an applet").accept_label("Open").filters(&filters).modal(true).build();
    match dialog.open_future(Some(&parent)).await {
        Ok(file) => file.path().map(Some).ok_or_else(|| "the chosen file is not a local file".to_string()),
        Err(e) if cancelled(&e) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Ask a yes / no question in an alert; Cancel is the default button.
/// `Ok(false)` = cancelled or closed.
async fn ask_alert(window: &glib::WeakRef<gtk::ApplicationWindow>, answers: &Option<Answers>, message: String, detail: String, yes: &str) -> Result<bool, String> {
    if let Some(a) = answers { return Ok(a.confirm); }
    let parent = dialog_parent(window)?;
    let dialog = gtk::AlertDialog::builder().message(message).detail(detail).buttons(["Cancel", yes]).cancel_button(0).default_button(0).modal(true).build();
    match dialog.choose_future(Some(&parent)).await {
        Ok(choice) => Ok(choice == 1),
        Err(e) if cancelled(&e) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// Ask whether to install the unpacked `staged` applet, showing the
/// permissions its own manifest requests.
async fn ask_confirm(window: &glib::WeakRef<gtk::ApplicationWindow>, answers: &Option<Answers>, staged: &package::Staged) -> Result<bool, String> {
    let question = model::import_question(&staged.id, &staged.manifest.name, &staged.manifest.version);
    ask_alert(window, answers, question, model::import_detail(&staged.manifest.permissions, staged.replaces()), "Import").await
}

/// Everything Export does after the click: ask where, write the archive on a
/// thread. `Ok(Some(path))` is where it was written, `Ok(None)` a cancel.
async fn export_flow(window: &glib::WeakRef<gtk::ApplicationWindow>, answers: &Option<Answers>, dir: PathBuf, id: &str, name: &str) -> Result<Option<PathBuf>, String> {
    let Some(chosen) = ask_save_path(window, answers, id, name).await? else { return Ok(None) };
    let path = package::with_archive_extension(chosen.clone());
    // The dialog asked before overwriting `chosen`, not the path with the extension added.
    if path != chosen && path.exists() { return Err(format!("{} already exists", path.display())); }
    let (target, id) = (path.clone(), id.to_string());
    blocking("export", move || package::export_applet(&dir, &id, &target)).await??;
    Ok(Some(path))
}

/// What a finished import tells the window.
struct Imported {
    /// The applet id.
    id: String,
    /// Its name for messages.
    name: String,
    /// True if an installed applet was replaced.
    replaced: bool,
}

/// Everything Import does after the click: ask which archive, unpack and check
/// it on a thread, ask for confirmation, install on a thread. `Ok(None)` is a
/// cancel (nothing installed, the staging folder removed).
async fn import_flow(window: &glib::WeakRef<gtk::ApplicationWindow>, answers: &Option<Answers>, applets_dir: PathBuf) -> Result<Option<Imported>, String> {
    let Some(archive) = ask_open_path(window, answers).await? else { return Ok(None) };
    let staged = blocking("import", move || package::stage_archive(&archive, &applets_dir)).await??;
    if !ask_confirm(window, answers, &staged).await? { return Ok(None) }
    let (id, replaced) = (staged.id.clone(), staged.replaces());
    let name = match staged.manifest.name.trim() { "" => id.clone(), n => n.to_string() };
    blocking("install", move || staged.install()).await??;
    Ok(Some(Imported { id, name, replaced }))
}

/// A click on `button` exports applet `status`.
fn connect_export(browser: &Rc<Browser>, button: &gtk::Button, status: &AppletStatus) {
    let weak = Rc::downgrade(browser);
    let id = status.id.clone();
    let name = model::display_name(status).to_string();
    button.connect_clicked(move |button| {
        let Some(browser) = upgrade(&weak, "export button") else { return };
        browser.clear_error();
        export(&browser, button, id.clone(), name.clone());
    });
}

/// Export applet `id` and show where it went, or the error. A cancel shows
/// nothing: the user chose it.
fn export(browser: &Rc<Browser>, button: &gtk::Button, id: String, name: String) {
    button.set_sensitive(false);
    let weak = Rc::downgrade(browser);
    let button_ref = button.downgrade();
    let (window, answers, dir) = (browser.window.clone(), browser.answers.clone(), browser.applets_dir.join(&id));
    glib::spawn_future_local(async move {
        let result = export_flow(&window, &answers, dir, &id, &name).await;
        let Some(browser) = upgrade(&weak, "Export") else { return };
        match button_ref.upgrade() {
            Some(button) => button.set_sensitive(true),
            None => err_log!(LOG_TAG, "Export {id}: its button is gone (the list was rebuilt)"),
        }
        match result {
            Ok(Some(path)) => browser.show_notice(&format!("Exported {name} to {}", path.display())),
            Ok(None) => {}
            Err(msg) => {
                err_log!(LOG_TAG, "cannot export the applet {id}: {msg}");
                browser.show_error(&format!("Cannot export {name}: {msg}"));
            }
        }
    });
}

/// A click on Import installs an applet from an archive.
fn connect_import(browser: &Rc<Browser>) {
    let weak = Rc::downgrade(browser);
    browser.import.connect_clicked(move |_| {
        let Some(browser) = upgrade(&weak, "import button") else { return };
        browser.clear_error();
        import(&browser);
    });
}

/// Import an applet; on success list again (and reload a replaced applet, so
/// a running one does not keep the old code). A cancel shows nothing.
fn import(browser: &Rc<Browser>) {
    browser.import.set_sensitive(false);
    let weak = Rc::downgrade(browser);
    let (window, answers, dir, socket) = (browser.window.clone(), browser.answers.clone(), browser.applets_dir.clone(), browser.socket.clone());
    glib::spawn_future_local(async move {
        let result = import_flow(&window, &answers, dir).await;
        let reload = match &result {
            Ok(Some(done)) if done.replaced => Some(call(socket, Request::Reload { id: done.id.clone() }).await),
            _ => None,
        };
        let Some(browser) = upgrade(&weak, "Import") else { return };
        browser.import.set_sensitive(true);
        match result {
            Ok(Some(done)) => {
                match reload.as_ref().and_then(failure) {
                    Some(msg) => {
                        err_log!(LOG_TAG, "imported {} but cannot reload it: {msg}", done.id);
                        browser.show_error(&format!("Imported {}, but reloading it failed: {msg}", done.name));
                    }
                    None => {
                        let how = if done.replaced { "replaced" } else { "disabled" };
                        browser.show_notice(&format!("Imported {} ({how})", done.name));
                    }
                }
                refresh_list(&browser);
            }
            Ok(None) => {}
            Err(msg) => {
                err_log!(LOG_TAG, "cannot import an applet: {msg}");
                browser.show_error(&format!("Cannot import: {msg}"));
            }
        }
    });
}

/// Everything Delete does after the click: ask, close the applet through the
/// applets process, remove the folder on a thread. `Ok(false)` is a cancel.
/// When the applet cannot be closed nothing is removed.
async fn delete_flow(window: &glib::WeakRef<gtk::ApplicationWindow>, answers: &Option<Answers>, socket: PathBuf, applets_dir: PathBuf, id: &str, name: &str) -> Result<bool, String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let detail = model::delete_detail(&applets_dir.join(id), home.as_deref());
    if !ask_alert(window, answers, model::delete_question(name), detail, "Delete").await? { return Ok(false) }
    let closed = call(socket, Request::SetEnabled { id: id.to_string(), enabled: false }).await;
    if let Some(msg) = failure(&closed) { return Err(format!("cannot close it first: {msg}")); }
    let id = id.to_string();
    blocking("delete", move || package::delete_applet(&applets_dir, &id)).await??;
    Ok(true)
}

/// A click on `button` deletes applet `status`.
fn connect_delete(browser: &Rc<Browser>, button: &gtk::Button, status: &AppletStatus) {
    let weak = Rc::downgrade(browser);
    let id = status.id.clone();
    let name = model::display_name(status).to_string();
    button.connect_clicked(move |button| {
        let Some(browser) = upgrade(&weak, "delete button") else { return };
        browser.clear_error();
        delete(&browser, button, id.clone(), name.clone());
    });
}

/// Delete applet `id` after the confirmation and list again. A cancel shows
/// nothing; a failure shows the error and lists again (the applet may have
/// been closed already).
fn delete(browser: &Rc<Browser>, button: &gtk::Button, id: String, name: String) {
    button.set_sensitive(false);
    let weak = Rc::downgrade(browser);
    let button_ref = button.downgrade();
    let (window, answers, dir, socket) = (browser.window.clone(), browser.answers.clone(), browser.applets_dir.clone(), browser.socket.clone());
    glib::spawn_future_local(async move {
        let result = delete_flow(&window, &answers, socket, dir, &id, &name).await;
        let Some(browser) = upgrade(&weak, "Delete") else { return };
        match button_ref.upgrade() {
            Some(button) => button.set_sensitive(true),
            None => err_log!(LOG_TAG, "Delete {id}: its button is gone (the list was rebuilt)"),
        }
        match result {
            Ok(true) => {
                browser.show_notice(&format!("Deleted {name}"));
                refresh_list(&browser);
            }
            Ok(false) => {}
            Err(msg) => {
                err_log!(LOG_TAG, "cannot delete the applet {id}: {msg}");
                browser.show_error(&format!("Cannot delete {name}: {msg}"));
                refresh_list(&browser);
            }
        }
    });
}
