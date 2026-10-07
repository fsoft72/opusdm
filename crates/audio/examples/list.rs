//! Prints the devices the audio tray would list, one per line.

use opusdm_audio::pulse::Session;

fn main() {
    let mut session = Session::connect().expect("sound server");
    for d in session.list().expect("device list") {
        println!(
            "{:?} #{} | {} | {}% muted={} default={} | {}",
            d.kind, d.index, d.description, d.volume_percent, d.muted, d.is_default, d.name
        );
    }
}
