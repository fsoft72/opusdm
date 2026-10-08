//! Prints what the real `xrandr` reports, parsed. Read only.

use opusdm_screen::backend::{ScreenBackend, XrandrBackend};

fn main() {
    match XrandrBackend::new().query() {
        Ok(parsed) => {
            for m in &parsed.monitors {
                println!("{} id={} preferred={:?} modes={}", m.name, m.edid_id, m.preferred, m.modes.len());
            }
            println!("{:#?}", parsed.layout);
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
