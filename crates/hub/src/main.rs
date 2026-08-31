//! opusdm-hub: the always-on controller. Owns the socket, the Lister
//! registry, the global roles, and the job engine.

mod jobs;
mod roles;
mod server;

fn main() {
    eprintln!("opusdm-hub: not wired up yet (see plan Tasks 7-10)");
    std::process::exit(1);
}
