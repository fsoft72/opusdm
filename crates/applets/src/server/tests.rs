use super::*;
use crate::protocol::request_at;
use std::io::Write;
use std::time::Instant;

/// Short temp dir under /tmp so socket paths stay below SUN_LEN.
fn short_dir() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("oa8c").tempdir_in("/tmp").unwrap()
}

/// Shared state over a fresh flag; the receiver plays the host.
fn shared() -> (Shared, Arc<AtomicBool>, async_channel::Receiver<HostMessage>) {
    let edit = Arc::new(AtomicBool::new(false));
    let (tx, rx) = async_channel::unbounded();
    (Shared::new(edit.clone(), tx), edit, rx)
}

/// A fake host thread that answers every request with `reply` and counts the
/// edit notifications into the returned channel.
fn fake_host(rx: async_channel::Receiver<HostMessage>, reply: Reply) -> std::sync::mpsc::Receiver<bool> {
    let (edits_tx, edits_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(msg) = rx.recv_blocking() {
            match msg {
                HostMessage::Request(_, back) => back.send(reply.clone()).unwrap(),
                HostMessage::Edit(on) => edits_tx.send(on).unwrap(),
            }
        }
    });
    edits_rx
}

#[test]
fn get_edit_is_answered_from_the_flag_without_the_host() {
    let (shared, edit, _rx) = shared();
    assert_eq!(answer(&Request::GetEdit, &shared), Reply::Edit(false));
    edit.store(true, Ordering::SeqCst);
    assert_eq!(answer(&Request::GetEdit, &shared), Reply::Edit(true));
}

#[test]
fn set_edit_stores_the_flag_and_notifies_the_host() {
    let (shared, edit, rx) = shared();
    let edits = fake_host(rx, Reply::Ok);
    assert_eq!(answer(&Request::SetEdit(true), &shared), Reply::Edit(true));
    assert!(edit.load(Ordering::SeqCst));
    assert!(edits.recv_timeout(Duration::from_secs(1)).unwrap());
    assert_eq!(answer(&Request::GetEdit, &shared), Reply::Edit(true));
}

#[test]
fn other_requests_go_to_the_host_and_its_reply_comes_back() {
    let (shared, _edit, rx) = shared();
    let _edits = fake_host(rx, Reply::Applets(vec![]));
    assert_eq!(answer(&Request::List, &shared), Reply::Applets(vec![]));
    assert_eq!(answer(&Request::Reload { id: "a".into() }, &shared), Reply::Applets(vec![]));
}

#[test]
fn a_host_that_does_not_answer_gives_busy_after_the_timeout() {
    let (shared, _edit, _rx) = shared();
    let shared = shared.with_reply_timeout(Duration::from_millis(200));
    let start = Instant::now();
    assert_eq!(answer(&Request::List, &shared), Reply::Error(BUSY_MESSAGE.into()));
    assert!(start.elapsed() >= Duration::from_millis(200));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_closed_host_channel_gives_an_error_not_a_hang() {
    let (shared, _edit, rx) = shared();
    drop(rx);
    let start = Instant::now();
    assert_eq!(answer(&Request::List, &shared), Reply::Error(HOST_GONE_MESSAGE.into()));
    assert!(start.elapsed() < Duration::from_secs(1));
    // The edit flag still works without the host.
    assert_eq!(answer(&Request::SetEdit(true), &shared), Reply::Edit(true));
    assert_eq!(answer(&Request::GetEdit, &shared), Reply::Edit(true));
}

#[test]
fn a_host_that_drops_the_request_gives_an_error() {
    let (shared, _edit, rx) = shared();
    std::thread::spawn(move || {
        while let Ok(msg) = rx.recv_blocking() {
            drop(msg);
        }
    });
    assert_eq!(answer(&Request::List, &shared), Reply::Error(HOST_GONE_MESSAGE.into()));
}

#[test]
fn a_connection_gets_one_reply_per_request_until_eof() {
    let (shared, _edit, rx) = shared();
    let _edits = fake_host(rx, Reply::Applets(vec![]));
    let (mut c, s) = UnixStream::pair().unwrap();
    let h = std::thread::spawn(move || handle_conn(s, &shared));
    send(&mut c, &Request::GetEdit).unwrap();
    assert_eq!(recv::<_, Reply>(&mut c).unwrap().unwrap(), Reply::Edit(false));
    send(&mut c, &Request::List).unwrap();
    assert_eq!(recv::<_, Reply>(&mut c).unwrap().unwrap(), Reply::Applets(vec![]));
    drop(c);
    h.join().unwrap().unwrap();
}

#[test]
fn a_bad_frame_drops_only_that_connection() {
    let (shared, _edit, _rx) = shared();
    let td = short_dir();
    let path = td.path().join("s.sock");
    let l = UnixListener::bind(&path).unwrap();
    let _h = spawn_accept(l, shared).unwrap();
    let mut bad = UnixStream::connect(&path).unwrap();
    bad.write_all(b"\x00\x00\x00\x03abc").unwrap();
    drop(bad);
    let r = request_at(&path, &Request::GetEdit).unwrap();
    assert_eq!(r, Reply::Edit(false));
}

/// Decision 2 of Task 8c: a client that holds a connection open and sends
/// nothing does not delay the probe of another connection.
#[test]
fn a_held_connection_does_not_block_a_probe() {
    let (shared, _edit, _rx) = shared();
    let td = short_dir();
    let path = td.path().join("s.sock");
    let l = UnixListener::bind(&path).unwrap();
    let _h = spawn_accept(l, shared).unwrap();
    let _held = UnixStream::connect(&path).unwrap();
    // Let the accept thread take the held connection first.
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    assert_eq!(request_at(&path, &Request::GetEdit).unwrap(), Reply::Edit(false));
    assert!(start.elapsed() < Duration::from_secs(1), "probe took {:?}", start.elapsed());
}

/// A request waiting for a busy host does not delay the probe either.
#[test]
fn a_request_waiting_for_the_host_does_not_block_a_probe() {
    let (shared, _edit, _rx) = shared();
    let td = short_dir();
    let path = td.path().join("s.sock");
    let l = UnixListener::bind(&path).unwrap();
    let _h = spawn_accept(l, shared).unwrap();
    let mut waiting = UnixStream::connect(&path).unwrap();
    send(&mut waiting, &Request::List).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    assert_eq!(request_at(&path, &Request::GetEdit).unwrap(), Reply::Edit(false));
    assert!(start.elapsed() < Duration::from_secs(1), "probe took {:?}", start.elapsed());
}
