//! Prints what the real `xrandr` (X11) or `wlr-randr` (Wayland) reports,
//! parsed. Read only.

use opusdm_screen::backend::for_session;

fn main() {
    match for_session().query() {
        Ok(parsed) => {
            for m in &parsed.monitors {
                println!("{} id={} preferred={:?} modes={}", m.name, m.edid_id, m.preferred, m.modes.len());
            }
            println!("{:#?}", parsed.layout);
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
