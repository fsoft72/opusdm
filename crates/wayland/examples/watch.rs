//! Print every snapshot of the compositor's windows and workspaces, then
//! (with `switch`) step to the next workspace once. A manual test tool:
//! `WAYLAND_DISPLAY=wayland-1 cargo run -p opusdm-wayland --example watch`.

fn main() {
    let Some(session) = opusdm_wayland::shared() else {
        eprintln!("not a Wayland session, or the compositor cannot be reached");
        std::process::exit(1);
    };
    println!("toplevels: {}, workspaces: {}", session.has_toplevels(), session.has_workspaces());
    session.subscribe(|snapshot| println!("{snapshot:#?}"));
    if std::env::args().any(|a| a == "switch") {
        std::thread::sleep(std::time::Duration::from_millis(300));
        session.switch_workspace(1);
    }
    std::thread::sleep(std::time::Duration::from_secs(3));
}
