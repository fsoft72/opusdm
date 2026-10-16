//! Remote locations in the Lister (F-39), the GTK part: sessions, the connect
//! flow (host key approval, password), listing, downloads to a scratch folder
//! and the address book dialog. The pure helpers are in `remote.rs`.

use crate::model::Row;
use crate::remote;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ftp::{AddressBook, RemoteError, RemoteFs, RemoteItem, RemoteUrl, Scheme, Site};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// A connected server, shared with the worker threads that use it.
pub type SharedFs = Arc<Mutex<Box<dyn RemoteFs>>>;

/// Name of the thread that closes remote connections.
const CLOSE_THREAD_NAME: &str = "opusdm-ftp-close";

/// What to do once a connection is ready.
type Then = Box<dyn FnOnce(&Rc<Ui>)>;

/// The Lister's open connections and the passwords typed this session (kept
/// in memory only, to reconnect without asking again).
#[derive(Default)]
pub struct RemoteState {
    sessions: RefCell<HashMap<String, SharedFs>>,
    passwords: RefCell<HashMap<String, String>>,
    /// Cancels the downloads still running when the Lister closes.
    downloads: crate::background::CancelToken,
}

/// Close every session in `sessions` (blocking: run it on a worker).
fn close_sessions(sessions: Vec<SharedFs>) {
    for session in sessions {
        if let Ok(mut fs) = session.lock() {
            fs.close();
        }
    }
}

impl RemoteState {
    /// Forget every connection and password and close the connections on a
    /// thread of their own (a dead server must not block the caller, which
    /// may be a closing window). Returns how many were open.
    pub fn close_all(&self) -> usize {
        let sessions: Vec<SharedFs> = self.sessions.borrow_mut().drain().map(|(_, s)| s).collect();
        self.passwords.borrow_mut().clear();
        let count = sessions.len();
        if count == 0 { return 0; }
        let spawned = std::thread::Builder::new().name(CLOSE_THREAD_NAME.to_string()).spawn(move || close_sessions(sessions));
        if let Err(e) = spawned {
            opusdm_ipc::err_log!("opusdm-lister", "cannot start a thread to close the remote connections: {e}");
        }
        count
    }

    /// The Lister is closing: stop the downloads and close every connection.
    pub fn shutdown(&self) {
        self.downloads.cancel();
        self.close_all();
    }
}

/// A callback that can be taken out once, for use inside `Fn` closures.
type ThenSlot = Rc<RefCell<Option<Then>>>;

/// Connect again with the callback in `slot`, if it is still there.
fn retry(ui: &Rc<Ui>, url: &RemoteUrl, slot: &ThenSlot) {
    if let Some(then) = slot.borrow_mut().take() {
        ui.connect_remote(url.clone(), then);
    }
}

/// List `url` on `session`; meant for a worker thread.
pub fn read_rows(session: &SharedFs, url: &RemoteUrl) -> io::Result<Vec<Row>> {
    let mut fs = session.lock().map_err(|_| io::Error::other("the connection is unusable"))?;
    let entries = fs.list(&url.path).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(remote::rows_from(entries))
}

impl Ui {
    /// The open connection for `url`'s server, if any.
    pub fn remote_session(&self, url: &RemoteUrl) -> Option<SharedFs> {
        self.remote.sessions.borrow().get(&url.session_key()).cloned()
    }

    /// Forget the connection for `url`'s server (it failed or was closed) so
    /// the next visit connects again.
    pub fn drop_remote_session(&self, url: &RemoteUrl) {
        self.remote.sessions.borrow_mut().remove(&url.session_key());
    }

    /// Run `then` once a connection to `url`'s server exists, connecting
    /// first (and asking for approval or a password) when needed.
    pub fn ensure_remote(self: &Rc<Self>, url: &RemoteUrl, then: impl FnOnce(&Rc<Ui>) + 'static) {
        if self.remote_session(url).is_some() {
            then(self);
            return;
        }
        self.connect_remote(url.clone(), Box::new(then));
    }

    /// One connection attempt on a worker thread, then the matching follow-up.
    fn connect_remote(self: &Rc<Self>, url: RemoteUrl, then: Then) {
        let key = url.session_key();
        let mut params = remote::params_for(&url, &opusdm_ftp::book::load_book());
        params.password = self.remote.passwords.borrow().get(&key).cloned();
        crate::actions::flash(self, &format!("connecting to {} ...", url.host));
        let known_hosts = params.known_hosts.clone();
        let ui = self.clone();
        crate::background::run_blocking(move || opusdm_ftp::connect(&params), move |result| {
            match result {
                Ok(fs) => {
                    ui.remote.sessions.borrow_mut().insert(key, Arc::new(Mutex::new(fs)));
                    crate::actions::flash(&ui, &format!("connected to {}", url.host));
                    then(&ui);
                }
                Err(RemoteError::UnknownHost { host, port, fingerprint }) => {
                    ui.ask_trust(url, host, port, fingerprint, known_hosts, then);
                }
                Err(RemoteError::AuthRequired) => ui.ask_password(url, None, then),
                Err(RemoteError::AuthFailed(message)) => {
                    ui.remote.passwords.borrow_mut().remove(&key);
                    ui.ask_password(url, Some(message), then);
                }
                Err(e) => crate::actions::flash(&ui, &format!("{}: {e}", url.host)),
            }
        });
    }

    /// First connection to a host: show its key fingerprint and, when the
    /// user approves, remember it and connect again.
    fn ask_trust(self: &Rc<Self>, url: RemoteUrl, host: String, port: u16, fingerprint: String, file: PathBuf, then: Then) {
        let ui = self.clone();
        let slot: ThenSlot = Rc::new(RefCell::new(Some(then)));
        let detail = format!("SHA-256 host key fingerprint:\n{fingerprint}\n\nTrust this host and continue?");
        opusdm_ui::requester(
            &self.window,
            "Unknown host",
            &format!("The authenticity of {host}:{port} cannot be verified."),
            Some(&detail),
            &[("Trust", true), ("Cancel", false)],
            {
                let url = url.clone();
                let fingerprint = fingerprint.clone();
                move |trusted| {
                    if !trusted {
                        return;
                    }
                    if let Err(e) = opusdm_ftp::book::trust_host(&file, &host, port, &fingerprint) {
                        crate::actions::flash(&ui, &format!("could not save the host key: {e}"));
                        return;
                    }
                    retry(&ui, &url, &slot);
                }
            },
        );
    }

    /// Ask for the password of `url`'s user, then connect again. `message`
    /// is why the previous attempt failed.
    fn ask_password(self: &Rc<Self>, url: RemoteUrl, message: Option<String>, then: Then) {
        let user = url.user.clone().unwrap_or_default();
        let who = if user.is_empty() { url.host.clone() } else { format!("{user}@{}", url.host) };
        let shell = opusdm_ui::DialogShell::new(&self.window, "Password").size(380, 140);
        let root = shell.content();
        let text = match &message {
            Some(m) => format!("Password for {who}\n({m})"),
            None => format!("Password for {who}"),
        };
        let label = gtk::Label::new(Some(&text));
        label.set_xalign(0.0);
        label.set_wrap(true);
        root.append(&label);
        let entry = gtk::PasswordEntry::new();
        entry.set_show_peek_icon(true);
        entry.set_activates_default(true);
        root.append(&entry);
        let connect_btn = shell.button("Connect");
        shell.button_spacer();
        let cancel_btn = shell.close_button("Cancel");
        let window = shell.present();
        window.set_default_widget(Some(&connect_btn));
        entry.grab_focus();

        let ui = self.clone();
        let slot: ThenSlot = Rc::new(RefCell::new(Some(then)));
        {
            let window = window.clone();
            let entry = entry.clone();
            let url = url.clone();
            connect_btn.connect_clicked(move |_| {
                let password = entry.text().to_string();
                window.close();
                ui.remote.passwords.borrow_mut().insert(url.session_key(), password);
                retry(&ui, &url, &slot);
            });
        }
        let _ = cancel_btn;
    }

    /// Download `items` of `url`'s server into a fresh scratch folder on a
    /// worker thread, then call `done` with the local paths.
    pub fn download_remote(self: &Rc<Self>, url: &RemoteUrl, items: Vec<RemoteItem>, done: impl FnOnce(&Rc<Ui>, Vec<PathBuf>) + 'static) {
        let Some(session) = self.remote_session(url) else {
            crate::actions::flash(self, "not connected");
            return;
        };
        let dir = match remote::new_download_dir() {
            Ok(dir) => dir,
            Err(e) => {
                crate::actions::flash(self, &format!("cannot create a download folder: {e}"));
                return;
            }
        };
        enum Msg {
            File(String),
            Done(Result<Vec<PathBuf>, RemoteError>),
        }
        let download = move |out: &crate::background::Emitter<Msg>| {
            let result = match session.lock() {
                Ok(mut fs) => opusdm_ftp::download(fs.as_mut(), &items, &dir, out.cancel().flag(), &mut |p| {
                    out.emit(Msg::File(p.to_string()));
                }),
                Err(_) => Err(RemoteError::Failed("the connection is unusable".to_string())),
            };
            out.emit(Msg::Done(result));
        };
        let ui = self.clone();
        let url = url.clone();
        let mut done = Some(done);
        crate::background::run_streaming(&self.remote.downloads, download, move |msg| match msg {
            Msg::File(path) => crate::actions::flash(&ui, &format!("downloading {path}")),
            Msg::Done(Ok(paths)) => {
                if let Some(done) = done.take() { done(&ui, paths); }
            }
            Msg::Done(Err(e)) => {
                crate::actions::flash(&ui, &format!("download failed: {e}"));
                // A broken connection must not stay in the table. A
                // transfer cancelled mid-file can leave the stream
                // half-read, so it counts as broken too.
                if matches!(e, RemoteError::Failed(_) | RemoteError::Cancelled) {
                    ui.drop_remote_session(&url);
                }
            }
        }, || {});
    }

    /// Close every connection ("FTP Quit"). A Lister showing a remote folder
    /// goes back to the home folder.
    pub fn quit_remote(self: &Rc<Self>) {
        let count = self.remote.close_all();
        if remote::url_of(&self.cwd_path()).is_some() {
            if let Ok(home) = std::env::var("HOME") {
                self.navigate_to(std::path::Path::new(&home));
            }
        }
        crate::actions::flash(self, &format!("closed {count} remote connection(s)"));
    }
}

/// The address book dialog: saved sites with Connect / New / Edit / Delete,
/// and a field to open any address.
pub fn open_address_book(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "FTP Address Book").size(520, 420);
    let root = shell.content();

    let address = gtk::Entry::new();
    address.set_placeholder_text(Some("sftp://user@host/path  (or ftp://, ftps://)"));
    let go = gtk::Button::with_label("Open");
    let address_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    address.set_hexpand(true);
    address_row.append(&address);
    address_row.append(&go);
    root.append(&address_row);

    let list = gtk::ListBox::new();
    let scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
    root.append(&scroll);

    let connect_btn = shell.button("Connect");
    let new_btn = shell.button("New...");
    let edit_btn = shell.button("Edit...");
    let delete_btn = shell.button("Delete");
    shell.button_spacer();
    shell.close_button("Close");

    let book = Rc::new(RefCell::new(opusdm_ftp::book::load_book()));
    let refresh: Rc<dyn Fn()> = {
        let list = list.clone();
        let book = book.clone();
        Rc::new(move || {
            while let Some(row) = list.first_child() {
                list.remove(&row);
            }
            for site in &book.borrow().sites {
                let label = gtk::Label::new(Some(&format!("{}   ({})", site.name, site.url())));
                label.set_xalign(0.0);
                label.set_margin_start(6);
                label.set_margin_top(4);
                label.set_margin_bottom(4);
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&label));
                list.append(&row);
            }
        })
    };
    refresh();
    let window = shell.present();

    let selected = {
        let list = list.clone();
        let book = book.clone();
        Rc::new(move || list.selected_row().and_then(|r| book.borrow().sites.get(r.index() as usize).cloned().map(|s| (r.index() as usize, s))))
    };

    {
        let ui = ui.clone();
        let address = address.clone();
        let window = window.clone();
        go.connect_clicked(move |_| match RemoteUrl::parse(address.text().as_str()) {
            Ok(url) => {
                window.close();
                ui.navigate_to(&remote::path_of(&url));
            }
            Err(e) => crate::actions::flash(&ui, &e),
        });
    }
    {
        let ui = ui.clone();
        let selected = selected.clone();
        let window = window.clone();
        connect_btn.connect_clicked(move |_| {
            let Some((_, site)) = selected() else { return };
            window.close();
            ui.navigate_to(&remote::path_of(&site.url()));
        });
    }
    {
        let ui = ui.clone();
        let book = book.clone();
        let refresh = refresh.clone();
        new_btn.connect_clicked(move |_| edit_site(&ui, &book, None, refresh.clone()));
    }
    {
        let ui = ui.clone();
        let book = book.clone();
        let refresh = refresh.clone();
        let selected = selected.clone();
        edit_btn.connect_clicked(move |_| {
            if let Some((index, _)) = selected() {
                edit_site(&ui, &book, Some(index), refresh.clone());
            }
        });
    }
    {
        let ui = ui.clone();
        let book = book.clone();
        let refresh = refresh.clone();
        delete_btn.connect_clicked(move |_| {
            let Some((index, _)) = selected() else { return };
            book.borrow_mut().sites.remove(index);
            save_book(&ui, &book.borrow());
            refresh();
        });
    }
}

/// Save the address book, telling the user when that fails.
fn save_book(ui: &Rc<Ui>, book: &AddressBook) {
    if let Err(e) = opusdm_ftp::book::save_book(book) {
        crate::actions::flash(ui, &format!("could not save the address book: {e}"));
    }
}

/// The add / edit form of one site.
fn edit_site(ui: &Rc<Ui>, book: &Rc<RefCell<AddressBook>>, index: Option<usize>, refresh: Rc<dyn Fn()>) {
    let site = index.and_then(|i| book.borrow().sites.get(i).cloned()).unwrap_or_default();
    let shell = opusdm_ui::DialogShell::new(&ui.window, if index.is_some() { "Edit Site" } else { "New Site" }).size(420, 360);
    let root = shell.content();

    let entry = |text: &str| {
        let e = gtk::Entry::new();
        e.set_text(text);
        e.set_hexpand(true);
        e
    };
    let name = entry(&site.name);
    let host = entry(&site.host);
    let port = entry(&site.port.to_string());
    let user = entry(&site.user);
    let path = entry(&site.path);
    let protocol = gtk::DropDown::from_strings(&["SFTP (SSH)", "FTP", "FTPS (explicit TLS)"]);
    protocol.set_selected(match site.protocol {
        Scheme::Sftp => 0,
        Scheme::Ftp => 1,
        Scheme::Ftps => 2,
    });
    let passive = gtk::CheckButton::with_label("Passive mode (FTP)");
    passive.set_active(site.passive);
    let verify = gtk::CheckButton::with_label("Verify the server certificate (FTPS)");
    verify.set_active(site.tls_verify);

    let grid = gtk::Grid::builder().row_spacing(6).column_spacing(8).build();
    for (row, (caption, widget)) in [
        ("Name", name.clone().upcast::<gtk::Widget>()),
        ("Protocol", protocol.clone().upcast()),
        ("Host", host.clone().upcast()),
        ("Port", port.clone().upcast()),
        ("User", user.clone().upcast()),
        ("Folder", path.clone().upcast()),
    ]
    .into_iter()
    .enumerate()
    {
        let label = gtk::Label::new(Some(caption));
        label.set_xalign(0.0);
        grid.attach(&label, 0, row as i32, 1, 1);
        grid.attach(&widget, 1, row as i32, 1, 1);
    }
    root.append(&grid);
    root.append(&passive);
    root.append(&verify);
    let hint = gtk::Label::new(Some("Passwords are not stored: they are asked when connecting."));
    hint.set_xalign(0.0);
    root.append(&hint);

    // Moving to another protocol resets the port when it still holds the
    // old protocol's default.
    {
        let port = port.clone();
        protocol.connect_selected_notify(move |d| {
            let scheme = [Scheme::Sftp, Scheme::Ftp, Scheme::Ftps][(d.selected() as usize).min(2)];
            if [Scheme::Sftp, Scheme::Ftp, Scheme::Ftps].iter().any(|s| port.text() == s.default_port().to_string()) {
                port.set_text(&scheme.default_port().to_string());
            }
        });
    }

    let save = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Cancel");
    let window = shell.present();

    let ui = ui.clone();
    let book = book.clone();
    save.connect_clicked(move |_| {
        let Ok(port_number) = port.text().trim().parse::<u16>() else {
            crate::actions::flash(&ui, "the port must be a number from 1 to 65535");
            return;
        };
        let edited = Site {
            name: name.text().trim().to_string(),
            host: host.text().trim().to_string(),
            port: port_number,
            user: user.text().trim().to_string(),
            protocol: [Scheme::Sftp, Scheme::Ftp, Scheme::Ftps][(protocol.selected() as usize).min(2)],
            passive: passive.is_active(),
            tls_verify: verify.is_active(),
            path: path.text().trim().to_string(),
        };
        if let Some(problem) = edited.problem() {
            crate::actions::flash(&ui, &problem);
            return;
        }
        {
            let mut book = book.borrow_mut();
            match index {
                Some(i) if i < book.sites.len() => book.sites[i] = edited,
                _ => book.sites.push(edited),
            }
            save_book(&ui, &book);
        }
        refresh();
        window.close();
    });
}
