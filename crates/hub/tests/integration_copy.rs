//! End-to-end: start the hub server layer, connect two fake Listers over the
//! real socket, drive a Copy SOURCE -> DEST, assert the file lands.
//!
//! Ignored by default (spawns threads and touches the filesystem); run with:
//!   cargo test -p opusdm-hub --test integration_copy -- --ignored

use opusdm_hub::server::spawn_server;
use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::{Op, ToHub, ToLister};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

fn reg(s: &mut UnixStream, pid: u32, path: &str) -> opusdm_ipc::ListerId {
    send(s, &ToHub::Register { pid, initial_path: path.into() }).unwrap();
    match recv::<_, ToLister>(s).unwrap().unwrap() {
        ToLister::Welcome { lister_id, .. } => lister_id,
        other => panic!("{other:?}"),
    }
}

#[test]
#[ignore]
fn copy_source_to_dest_lands_file() {
    let td = tempfile::tempdir().unwrap();
    let sock = td.path().join("hub.sock");
    let src_dir = td.path().join("srcpane");
    let dst_dir = td.path().join("dstpane");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dst_dir).unwrap();
    std::fs::write(src_dir.join("payload.txt"), b"hello opusdm").unwrap();

    let listener = UnixListener::bind(&sock).unwrap();
    let _handle = spawn_server(listener, Box::new(|_p| {}));

    let mut a = UnixStream::connect(&sock).unwrap();
    a.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut b = UnixStream::connect(&sock).unwrap();
    b.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    let _ia = reg(&mut a, 1, src_dir.to_str().unwrap());
    let _ib = reg(&mut b, 2, dst_dir.to_str().unwrap());

    // We want A = SOURCE, B = DEST: focus B, then focus A.
    send(&mut b, &ToHub::FocusGained).unwrap();
    let _ = recv::<_, ToLister>(&mut b).unwrap();
    let _ = recv::<_, ToLister>(&mut a).unwrap();
    send(&mut a, &ToHub::FocusGained).unwrap();
    let _ = recv::<_, ToLister>(&mut a).unwrap();
    let _ = recv::<_, ToLister>(&mut b).unwrap();

    // dest_dir: None exercises the hub resolving it from the DEST Lister (B).
    send(
        &mut a,
        &ToHub::RunJob {
            op: Op::Copy,
            srcs: vec![src_dir.join("payload.txt")],
            dest_dir: None,
        },
    )
    .unwrap();

    loop {
        match recv::<_, ToLister>(&mut a).unwrap() {
            Some(ToLister::JobDone { outcome, errors, .. }) => {
                assert!(errors.is_empty(), "{errors:?}");
                assert!(matches!(outcome, opusdm_ipc::Outcome::Ok { .. }), "{outcome:?}");
                break;
            }
            Some(_) => continue,
            None => panic!("hub closed before JobDone"),
        }
    }

    let landed = dst_dir.join("payload.txt");
    assert_eq!(std::fs::read(&landed).unwrap(), b"hello opusdm");
}
