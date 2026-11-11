//! Joins a Wi-Fi network with the tray backend and prints every step and
//! the outcome, with the seconds since the start. CHANGES THE NETWORK.
//!
//! `cargo run -p opusdm-network --example connect -- <ssid> [password]`

use opusdm_network::{Backend, NmBackend};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(ssid) = args.next() else {
        eprintln!("usage: connect <ssid> [password]");
        std::process::exit(2);
    };
    let password = args.next();
    let backend = NmBackend::connect().expect("system bus");
    let start = Instant::now();
    let progress = |step: opusdm_network::ConnectStep| println!("{:6.2} {}", start.elapsed().as_secs_f32(), step.text());
    match backend.connect(&ssid, password.as_deref(), &progress) {
        Ok(()) => println!("{:6.2} done", start.elapsed().as_secs_f32()),
        Err(e) => println!("{:6.2} failed: {} (needs password: {}, refused: {})", start.elapsed().as_secs_f32(), e.text(), e.needs_password, e.password_refused),
    }
}
