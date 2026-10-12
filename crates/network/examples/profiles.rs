//! Lists the connection profiles read from NetworkManager and loads each one
//! (secrets are never printed, only whether they were found).
//!
//! With `--roundtrip` it also adds a WireGuard profile, loads it back,
//! compares it and deletes it: this changes NetworkManager and the keyring
//! for a moment.

use opusdm_network::ipconfig::IpAddress;
use opusdm_network::profile::{Detail, Profile, ProfileKind, Secret};
use opusdm_network::{NmBackend, ProfileBackend};

/// The public key of the peer of the round trip profile.
const PEER_KEY: &str = "HIgo9xNzJMWLKASShiTqIybxZ0U3wGLiUeJ1PKf8ykw=";

fn main() {
    let backend = NmBackend::connect().expect("system bus");
    let list = match backend.list() {
        Ok(list) => list,
        Err(e) => return eprintln!("list: {e}"),
    };
    for summary in &list {
        let loaded = backend.load(&summary.uuid).map(|l| l.profile.secrets().len());
        let secrets = match loaded {
            Ok(n) => format!("{n} secrets found"),
            Err(e) => format!("load failed: {e}"),
        };
        println!("{:<30} {:<18} active={} ask={} {}", summary.name, summary.type_name, summary.active, summary.ask_password, secrets);
    }
    if std::env::args().any(|a| a == "--roundtrip") {
        round_trip(&backend);
    }
}

/// Add, load, compare and delete a WireGuard profile.
fn round_trip(backend: &NmBackend) {
    let mut p = Profile::new(ProfileKind::WireGuard).expect("new profile");
    p.id = "opusdm-roundtrip".into();
    p.interface_name = "opusdm-rt".into();
    p.ipv4.addresses = vec![IpAddress { address: "10.99.0.2".into(), prefix: 32 }];
    p.ipv4.dns = vec!["10.99.0.1".into()];
    if let Detail::WireGuard(w) = &mut p.detail {
        w.peers[0].public_key = PEER_KEY.into();
        w.peers[0].endpoint = "192.0.2.1:51820".into();
        w.peers[0].preshared_key = Secret::new(PEER_KEY);
    }
    backend.save(&p, None).expect("add");
    let loaded = backend.load(&p.uuid).expect("load");
    println!("round trip equal: {}", loaded.profile == p);
    if loaded.profile != p {
        println!("wrote {p:#?}\nread {:#?}", loaded.profile);
    }
    let mut changed = loaded.profile.clone();
    changed.id = "opusdm-roundtrip-2".into();
    backend.save(&changed, Some(&loaded.base)).expect("update");
    println!("update read back: {}", backend.load(&p.uuid).expect("load").profile == changed);
    backend.delete(&p.uuid).expect("delete");
    println!("deleted: {}", backend.load(&p.uuid).is_err());
}
