//! Talks to the real sound server. Run with `cargo test -p opusdm-audio -- --ignored`.

use opusdm_audio::pulse::Session;
use opusdm_audio::Kind;

#[test]
#[ignore = "needs a running PulseAudio or pipewire-pulse"]
fn lists_outputs_and_no_monitors() {
    let mut session = Session::connect().expect("connect");
    let list = session.list().expect("list");
    assert!(list.iter().any(|d| d.kind == Kind::Output), "no output device: {list:?}");
    assert!(list.iter().all(|d| !d.name.ends_with(".monitor")), "a monitor leaked: {list:?}");
    for kind in [Kind::Output, Kind::Input] {
        let defaults = list.iter().filter(|d| d.kind == kind && d.is_default).count();
        assert!(defaults <= 1, "{kind:?} has {defaults} defaults");
    }
}

#[test]
#[ignore = "needs a running PulseAudio or pipewire-pulse"]
fn a_call_on_a_vanished_device_is_no_such_device() {
    use opusdm_audio::{AudioError, Backend};
    // The session is not `Send`: the owner thread creates it and hands out the backend.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut session = Session::connect().expect("connect");
        tx.send(session.backend()).unwrap();
        // Jobs run while `follow` runs; the thread ends with the test process.
        let _ = session.follow(Vec::new(), |_| true);
    });
    let backend = rx.recv().expect("backend");
    assert_eq!(backend.set_mute(Kind::Output, 999_999, false), Err(AudioError::NoSuchDevice));
    assert_eq!(backend.set_volume(Kind::Input, 999_999, 10), Err(AudioError::NoSuchDevice));
}
