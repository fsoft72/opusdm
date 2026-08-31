//! Socket link to the hub: a shared write half plus a reader thread that
//! forwards every `ToLister` onto an async channel the GTK loop drains.

use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::{ListerId, ToHub, ToLister};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Mutex;

/// The roles the hub reported.
#[derive(Debug, Clone, Copy)]
pub struct Roles {
    /// Current SOURCE Lister, if any.
    pub source: Option<ListerId>,
    /// Current DEST Lister, if any.
    pub dest: Option<ListerId>,
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
    ) -> io::Result<(Client, ListerId, Roles)> {
        let mut stream = UnixStream::connect(sock_path)?;
        send(&mut stream, &ToHub::Register { pid, initial_path: initial_path.to_path_buf() })?;
        let welcome: ToLister = recv(&mut stream)?
            .ok_or_else(|| io::Error::other("hub closed before Welcome"))?;
        let (id, roles) = match welcome {
            ToLister::Welcome { lister_id, source, dest } => (lister_id, Roles { source, dest }),
            other => {
                return Err(io::Error::other(format!("expected Welcome, got {other:?}")));
            }
        };
        let reader_stream = stream.try_clone()?;
        let inbox = start_reader(reader_stream);
        Ok((
            Client { write: Mutex::new(stream), inbox: Some(inbox) },
            id,
            roles,
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
    use opusdm_ipc::{ListerId, ToHub, ToLister};
    use std::os::unix::net::UnixListener;

    #[test]
    fn connect_sends_register_and_parses_welcome() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = UnixListener::bind(&path).unwrap();

        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let got: ToHub = recv(&mut s).unwrap().unwrap();
            assert!(matches!(got, ToHub::Register { pid: 77, .. }));
            send(&mut s, &ToLister::Welcome {
                lister_id: ListerId(5),
                source: Some(ListerId(5)),
                dest: None,
            }).unwrap();
            send(&mut s, &ToLister::InvalidatePath("/tmp".into())).unwrap();
        });

        let (mut client, id, roles) =
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
            send(&mut s, &ToLister::Welcome {
                lister_id: ListerId(1), source: None, dest: None,
            }).unwrap();
        });
        let (mut client, _id, _roles) =
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
}
