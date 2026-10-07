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
