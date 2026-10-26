//! The accept loop of `opusdm-applets` and the placeholder answers.

use crate::protocol::{Reply, Request};
use opusdm_ipc::frame::{recv, send};
use std::os::unix::net::{UnixListener, UnixStream};

/// Read timeout of an accepted stream, so a stuck client cannot pin the thread.
pub const CONN_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Answer one request. Placeholder until the real host is wired in.
pub fn answer(req: &Request) -> Reply {
    match req {
        Request::List => Reply::Applets(vec![]),
        Request::GetEdit => Reply::Edit(false),
        _ => Reply::Error("not ready".into()),
    }
}

/// Serve one connection: answer every request until a clean EOF.
pub fn handle_conn(stream: UnixStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(CONN_READ_TIMEOUT))?;
    stream.set_write_timeout(Some(CONN_READ_TIMEOUT))?;
    let mut stream = stream;
    while let Some(req) = recv::<_, Request>(&mut stream)? {
        send(&mut stream, &answer(&req))?;
    }
    Ok(())
}

/// Start the accept thread, which owns `listener` for the whole run.
pub fn spawn_accept(listener: UnixListener) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("opusdm-applets-accept".into()).spawn(move || {
        // One connection at a time for now; the real host (Task 8c) may serve
        // them concurrently.
        for conn in listener.incoming() {
            let stream = match conn {
                Ok(s) => s,
                Err(e) => {
                    opusdm_ipc::err_log!("applets", "accept failed, exiting: {e}");
                    std::process::exit(1);
                }
            };
            if let Err(e) = handle_conn(stream) {
                opusdm_ipc::err_log!("applets", "connection dropped: {e}");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_the_placeholders() {
        assert_eq!(answer(&Request::List), Reply::Applets(vec![]));
        assert_eq!(answer(&Request::GetEdit), Reply::Edit(false));
        assert_eq!(answer(&Request::SetEdit(true)), Reply::Error("not ready".into()));
        assert_eq!(answer(&Request::Reload { id: "a".into() }), Reply::Error("not ready".into()));
    }

    #[test]
    fn a_connection_gets_one_reply_per_request_until_eof() {
        let (mut c, s) = UnixStream::pair().unwrap();
        let h = std::thread::spawn(move || handle_conn(s));
        send(&mut c, &Request::GetEdit).unwrap();
        assert_eq!(recv::<_, Reply>(&mut c).unwrap().unwrap(), Reply::Edit(false));
        send(&mut c, &Request::List).unwrap();
        assert_eq!(recv::<_, Reply>(&mut c).unwrap().unwrap(), Reply::Applets(vec![]));
        drop(c);
        h.join().unwrap().unwrap();
    }

    #[test]
    fn a_bad_frame_drops_only_that_connection() {
        use std::io::Write;
        let td = tempfile::Builder::new().prefix("oa8b").tempdir_in("/tmp").unwrap();
        let path = td.path().join("s.sock");
        let l = UnixListener::bind(&path).unwrap();
        let _h = spawn_accept(l).unwrap();
        let mut bad = UnixStream::connect(&path).unwrap();
        bad.write_all(b"\x00\x00\x00\x03abc").unwrap();
        drop(bad);
        let r = crate::protocol::request_at(&path, &Request::GetEdit).unwrap();
        assert_eq!(r, Reply::Edit(false));
    }
}
