//! Print the power state read from UPower and power-profiles-daemon.
//! Run with `cargo run -p opusdm-power --example power_state`.

use opusdm_power::{Backend, UPowerBackend};

fn main() {
    let backend = UPowerBackend::connect().unwrap_or_else(|e| panic!("system bus: {e}"));
    match backend.state() {
        Ok(state) => println!("{state:#?}"),
        Err(e) => eprintln!("error: {e}"),
    }
}
