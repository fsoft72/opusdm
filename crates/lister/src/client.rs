//! Socket link to the hub: a shared write half plus a reader thread that
//! forwards every `ToLister` onto an async channel the GTK loop drains.

use opusdm_ipc::frame::{recv, recv_tolerant, send, Received};
use opusdm_ipc::function::Function;
use opusdm_ipc::{Bindings, FormatStore, ListerId, Role, Settings, ToHub, ToLister, Welcome};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Mutex;

/// What `Client::connect` returns: the client plus everything the `Welcome`
/// carried (id, roles, settings, bindings, formats and functions).
pub type Connected = (Client, ListerId, Roles, Settings, Bindings, FormatStore, Vec<Function>);

/// The roles the hub reported.
#[derive(Debug, Clone, Default)]
pub struct Roles {
    /// Current SOURCE Lister, if any.
    pub source: Option<ListerId>,
    /// Current DEST Listers (a locked DEST persists; one may be transient).
    pub dests: Vec<ListerId>,
    /// Whether SOURCE is pinned. While true, `SetRole(_, Source)` from any
    /// other Lister is rejected by the hub - the badge cycle uses this to
    /// skip straight to DEST instead of retrying a doomed request.
    pub source_locked: bool,
}

impl Roles {
    /// Whether `id` holds a DEST role.
    pub fn is_dest(&self, id: ListerId) -> bool {
        self.dests.contains(&id)
    }

    /// The role clicking the badge should request next for `id`:
    /// SOURCE -> DEST -> NONE. If SOURCE is pinned by another Lister, a
    /// SetRole(Source) request would just be rejected every time (see the
    /// hub's roles.rs), so skip straight to DEST instead of retrying a
    /// doomed request and looking stuck.
    pub fn next_role(&self, id: ListerId) -> Role {
        let is_source = self.source == Some(id);
        let is_dest = self.is_dest(id);
        let source_unreachable = self.source_locked && self.source != Some(id);
        if is_source {
            Role::Dest
        } else if is_dest {
            Role::None
        } else if source_unreachable {
            Role::Dest
        } else {
            Role::Source
        }
    }
}

/// Something arriving from the hub.
#[derive(Debug)]
pub enum Inbound {
    /// A protocol message.
    Msg(ToLister),
    /// The connection ended; the process should exit.
    HubGone,
}

/// Owns the write half of the hub connection.
pub struct Client {
    write: Mutex<UnixStream>,
    inbox: Option<async_channel::Receiver<Inbound>>,
}

impl Client {
    /// Connect, `Register`, and wait for `Welcome`.
    pub fn connect(
        sock_path: &Path,
        pid: u32,
        initial_path: &Path,
    ) -> io::Result<Connected> {
        let mut stream = UnixStream::connect(sock_path)?;
        send(
            &mut stream,
            &ToHub::Register { pid, initial_path: initial_path.to_path_buf(), version: opusdm_ipc::PROTOCOL_VERSION },
        )?;
        let welcome: Welcome = recv(&mut stream)?
            .ok_or_else(|| io::Error::other("hub closed before Welcome"))?;
        let Welcome { lister_id: id, source, dests, source_locked, settings, bindings, formats, functions, version } = welcome;
        if let Some(warning) = opusdm_ipc::protocol_mismatch(version) {
            opusdm_ipc::err_log!("opusdm-lister", "hub: {warning}");
        }
        let roles = Roles { source, dests, source_locked };
        let reader_stream = stream.try_clone()?;
        let inbox = start_reader(reader_stream);
        Ok((
            Client { write: Mutex::new(stream), inbox: Some(inbox) },
            id,
            roles,
            settings,
            bindings,
            formats,
            functions,
        ))
    }

    /// Connect without registering, for a tool window (Manage Functions)
    /// that is not a Lister: the hub serves its requests and replies to it,
    /// but never counts it as a Lister nor gives it a role.
    pub fn connect_tool(sock_path: &Path) -> io::Result<Client> {
        let stream = UnixStream::connect(sock_path)?;
        let inbox = start_reader(stream.try_clone()?);
        Ok(Client { write: Mutex::new(stream), inbox: Some(inbox) })
    }

    /// Frame and send one message to the hub.
    pub fn send(&self, msg: &ToHub) -> io::Result<()> {
        let mut guard = self.write.lock().unwrap();
        send(&mut *guard, msg)
    }

    /// Take the reader channel. Panics if called twice.
    pub fn take_inbox(&mut self) -> async_channel::Receiver<Inbound> {
        self.inbox.take().expect("take_inbox called more than once")
    }
}

/// Collects the hub's answer to `ToHub::QueryConfig`: one `BindingsUpdate`
/// and one `FunctionsUpdate`, in any order.
#[derive(Default)]
pub struct ConfigReply {
    bindings: Option<Bindings>,
    functions: Option<Vec<Function>>,
}

impl ConfigReply {
    /// Take `msg` into account; returns the bindings and Functions once both
    /// arrived. Other messages are ignored.
    pub fn feed(&mut self, msg: ToLister) -> Option<(Bindings, Vec<Function>)> {
        match msg {
            ToLister::BindingsUpdate(b) => self.bindings = Some(b),
            ToLister::FunctionsUpdate(f) => self.functions = Some(f),
            _ => {}
        }
        if self.bindings.is_none() || self.functions.is_none() { return None; }
        Some((self.bindings.take()?, self.functions.take()?))
    }
}

/// Ask the hub for the bindings and saved Functions in force (the hub owns
/// those files, so a tool window starts from its copy, not from disk).
pub async fn fetch_config(client: &Client, inbox: &async_channel::Receiver<Inbound>) -> Result<(Bindings, Vec<Function>), String> {
    client.send(&ToHub::QueryConfig).map_err(|e| format!("lost the connection to the hub: {e}"))?;
    let mut reply = ConfigReply::default();
    loop {
        match inbox.recv().await {
            Ok(Inbound::Msg(msg)) => {
                if let Some(config) = reply.feed(msg) { return Ok(config); }
            }
            Ok(Inbound::HubGone) | Err(_) => return Err("the hub closed the connection".to_string()),
        }
    }
}

/// Spawn the reader thread for `stream`; returns the channel it feeds.
pub fn start_reader(mut stream: UnixStream) -> async_channel::Receiver<Inbound> {
    let (tx, rx) = async_channel::unbounded::<Inbound>();
    std::thread::Builder::new()
        .name("opusdm-lister-reader".into())
        .spawn(move || loop {
            match recv_tolerant::<_, ToLister>(&mut stream) {
                Ok(Some(Received::Msg(msg))) => {
                    if tx.send_blocking(Inbound::Msg(msg)).is_err() {
                        return;
                    }
                }
                // A newer hub sent something this Lister does not know: skip
                // it and keep the window, the rest of the protocol still works.
                Ok(Some(Received::Unknown(tag))) => {
                    opusdm_ipc::err_log!("opusdm-lister", "ignoring hub message {tag}: this Lister does not know it (another OpusDM version?)");
                }
                Ok(None) => {
                    let _ = tx.send_blocking(Inbound::HubGone);
                    return;
                }
                Err(e) => {
                    opusdm_ipc::err_log!("opusdm-lister", "cannot read from the hub, closing the window: {e}");
                    let _ = tx.send_blocking(Inbound::HubGone);
                    return;
                }
            }
        })
        .expect("spawn reader thread");
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::frame::{recv, send};
    use opusdm_ipc::{ListerId, ToHub, ToLister, Welcome};
    use std::os::unix::net::UnixListener;

    #[test]
    fn config_reply_waits_for_both_parts_in_any_order() {
        let mut reply = ConfigReply::default();
        assert!(reply.feed(ToLister::FunctionsUpdate(vec![Function::new("f")])).is_none());
        assert!(reply.feed(ToLister::Pong).is_none(), "other messages are ignored");
        let (bindings, functions) = reply.feed(ToLister::BindingsUpdate(Bindings::default())).expect("complete");
        assert_eq!(bindings, Bindings::default());
        assert_eq!(functions[0].name, "f");
    }

    #[test]
    fn fetch_config_asks_the_hub_and_returns_its_answer() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let got: ToHub = recv(&mut s).unwrap().unwrap();
            assert!(matches!(got, ToHub::QueryConfig));
            send(&mut s, &ToLister::BindingsUpdate(Bindings::default())).unwrap();
            send(&mut s, &ToLister::FunctionsUpdate(vec![Function::new("hub copy")])).unwrap();
        });
        let mut client = Client::connect_tool(&path).unwrap();
        let inbox = client.take_inbox();
        let (_, functions) = gtk4::glib::MainContext::default().block_on(fetch_config(&client, &inbox)).unwrap();
        assert_eq!(functions[0].name, "hub copy");
        server.join().unwrap();
        // The hub going away is an error, not a hang.
        assert!(gtk4::glib::MainContext::default().block_on(fetch_config(&client, &inbox)).is_err());
    }

    #[test]
    fn next_role_cycles_source_dest_none() {
        let me = ListerId(1);
        let roles = Roles::default();
        assert_eq!(roles.next_role(me), Role::Source);

        let roles = Roles { source: Some(me), ..Default::default() };
        assert_eq!(roles.next_role(me), Role::Dest);

        let roles = Roles { dests: vec![me], ..Default::default() };
        assert_eq!(roles.next_role(me), Role::None);
    }

    #[test]
    fn next_role_skips_to_dest_when_source_is_locked_elsewhere() {
        let me = ListerId(1);
        let other = ListerId(2);
        let roles = Roles { source: Some(other), source_locked: true, ..Default::default() };
        assert_eq!(roles.next_role(me), Role::Dest);
    }

    #[test]
    fn connect_sends_register_and_parses_welcome() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = UnixListener::bind(&path).unwrap();

        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let got: ToHub = recv(&mut s).unwrap().unwrap();
            assert!(matches!(got, ToHub::Register { pid: 77, version: opusdm_ipc::PROTOCOL_VERSION, .. }));
            send(&mut s, &Welcome {
                lister_id: ListerId(5),
                source: Some(ListerId(5)),
                dests: vec![],
                source_locked: false,
                settings: opusdm_ipc::Settings::default(),
                bindings: opusdm_ipc::Bindings::default(),
                formats: opusdm_ipc::FormatStore::default(),
                functions: Vec::new(),
                version: opusdm_ipc::PROTOCOL_VERSION,
            }).unwrap();
            send(&mut s, &ToLister::InvalidatePath("/tmp".into())).unwrap();
        });

        let (mut client, id, roles, _settings, _bindings, _formats, _functions) =
            Client::connect(&path, 77, std::path::Path::new("/home/x")).unwrap();
        assert_eq!(id, ListerId(5));
        assert_eq!(roles.source, Some(ListerId(5)));

        let inbox = client.take_inbox();
        match inbox.recv_blocking().unwrap() {
            Inbound::Msg(ToLister::InvalidatePath(p)) => {
                assert_eq!(p, std::path::PathBuf::from("/tmp"))
            }
            other => panic!("{other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn reader_skips_an_unknown_hub_message_and_keeps_the_window() {
        let (mut hub, lister) = std::os::unix::net::UnixStream::pair().unwrap();
        opusdm_ipc::frame::write_frame(&mut hub, br#"{"FromTheFuture":{"x":1}}"#).unwrap();
        opusdm_ipc::frame::write_frame(&mut hub, br#""AlsoNew""#).unwrap();
        send(&mut hub, &ToLister::Pong).unwrap();
        drop(hub);
        let inbox = start_reader(lister);
        assert!(matches!(inbox.recv_blocking(), Ok(Inbound::Msg(ToLister::Pong))), "the known message after the unknown ones arrives");
        assert!(matches!(inbox.recv_blocking(), Ok(Inbound::HubGone)));
    }

    #[test]
    fn reader_reports_hub_gone_on_eof() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _got: ToHub = recv(&mut s).unwrap().unwrap();
            send(&mut s, &Welcome {
                lister_id: ListerId(1), source: None, dests: vec![],
                source_locked: false,
                settings: opusdm_ipc::Settings::default(),
                bindings: opusdm_ipc::Bindings::default(),
                formats: opusdm_ipc::FormatStore::default(),
                functions: Vec::new(),
                version: opusdm_ipc::PROTOCOL_VERSION,
            }).unwrap();
        });
        let (mut client, _id, _roles, _settings, _bindings, _formats, _functions) =
            Client::connect(&path, 1, std::path::Path::new("/")).unwrap();
        let inbox = client.take_inbox();
        loop {
            match inbox.recv_blocking() {
                Ok(Inbound::HubGone) => break,
                Ok(_) => continue,
                Err(_) => panic!("channel closed before HubGone"),
            }
        }
        server.join().unwrap();
    }

    #[test]
    fn connect_returns_the_bindings_from_welcome() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let sent = opusdm_ipc::Bindings {
            hotkeys: vec![opusdm_ipc::bindings::Hotkey { key: "Ctrl+u".into(), function: "f".into(), global: false }],
            ..opusdm_ipc::Bindings::default()
        };
        let to_send = sent.clone();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _got: ToHub = recv(&mut s).unwrap().unwrap();
            send(&mut s, &Welcome {
                lister_id: ListerId(1), source: None, dests: vec![],
                source_locked: false,
                settings: opusdm_ipc::Settings::default(),
                bindings: to_send,
                formats: opusdm_ipc::FormatStore::default(),
                functions: Vec::new(),
                version: opusdm_ipc::PROTOCOL_VERSION,
            }).unwrap();
        });
        let (_client, _id, _roles, _settings, bindings, _formats, _functions) =
            Client::connect(&path, 1, std::path::Path::new("/")).unwrap();
        assert_eq!(bindings, sent);
        server.join().unwrap();
    }

    #[test]
    fn connect_returns_the_formats_from_welcome() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let mut sent = opusdm_ipc::FormatStore::default();
        sent.upsert("/tmp".into(), true, opusdm_ipc::ListerFormat::builtin());
        let to_send = sent.clone();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _got: ToHub = recv(&mut s).unwrap().unwrap();
            send(&mut s, &Welcome {
                lister_id: ListerId(1), source: None, dests: vec![],
                source_locked: false,
                settings: opusdm_ipc::Settings::default(),
                bindings: opusdm_ipc::Bindings::default(),
                formats: to_send,
                functions: Vec::new(),
                version: opusdm_ipc::PROTOCOL_VERSION,
            }).unwrap();
        });
        let (_client, _id, _roles, _settings, _bindings, formats, _functions) =
            Client::connect(&path, 1, std::path::Path::new("/")).unwrap();
        assert_eq!(formats, sent);
        server.join().unwrap();
    }
}
