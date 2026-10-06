//! Prints the volumes the notifier would list, one per line.

use opusdm_devices::{Backend, UdisksBackend};

fn main() {
    let backend = UdisksBackend::connect().expect("system bus");
    for d in backend.list().expect("udisks2") {
        println!(
            "{} | {} | {} | {:?} | eject={} off={}",
            d.dev_node, d.label, opusdm_devices::human_size(d.size), d.mount_point, d.can_eject, d.can_power_off
        );
    }
}
