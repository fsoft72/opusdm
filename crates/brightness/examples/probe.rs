//! Manual check against the real system: prints the current brightness, sets
//! 60 and restores the old value. With the argument `xrandr` it forces the
//! software backend instead of the detected one.

use opusdm_brightness::backend::Xrandr;
use opusdm_brightness::{Backend, Detected};

fn main() {
    let found = if std::env::args().nth(1).as_deref() == Some("xrandr") {
        let backend = Xrandr::find().unwrap_or_else(|e| panic!("{e}"));
        let percent = backend.get().unwrap_or_else(|e| panic!("{e}"));
        Detected { backend: Box::new(backend), percent }
    } else {
        opusdm_brightness::detect().unwrap_or_else(|e| panic!("{e}"))
    };
    println!("method {} at {}%", found.backend.name(), found.percent);
    found.backend.set(60).expect("set 60");
    println!("now {}%", found.backend.get().expect("get"));
    found.backend.set(found.percent).expect("restore");
    println!("restored {}%", found.backend.get().expect("get"));
}
