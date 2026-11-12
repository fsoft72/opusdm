//! The password window: a short-lived GTK process that asks for the login
//! password and unlocks (or restarts and unlocks) gnome-keyring. The password
//! goes to the unlock call in memory that is zeroed after use. The entry is
//! emptied as soon as the password is taken. (GTK's own copy of the text is
//! not under our control.)

use crate::gnome;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use zeroize::Zeroizing;

/// Tag used in log lines.
const TAG: &str = "opusdm-keyring-watch";
/// The application id of the window process.
const APP_ID: &str = "org.opusdm.KeyringWatch.Ask";
/// Width of the window in pixels.
const WINDOW_WIDTH: i32 = 420;
/// Width in characters the texts wrap at.
const TEXT_CHARS: i32 = 48;
/// Exit code: the keyring was unlocked.
pub const EXIT_OK: i32 = 0;
/// Exit code: the user cancelled.
pub const EXIT_CANCELLED: i32 = 1;
/// Exit code: the unlock failed for another reason than a wrong password.
pub const EXIT_FAILED: i32 = 2;

/// What the window does with the password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskMode {
    /// Unlock the running keyring.
    Unlock,
    /// Restart gnome-keyring and unlock it again.
    Restart,
}

impl AskMode {
    /// The title of the window.
    fn title(&self) -> &'static str {
        match self {
            AskMode::Unlock => "Unlock the keyring",
            AskMode::Restart => "Restart the keyring",
        }
    }

    /// What the window tells the user.
    fn message(&self) -> &'static str {
        match self {
            AskMode::Unlock => "Type your login password to unlock the keyring.",
            AskMode::Restart => "gnome-keyring will be restarted: programs that are using it lose their open session. Type your login password to unlock it again.",
        }
    }

    /// The label of the main button.
    fn action(&self) -> &'static str {
        match self {
            AskMode::Unlock => "Unlock",
            AskMode::Restart => "Restart",
        }
    }
}

/// Show the window and return the exit code of the process.
pub fn run(mode: AskMode) -> i32 {
    let code = Rc::new(Cell::new(EXIT_CANCELLED));
    let app = gtk::Application::builder().application_id(APP_ID).flags(gtk::gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    let result = code.clone();
    app.connect_activate(move |app| build(app, mode, &result));
    // No arguments for GTK: ours (`--ask-password`) are not its own.
    app.run_with_args::<&str>(&[]);
    code.get()
}

/// Build and show the window.
fn build(app: &gtk::Application, mode: AskMode, code: &Rc<Cell<i32>>) {
    let shell = opusdm_ui::DialogShell::standalone(app, mode.title()).resizable(false).size(WINDOW_WIDTH, -1);
    let message = gtk::Label::new(Some(mode.message()));
    message.set_xalign(0.0);
    message.set_wrap(true);
    message.set_max_width_chars(TEXT_CHARS);
    let entry = gtk::PasswordEntry::new();
    entry.set_hexpand(true);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_max_width_chars(TEXT_CHARS);
    status.set_visible(false);
    shell.content().append(&message);
    shell.content().append(&entry);
    shell.content().append(&status);
    shell.close_button("Cancel");
    let ok = shell.button(mode.action());
    ok.add_css_class("suggested-action");
    let window = shell.window().clone();

    let submit = {
        let (entry, status, ok, window, code) = (entry.clone(), status.clone(), ok.clone(), window.clone(), code.clone());
        Rc::new(move || submit_password(mode, &entry, &status, &ok, &window, &code))
    };
    let on_ok = submit.clone();
    ok.connect_clicked(move |_| on_ok());
    entry.connect_activate(move |_| submit());
    shell.present();
    entry.grab_focus();
}

/// Take the password out of the entry and run the unlock on a worker thread,
/// then show the outcome.
fn submit_password(mode: AskMode, entry: &gtk::PasswordEntry, status: &gtk::Label, ok: &gtk::Button, window: &gtk::Window, code: &Rc<Cell<i32>>) {
    let password = Zeroizing::new(entry.text().to_string());
    if password.is_empty() {
        show_status(status, "Type the password first.");
        return;
    }
    entry.set_text("");
    entry.set_sensitive(false);
    ok.set_sensitive(false);
    show_status(status, "Working...");
    let (tx, rx) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new().name("opusdm-keyring-unlock".into()).spawn(move || {
        let result = match mode {
            AskMode::Unlock => gnome::unlock_with_password(password),
            AskMode::Restart => gnome::restart(password),
        };
        if tx.send_blocking(result).is_err() { opusdm_ipc::err_log!(TAG, "the password window was closed before the unlock ended"); }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(TAG, "cannot start the unlock thread: {e}");
        code.set(EXIT_FAILED);
        show_status(status, &format!("Cannot start the unlock: {e}"));
        entry.set_sensitive(true);
        ok.set_sensitive(true);
        return;
    }
    let (entry, status, ok, window, code) = (entry.clone(), status.clone(), ok.clone(), window.clone(), code.clone());
    glib::spawn_future_local(async move {
        let result = rx.recv().await;
        match result {
            Ok(Ok(())) => {
                code.set(EXIT_OK);
                window.close();
                return;
            }
            Ok(Err(e)) if e == gnome::WRONG_PASSWORD => show_status(&status, gnome::WRONG_PASSWORD),
            Ok(Err(e)) => {
                opusdm_ipc::err_log!(TAG, "unlock failed: {e}");
                code.set(EXIT_FAILED);
                show_status(&status, &e);
            }
            Err(e) => {
                opusdm_ipc::err_log!(TAG, "the unlock thread ended without an answer: {e}");
                code.set(EXIT_FAILED);
                show_status(&status, "The unlock ended without an answer.");
            }
        }
        entry.set_sensitive(true);
        ok.set_sensitive(true);
        entry.grab_focus();
    });
}

/// Show `text` in the status label.
fn show_status(status: &gtk::Label, text: &str) {
    status.set_text(text);
    status.set_visible(true);
}
