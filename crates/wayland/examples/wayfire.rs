//! Print the windows and desktops Wayfire's IPC reports, then (with
//! `move <id> <desktop>`) move a window to another desktop. A manual test
//! tool, run under Wayfire (it needs `WAYFIRE_SOCKET`):
//! `cargo run -p opusdm-wayland --example wayfire -- move 3 1`.

use opusdm_wayland::wayfire;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [cmd, id, desktop] = args.as_slice() {
        if cmd != "move" { usage(); }
        let (Ok(id), Ok(desktop)) = (id.parse(), desktop.parse()) else { usage() };
        if let Err(e) = wayfire::move_to_desktop(id, desktop) {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
    let state = wayfire::State::read().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    println!("desktops: {}, current: {}", state.desktop_count(), state.current_desktop());
    for v in &state.views {
        let r = state.rect_of(v);
        println!("{} {:?} desktop {:?} at {},{} {}x{} pid {:?}", v.id, v.title, state.desktop_of(v), r.x, r.y, r.w, r.h, v.pid);
    }
}

/// Print the usage and exit.
fn usage() -> ! {
    eprintln!("usage: wayfire [move <window id> <desktop index>]");
    std::process::exit(2);
}
