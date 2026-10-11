//! The password dialogs: one window per request of polkitd. It runs the
//! polkit helper for the chosen user and shows what PAM asks (the password
//! prompt, errors, notices). A wrong password starts a new helper run and
//! keeps the dialog open; success, Cancel or a cancel from polkitd close it.

use crate::agent::{AgentMsg, Outcome, Request};
use crate::helper::{HelperEvent, Session, HELPER_PATH};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::{Rc, Weak};

/// Width of the dialog in pixels.
const DIALOG_WIDTH: i32 = 440;
/// Size of the request's icon.
const ICON_SIZE: i32 = 48;
/// The icon when the request names none the theme has.
const FALLBACK_ICON: &str = "dialog-password";
/// Width in characters the message wraps at.
const MESSAGE_CHARS: i32 = 50;
/// The prompt shown before PAM sends its own.
const DEFAULT_PROMPT: &str = "Password:";
/// Shown when PAM refuses without a message of its own.
const WRONG_PASSWORD: &str = "Authentication failed. Please try again.";

/// Dialog-specific style, added to the shared OpusDM theme.
pub const POLKIT_CSS: &str = ".polkit-title { font-weight: bold; font-size: 115%; } \
     .polkit-detail, .polkit-info { opacity: 0.7; } \
     .polkit-error { color: #e5484d; }";

/// The open dialogs, by the cookie of their request.
pub struct Dialogs {
    app: gtk::Application,
    open: RefCell<HashMap<String, Rc<Dialog>>>,
}

impl Dialogs {
    /// No dialog open yet; new ones belong to `app`.
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        Rc::new(Self { app: app.clone(), open: RefCell::new(HashMap::new()) })
    }

    /// Act on one message of the agent.
    pub fn handle(self: &Rc<Self>, msg: AgentMsg) {
        match msg {
            AgentMsg::Begin(request) => self.open_dialog(request),
            AgentMsg::Cancel(cookie) => {
                let dialog = self.open.borrow().get(&cookie).cloned();
                if let Some(dialog) = dialog {
                    dialog.finish(Outcome::Cancelled);
                }
            }
        }
    }

    /// Show the dialog of `request` and start the helper for its first user.
    fn open_dialog(self: &Rc<Self>, request: Request) {
        let dialog = Dialog::build(&self.app, request, Rc::downgrade(self));
        self.open.borrow_mut().insert(dialog.cookie.clone(), dialog.clone());
        dialog.start_run();
    }
}

/// One open dialog.
struct Dialog {
    cookie: String,
    reply: async_channel::Sender<Outcome>,
    users: Vec<String>,
    owner: Weak<Dialogs>,
    window: gtk::Window,
    user_choice: Option<gtk::DropDown>,
    prompt: gtk::Label,
    entry: gtk::Entry,
    status: gtk::Label,
    ok: gtk::Button,
    /// The running helper, `None` between runs.
    session: RefCell<Option<Session>>,
    /// Number of the current helper run: events of older runs are dropped.
    run: Cell<u64>,
    /// Whether PAM sent an error during the current run.
    run_had_error: Cell<bool>,
    finished: Cell<bool>,
}

impl Dialog {
    /// Build and show the window of `request`.
    fn build(app: &gtk::Application, request: Request, owner: Weak<Dialogs>) -> Rc<Self> {
        let shell = opusdm_ui::DialogShell::standalone(app, "Authentication Required").resizable(false).size(DIALOG_WIDTH, -1);
        shell.content().append(&header(&request));

        let user_choice = (request.users.len() > 1).then(|| {
            let names: Vec<&str> = request.users.iter().map(String::as_str).collect();
            gtk::DropDown::from_strings(&names)
        });
        shell.content().append(&user_row(&request.users, user_choice.as_ref()));

        let prompt = gtk::Label::new(Some(DEFAULT_PROMPT));
        prompt.set_xalign(0.0);
        let entry = gtk::Entry::new();
        entry.set_visibility(false);
        entry.set_input_purpose(gtk::InputPurpose::Password);
        entry.set_hexpand(true);
        entry.set_sensitive(false);
        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.set_max_width_chars(MESSAGE_CHARS);
        status.set_visible(false);
        shell.content().append(&prompt);
        shell.content().append(&entry);
        shell.content().append(&status);
        shell.content().append(&details(&request));

        let cancel = shell.button("Cancel");
        let ok = shell.button("Authenticate");
        ok.add_css_class("suggested-action");
        ok.set_sensitive(false);

        // Wayland: a centered overlay that holds the keyboard (a no-op on X11).
        opusdm_ui::layer::init(shell.window(), &layer_spec());
        keep_centered_above(shell.window());

        let Request { cookie, users, reply, .. } = request;
        let dialog = Rc::new(Self {
            cookie,
            reply,
            users,
            owner,
            window: shell.window().clone(),
            user_choice,
            prompt,
            entry,
            status,
            ok,
            session: RefCell::new(None),
            run: Cell::new(0),
            run_had_error: Cell::new(false),
            finished: Cell::new(false),
        });
        dialog.wire(&cancel);
        shell.present();
        dialog
    }

    /// Connect the buttons, the entry, the user choice, Escape and closing.
    fn wire(self: &Rc<Self>, cancel: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.entry.connect_activate(move |_| if let Some(d) = weak.upgrade() { d.submit() });
        let weak = Rc::downgrade(self);
        self.ok.connect_clicked(move |_| if let Some(d) = weak.upgrade() { d.submit() });
        let weak = Rc::downgrade(self);
        cancel.connect_clicked(move |_| if let Some(d) = weak.upgrade() { d.finish(Outcome::Cancelled) });
        if let Some(choice) = &self.user_choice {
            let weak = Rc::downgrade(self);
            choice.connect_selected_notify(move |_| if let Some(d) = weak.upgrade() { d.start_run() });
        }
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != gtk::gdk::Key::Escape { return glib::Propagation::Proceed; }
            if let Some(d) = weak.upgrade() { d.finish(Outcome::Cancelled) }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(d) = weak.upgrade() { d.conclude(Outcome::Cancelled) }
            glib::Propagation::Proceed
        });
    }

    /// The user the helper runs for.
    fn selected_user(&self) -> &str {
        let index = self.user_choice.as_ref().map_or(0, |choice| choice.selected() as usize);
        self.users.get(index).or(self.users.first()).map_or("", String::as_str)
    }

    /// End the running helper, if any, and start a new run for the selected
    /// user. Its events arrive on the GTK loop.
    fn start_run(self: &Rc<Self>) {
        self.session.replace(None);
        let run = self.run.get() + 1;
        self.run.set(run);
        self.run_had_error.set(false);
        self.set_answering(false);
        let (session, events) = match Session::start(Path::new(HELPER_PATH), self.selected_user(), &self.cookie) {
            Ok(started) => started,
            Err(e) => {
                opusdm_ipc::err_log!("opusdm-polkit", "cannot start {HELPER_PATH}: {e}");
                self.show_status(&format!("Cannot start the polkit helper: {e}"), true);
                return;
            }
        };
        self.session.replace(Some(session));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(dialog) = weak.upgrade() else { return };
                if dialog.finished.get() || dialog.run.get() != run { return; }
                dialog.on_event(event);
            }
        });
    }

    /// Show one event of the helper.
    fn on_event(self: &Rc<Self>, event: HelperEvent) {
        match event {
            HelperEvent::PromptHidden(text) => self.ask(&text, false),
            HelperEvent::PromptVisible(text) => self.ask(&text, true),
            HelperEvent::Error(text) => {
                self.run_had_error.set(true);
                self.show_status(&text, true);
            }
            HelperEvent::Info(text) => self.show_status(&text, false),
            HelperEvent::Success => self.finish(Outcome::Authenticated),
            HelperEvent::Failure => {
                if !self.run_had_error.get() {
                    self.show_status(WRONG_PASSWORD, true);
                }
                self.start_run();
            }
        }
    }

    /// Let the user answer PAM's `prompt`, the typing hidden unless `visible`.
    fn ask(&self, prompt: &str, visible: bool) {
        self.prompt.set_text(prompt.trim());
        self.entry.set_visibility(visible);
        self.set_answering(true);
        self.entry.grab_focus();
    }

    /// Send the typed answer to the helper. The entry is emptied at once so
    /// the password stays in the widget no longer than needed.
    fn submit(&self) {
        if !self.entry.is_sensitive() { return; }
        let answer = self.entry.text();
        self.entry.set_text("");
        self.set_answering(false);
        self.status.set_visible(false);
        let sent = self.session.borrow_mut().as_mut().map(|session| session.respond(&answer));
        if let Some(Err(e)) = sent {
            opusdm_ipc::err_log!("opusdm-polkit", "cannot answer the polkit helper: {e}");
            self.show_status(&format!("Cannot talk to the polkit helper: {e}"), true);
        }
    }

    /// Enable or disable the entry and the Authenticate button.
    fn set_answering(&self, answering: bool) {
        self.entry.set_sensitive(answering);
        self.ok.set_sensitive(answering);
    }

    /// Show `text` under the entry, styled as an error or as a notice.
    fn show_status(&self, text: &str, error: bool) {
        self.status.set_text(text);
        self.status.set_css_classes(&[if error { "polkit-error" } else { "polkit-info" }]);
        self.status.set_visible(true);
    }

    /// End the request with `outcome` and close the window.
    fn finish(&self, outcome: Outcome) {
        if self.finished.get() { return; }
        self.conclude(outcome);
        self.window.close();
    }

    /// End the request with `outcome`: stop the helper, answer polkitd and
    /// forget the dialog. The window is left to the caller. Runs once.
    fn conclude(&self, outcome: Outcome) {
        if self.finished.replace(true) { return; }
        self.session.replace(None);
        let _ = self.reply.try_send(outcome);
        if let Some(owner) = self.owner.upgrade() {
            owner.open.borrow_mut().remove(&self.cookie);
        }
    }
}

/// The icon, title and message of `request`.
fn header(request: &Request) -> gtk::Box {
    let icon = gtk::Image::from_icon_name(icon_name(&request.icon_name));
    icon.set_pixel_size(ICON_SIZE);
    icon.set_valign(gtk::Align::Start);
    let title = gtk::Label::new(Some("Authentication Required"));
    title.add_css_class("polkit-title");
    title.set_xalign(0.0);
    let message = gtk::Label::new(Some(&request.message));
    message.set_xalign(0.0);
    message.set_wrap(true);
    message.set_max_width_chars(MESSAGE_CHARS);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    texts.append(&title);
    texts.append(&message);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.append(&icon);
    row.append(&texts);
    row
}

/// The request's icon when the theme has it, otherwise a padlock.
fn icon_name(requested: &str) -> &str {
    if requested.is_empty() { return FALLBACK_ICON; }
    let known = gtk::gdk::Display::default().is_some_and(|display| gtk::IconTheme::for_display(&display).has_icon(requested));
    if known { requested } else { FALLBACK_ICON }
}

/// The user who authenticates: a choice when there are several.
fn user_row(users: &[String], choice: Option<&gtk::DropDown>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some("User:"));
    row.append(&label);
    match choice {
        Some(choice) => {
            choice.set_hexpand(true);
            row.append(choice);
        }
        None => row.append(&gtk::Label::new(users.first().map(String::as_str))),
    }
    row
}

/// The action id and the details polkitd sent, folded away.
fn details(request: &Request) -> gtk::Expander {
    let grid = gtk::Grid::builder().column_spacing(8).row_spacing(2).build();
    let mut rows: Vec<(&str, &str)> = vec![("Action", request.action_id.as_str())];
    let mut extra: Vec<(&str, &str)> = request.details.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    extra.sort();
    rows.extend(extra);
    for (at, (key, value)) in rows.into_iter().enumerate() {
        for (column, text) in [(0, key), (1, value)] {
            let label = gtk::Label::new(Some(text));
            label.add_css_class("polkit-detail");
            label.set_xalign(0.0);
            label.set_selectable(column == 1);
            label.set_wrap(true);
            grid.attach(&label, column, at as i32, 1, 1);
        }
    }
    let expander = gtk::Expander::new(Some("Details"));
    expander.set_child(Some(&grid));
    expander
}

/// Layer shell placement: an overlay centered on the monitor the compositor
/// picks, above every window, holding the keyboard while it is open.
fn layer_spec() -> opusdm_ui::layer::LayerSpec {
    use opusdm_ui::layer::{Anchors, Exclusive, KeyboardMode, Layer, LayerSpec, Margins};
    LayerSpec {
        layer: Layer::Overlay,
        namespace: "opusdm-polkit",
        anchors: Anchors::default(),
        margins: Margins::default(),
        exclusive: Exclusive::None,
        keyboard: KeyboardMode::Exclusive,
        monitor: None,
    }
}

/// Top left corner that centers a `size` window on `monitor` (x, y, width,
/// height).
pub fn centered(monitor: (i32, i32, i32, i32), size: (i32, i32)) -> (i32, i32) {
    let (x, y, w, h) = monitor;
    (x + (w - size.0).max(0) / 2, y + (h - size.1).max(0) / 2)
}

/// X11: center the window on the primary monitor once it is mapped and keep
/// it above the others. The window manager places a window after map and
/// drops early state messages, so this is repeated for a second.
fn keep_centered_above(window: &gtk::Window) {
    if opusdm_ui::layer::is_layer(window) { return; }
    window.connect_map(move |window| {
        let window = window.clone();
        opusdm_ui::retry_after_map(&opusdm_ui::PLACEMENT_RETRY_MS, move || {
            let Some(xid) = x11_window_id(&window) else { return };
            let monitor = gtk::gdk::Display::default()
                .as_ref()
                .map(opusdm_ui::list_monitors)
                .and_then(|rects| rects.first().copied())
                .unwrap_or((0, 0, 1280, 800));
            let size = (window.width().max(1), window.height().max(1));
            let (x, y) = centered(monitor, size);
            let placed = opusdm_ui::with_x11(|x11| {
                x11.pin_geometry(xid, x, y, size.0 as u32, size.1 as u32)?;
                x11.add_wm_states(xid, &["_NET_WM_STATE_ABOVE"])
            });
            if let Err(e) = placed {
                opusdm_ipc::err_log!("opusdm-polkit", "cannot place the dialog: {e}");
            }
        });
    });
}

/// The X11 window id backing `window`, if it runs under X11.
fn x11_window_id(window: &gtk::Window) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_is_centered_on_its_monitor() {
        assert_eq!(centered((0, 0, 1920, 1080), (440, 300)), (740, 390));
        assert_eq!(centered((1920, 100, 1280, 800), (440, 300)), (1920 + 420, 100 + 250));
    }

    #[test]
    fn a_monitor_smaller_than_the_dialog_keeps_its_corner() {
        assert_eq!(centered((10, 20, 300, 200), (440, 300)), (10, 20));
    }
}
