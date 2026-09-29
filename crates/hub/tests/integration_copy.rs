//! End-to-end: start the hub server layer, connect two fake Listers over the
//! real socket, drive a Copy SOURCE -> DEST, assert the file lands.
//!
//! Ignored by default (spawns threads and touches the filesystem); run with:
//!   cargo test -p opusdm-hub --test integration_copy -- --ignored

use opusdm_hub::server::spawn_server;
use opusdm_ipc::frame::{recv, send};
use opusdm_ipc::{Op, ToHub, ToLister, Welcome};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

fn reg(s: &mut UnixStream, pid: u32, path: &str) -> opusdm_ipc::ListerId {
    send(s, &ToHub::Register { pid, initial_path: path.into() }).unwrap();
    recv::<_, Welcome>(s).unwrap().unwrap().lister_id
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
            options: opusdm_ipc::JobOptions::default(),
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

#[test]
#[ignore]
fn copy_fans_out_to_all_locked_dests() {
    let td = tempfile::tempdir().unwrap();
    let sock = td.path().join("hub.sock");
    let src = td.path().join("src");
    let d1 = td.path().join("d1");
    let d2 = td.path().join("d2");
    for p in [&src, &d1, &d2] {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(src.join("f.txt"), b"fan out").unwrap();

    let listener = UnixListener::bind(&sock).unwrap();
    let _handle = spawn_server(listener, Box::new(|_p| {}));

    let mut a = UnixStream::connect(&sock).unwrap();
    a.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut b = UnixStream::connect(&sock).unwrap();
    b.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut c = UnixStream::connect(&sock).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    reg(&mut a, 1, src.to_str().unwrap());
    reg(&mut b, 2, d1.to_str().unwrap());
    reg(&mut c, 3, d2.to_str().unwrap());

    // a = SOURCE.
    send(&mut a, &ToHub::FocusGained).unwrap();
    // b and c = locked DESTs.
    send(&mut b, &ToHub::SetRole(opusdm_ipc::Role::Dest)).unwrap();
    send(&mut b, &ToHub::LockRole).unwrap();
    send(&mut c, &ToHub::SetRole(opusdm_ipc::Role::Dest)).unwrap();
    send(&mut c, &ToHub::LockRole).unwrap();

    // Wait until A has observed both DESTs before issuing the job.
    loop {
        match recv::<_, ToLister>(&mut a).unwrap() {
            Some(ToLister::RoleChanged { dests, .. }) if dests.len() == 2 => break,
            Some(_) => {}
            None => panic!("hub closed while waiting for roles"),
        }
    }

    send(
        &mut a,
        &ToHub::RunJob {
            op: Op::Copy,
            srcs: vec![src.join("f.txt")],
            dest_dir: None,
            options: opusdm_ipc::JobOptions::default(),
        },
    )
    .unwrap();

    // Expect two JobDone messages on A (one per destination).
    let mut done = 0;
    while done < 2 {
        match recv::<_, ToLister>(&mut a).unwrap() {
            Some(ToLister::JobDone { outcome, errors, .. }) => {
                assert!(errors.is_empty(), "{errors:?}");
                assert!(matches!(outcome, opusdm_ipc::Outcome::Ok { .. }));
                done += 1;
            }
            Some(_) => {}
            None => panic!("hub closed early"),
        }
    }

    assert_eq!(std::fs::read(d1.join("f.txt")).unwrap(), b"fan out");
    assert_eq!(std::fs::read(d2.join("f.txt")).unwrap(), b"fan out");
}

#[test]
#[ignore]
fn check_fit_answers_with_sizes_and_free_space() {
    let td = tempfile::tempdir().unwrap();
    let sock = td.path().join("hub.sock");
    std::fs::write(td.path().join("f.bin"), vec![0u8; 300]).unwrap();

    let listener = UnixListener::bind(&sock).unwrap();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = UnixStream::connect(&sock).unwrap();
    a.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    reg(&mut a, 1, td.path().to_str().unwrap());

    send(
        &mut a,
        &ToHub::CheckFit { srcs: vec![td.path().join("f.bin")], dest_dir: Some(td.path().to_path_buf()) },
    )
    .unwrap();
    loop {
        match recv::<_, ToLister>(&mut a).unwrap() {
            Some(ToLister::CheckFitResult { needed, free, fits, percent, .. }) => {
                assert_eq!(needed, 300);
                assert!(free > 0 && fits && percent == 100);
                break;
            }
            Some(ToLister::CheckFitFailed { message }) => panic!("{message}"),
            Some(_) => continue,
            None => panic!("hub closed before the answer"),
        }
    }
}

#[test]
#[ignore]
fn duplicate_job_with_options_creates_a_copy_next_to_the_source() {
    let td = tempfile::tempdir().unwrap();
    let sock = td.path().join("hub.sock");
    std::fs::write(td.path().join("f.txt"), b"dup").unwrap();

    let listener = UnixListener::bind(&sock).unwrap();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = UnixStream::connect(&sock).unwrap();
    a.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    reg(&mut a, 1, td.path().to_str().unwrap());

    send(
        &mut a,
        &ToHub::RunJob {
            op: Op::Duplicate,
            srcs: vec![td.path().join("f.txt")],
            dest_dir: Some(td.path().to_path_buf()),
            options: opusdm_ipc::JobOptions::default(),
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
    assert_eq!(std::fs::read(td.path().join("f.txt.copy")).unwrap(), b"dup");
}
