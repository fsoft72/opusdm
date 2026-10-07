//! Prints the network state read from NetworkManager (read only).

use opusdm_network::{Backend, NmBackend};

fn main() {
    let backend = NmBackend::connect().expect("system bus");
    match backend.state() {
        Ok(state) => println!("{state:#?}"),
        Err(e) => eprintln!("error: {e}"),
    }
}
