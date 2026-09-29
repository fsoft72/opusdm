//! Socket link to the hub: a shared write half plus a reader thread that
//! forwards every `ToLister` onto an async channel the GTK loop drains.

use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::{Bindings, ListerId, Role, Settings, ToHub, ToLister, Welcome};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Mutex;

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
    ) -> io::Result<(Client, ListerId, Roles, Settings, Bindings)> {
        let mut stream = UnixStream::connect(sock_path)?;
        send(&mut stream, &ToHub::Register { pid, initial_path: initial_path.to_path_buf() })?;
        let welcome: Welcome = recv(&mut stream)?
            .ok_or_else(|| io::Error::other("hub closed before Welcome"))?;
        let Welcome { lister_id: id, source, dests, source_locked, settings, bindings } = welcome;
        let roles = Roles { source, dests, source_locked };
        let reader_stream = stream.try_clone()?;
        let inbox = start_reader(reader_stream);
        Ok((
            Client { write: Mutex::new(stream), inbox: Some(inbox) },
            id,
            roles,
            settings,
            bindings,
        ))
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

/// Spawn the reader thread for `stream`; returns the channel it feeds.
pub fn start_reader(mut stream: UnixStream) -> async_channel::Receiver<Inbound> {
    let (tx, rx) = async_channel::unbounded::<Inbound>();
    std::thread::Builder::new()
        .name("opusdm-lister-reader".into())
        .spawn(move || loop {
            match recv::<_, ToLister>(&mut stream) {
                Ok(Some(msg)) => {
                    if tx.send_blocking(Inbound::Msg(msg)).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => {
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
            assert!(matches!(got, ToHub::Register { pid: 77, .. }));
            send(&mut s, &Welcome {
                lister_id: ListerId(5),
                source: Some(ListerId(5)),
                dests: vec![],
                source_locked: false,
                settings: opusdm_ipc::Settings::default(),
                bindings: opusdm_ipc::Bindings::default(),
            }).unwrap();
            send(&mut s, &ToLister::InvalidatePath("/tmp".into())).unwrap();
        });

        let (mut client, id, roles, _settings, _bindings) =
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
            }).unwrap();
        });
        let (mut client, _id, _roles, _settings, _bindings) =
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
            }).unwrap();
        });
        let (_client, _id, _roles, _settings, bindings) =
            Client::connect(&path, 1, std::path::Path::new("/")).unwrap();
        assert_eq!(bindings, sent);
        server.join().unwrap();
    }
}
