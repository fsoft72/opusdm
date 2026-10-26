//! Wire protocol of the `opusdm-applets` socket: length-prefixed JSON frames
//! (`opusdm_ipc::frame`), one `Reply` per `Request`.

use serde::{Deserialize, Serialize};
use opusdm_ipc::frame::{recv, send};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// Environment variable that overrides the applets socket path.
pub const ENV_SOCK: &str = "OPUSDM_APPLETS_SOCK";
/// File name of the applets socket, next to the hub socket.
pub const SOCK_FILE_NAME: &str = "applets.sock";
/// Connect-side read and write timeout of the liveness probe (`GetEdit`).
pub const CLIENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Connect-side read and write timeout of every other request: longer than
/// the server's reply timeout, so the client sees its busy answer.
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(crate::server::REPLY_TIMEOUT.as_secs() + 2);

/// A request sent to the applets process.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Request {
    /// List every discovered applet with its status.
    List,
    /// Enable or disable one applet.
    SetEnabled { id: String, enabled: bool },
    /// Reload one applet from disk.
    Reload { id: String },
    /// Turn the edit mode of the applets on or off.
    SetEdit(bool),
    /// Ask whether the edit mode is on (also the liveness probe).
    GetEdit,
}

/// The answer to one `Request`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Reply {
    /// Answer to `List`.
    Applets(Vec<AppletStatus>),
    /// Answer to `GetEdit` (and `SetEdit`): the edit mode state.
    Edit(bool),
    /// The request was done.
    Ok,
    /// The request failed; the text says why.
    Error(String),
}

/// Status of one applet as shown by the Applet Browser.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AppletStatus {
    pub id: String,
    pub name: Option<String>,
    pub description: String,
    pub version: String,
    pub enabled: bool,
    pub running: bool,
    pub error: Option<String>,
}

/// Path of the applets socket: `$OPUSDM_APPLETS_SOCK`, else `applets.sock`
/// in the folder of the hub socket (the temp dir if that has no parent).
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os(ENV_SOCK).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let hub = opusdm_ipc::sockpath::hub_sock_path();
    let dir = hub.parent().map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
    dir.join(SOCK_FILE_NAME)
}

/// Send `req` to the socket at `path` and return the one reply. A missing
/// socket, a timeout or an EOF before the reply is an `io::Error`.
pub fn request_at(path: &Path, req: &Request) -> io::Result<Reply> {
    let mut s = UnixStream::connect(path)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot connect to {}: {e}", path.display())))?;
    let timeout = client_timeout(req);
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    send(&mut s, req)?;
    match recv::<_, Reply>(&mut s) {
        Ok(Some(reply)) => Ok(reply),
        Ok(None) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "applets socket closed before a reply")),
        Err(e) => Err(io::Error::new(e.kind(), format!("no reply from the applets socket: {e}"))),
    }
}

/// The client timeout of `req`: short for the probe, longer than the
/// server's reply timeout for the rest.
pub fn client_timeout(req: &Request) -> std::time::Duration {
    match req {
        Request::GetEdit => CLIENT_TIMEOUT,
        _ => REQUEST_TIMEOUT,
    }
}

/// `request_at` on `socket_path()`.
pub fn request(req: &Request) -> io::Result<Reply> {
    request_at(&socket_path(), req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::frame::{recv, send};
    use std::os::unix::net::{UnixListener, UnixStream};

    /// Short temp dir under /tmp so socket paths stay below SUN_LEN.
    pub fn short_dir() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("oa8b").tempdir_in("/tmp").unwrap()
    }

    #[test]
    fn request_reply_round_trip_over_a_socket_pair() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        send(&mut a, &Request::SetEdit(true)).unwrap();
        let got: Request = recv(&mut b).unwrap().unwrap();
        assert_eq!(got, Request::SetEdit(true));
        send(&mut b, &Reply::Edit(true)).unwrap();
        let back: Reply = recv(&mut a).unwrap().unwrap();
        assert_eq!(back, Reply::Edit(true));
    }

    #[test]
    fn every_request_and_reply_round_trips_through_json() {
        let reqs = vec![
            Request::List,
            Request::SetEnabled { id: "a".into(), enabled: true },
            Request::Reload { id: "a".into() },
            Request::SetEdit(false),
            Request::GetEdit,
        ];
        for r in reqs {
            let j = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Request>(&j).unwrap(), r);
        }
        let st = AppletStatus {
            id: "a".into(),
            name: Some("A".into()),
            description: "d".into(),
            version: "1".into(),
            enabled: true,
            running: false,
            error: Some("boom".into()),
        };
        let reps = vec![
            Reply::Applets(vec![st]),
            Reply::Edit(true),
            Reply::Ok,
            Reply::Error("x".into()),
        ];
        for r in reps {
            let j = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Reply>(&j).unwrap(), r);
        }
    }

    #[test]
    fn request_to_a_missing_socket_is_an_error_not_a_hang() {
        let td = short_dir();
        let err = request_at(&td.path().join("none.sock"), &Request::List).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn the_probe_to_a_silent_listener_times_out_fast() {
        let td = short_dir();
        let path = td.path().join("s.sock");
        let _l = UnixListener::bind(&path).unwrap();
        let start = std::time::Instant::now();
        assert!(request_at(&path, &Request::GetEdit).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn only_the_probe_has_the_short_client_timeout() {
        assert_eq!(client_timeout(&Request::GetEdit), CLIENT_TIMEOUT);
        assert_eq!(client_timeout(&Request::List), REQUEST_TIMEOUT);
        assert_eq!(client_timeout(&Request::SetEdit(true)), REQUEST_TIMEOUT);
        assert_eq!(client_timeout(&Request::Reload { id: "a".into() }), REQUEST_TIMEOUT);
        assert_eq!(REQUEST_TIMEOUT, crate::server::REPLY_TIMEOUT + std::time::Duration::from_secs(2));
    }

    #[test]
    fn request_gets_the_reply_of_a_live_server() {
        let td = short_dir();
        let path = td.path().join("s.sock");
        let l = UnixListener::bind(&path).unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let r: Request = recv(&mut s).unwrap().unwrap();
            assert_eq!(r, Request::GetEdit);
            send(&mut s, &Reply::Edit(false)).unwrap();
        });
        assert_eq!(request_at(&path, &Request::GetEdit).unwrap(), Reply::Edit(false));
        h.join().unwrap();
    }
}
