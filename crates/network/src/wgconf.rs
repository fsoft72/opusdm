//! Import of a WireGuard configuration file, the `wg-quick` format.

use crate::error::NetError;
use crate::import::Imported;
use crate::ipconfig::{Family, IpAddress, IpMethod};
use crate::profile::{Detail, Peer, Profile, ProfileKind, Secret};

/// The longest Linux interface name.
const IFNAME_MAX: usize = 15;

/// Which section a line belongs to.
enum Section {
    None,
    Interface,
    Peer,
}

/// Read `text`, a configuration named `name` (the file name without its
/// extension, which `wg-quick` also uses as the interface name).
pub fn parse(text: &str, name: &str) -> Result<Imported, NetError> {
    let mut profile = Profile::new(ProfileKind::WireGuard)?;
    profile.id = name.to_string();
    profile.interface_name = interface_name(name);
    profile.ipv4.method = IpMethod::Disabled;
    let Detail::WireGuard(wg) = &mut profile.detail else { unreachable!("a new WireGuard profile has WireGuard details") };
    wg.peers.clear();
    wg.private_key = Secret::default();
    let mut ignored = Vec::new();
    let mut section = Section::None;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() { continue; }
        let err = |what: &str| NetError::other(format!("Line {}: {what}", n + 1));
        if line.eq_ignore_ascii_case("[interface]") {
            section = Section::Interface;
            continue;
        }
        if line.eq_ignore_ascii_case("[peer]") {
            wg.peers.push(Peer::default());
            section = Section::Peer;
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| err("expected Key = Value"))?;
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        let list = || value.split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect::<Vec<_>>();
        let number = || value.parse::<u32>().map_err(|_| err(&format!("{value} is not a number")));
        match (&section, key.as_str()) {
            (Section::None, _) => return Err(err("a setting before [Interface] or [Peer]")),
            (Section::Interface, "privatekey") => wg.private_key = Secret::new(value),
            (Section::Interface, "listenport") => wg.listen_port = number()?,
            (Section::Interface, "mtu") => wg.mtu = number()?,
            (Section::Interface, "fwmark") => wg.fwmark = fwmark(value).ok_or_else(|| err(&format!("{value} is not a firewall mark")))?,
            (Section::Interface, "address") => {
                for a in list() {
                    let ip = address(&a).ok_or_else(|| err(&format!("{a} is not an address")))?;
                    let config = if ip.0 == Family::V4 { &mut profile.ipv4 } else { &mut profile.ipv6 };
                    config.method = IpMethod::Manual;
                    config.addresses.push(ip.1);
                }
            }
            (Section::Interface, "dns") => {
                for d in list() {
                    if Family::V4.parses(&d) {
                        profile.ipv4.dns.push(d);
                        profile.ipv4.ignore_auto_dns = true;
                    } else if Family::V6.parses(&d) {
                        profile.ipv6.dns.push(d);
                        profile.ipv6.ignore_auto_dns = true;
                    } else {
                        ignored.push(format!("DNS search domain {d}"));
                    }
                }
            }
            (Section::Peer, k) => {
                let peer = wg.peers.last_mut().ok_or_else(|| err("a peer setting outside [Peer]"))?;
                match k {
                    "publickey" => peer.public_key = value.to_string(),
                    "presharedkey" => peer.preshared_key = Secret::new(value),
                    "allowedips" => peer.allowed_ips.extend(list()),
                    "endpoint" => peer.endpoint = value.to_string(),
                    "persistentkeepalive" => peer.keepalive = if value == "off" { 0 } else { number()? },
                    _ => ignored.push(raw.trim().to_string()),
                }
            }
            (Section::Interface, _) => ignored.push(key_only(raw)),
        }
    }
    if wg.private_key.is_empty() { return Err(NetError::other("The file has no PrivateKey")); }
    if wg.peers.is_empty() { return Err(NetError::other("The file has no [Peer]")); }
    Ok(Imported { profile, ignored, files: Vec::new() })
}

/// An `Address` entry: `10.0.0.2/24`; no prefix means a single host.
fn address(text: &str) -> Option<(Family, IpAddress)> {
    let (addr, prefix) = text.split_once('/').map(|(a, p)| (a, Some(p))).unwrap_or((text, None));
    let family = if Family::V4.parses(addr) { Family::V4 } else if Family::V6.parses(addr) { Family::V6 } else { return None };
    let prefix = match prefix {
        Some(p) => p.parse().ok().filter(|p| *p <= family.max_prefix())?,
        None => family.max_prefix(),
    };
    Some((family, IpAddress { address: addr.to_string(), prefix }))
}

/// A `FwMark`: a number, hexadecimal with `0x`, or `off`.
fn fwmark(text: &str) -> Option<u32> {
    if text == "off" { return Some(0); }
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// An interface name from the file name: letters, digits, `-` and `_`, up to
/// 15 characters; `wg0` when nothing is left.
fn interface_name(name: &str) -> String {
    let clean: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(IFNAME_MAX).collect();
    if clean.is_empty() { "wg0".to_string() } else { clean }
}

/// The key of an ignored line, so a value like `PostUp` commands is not shown.
fn key_only(raw: &str) -> String {
    raw.split('=').next().unwrap_or(raw).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# home tunnel
[Interface]
PrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=
Address = 10.8.0.2/24, fd00::2/64
DNS = 1.1.1.1, fd00::53, home.lan
ListenPort = 51820
FwMark = 0x10
PostUp = iptables -A FORWARD -j ACCEPT

[Peer]
PublicKey = HIgo9xNzJMWLKASShiTqIybxZ0U3wGLiUeJ1PKf8ykw=
PresharedKey = HIgo9xNzJMWLKASShiTqIybxZ0U3wGLiUeJ1PKf8ykw=
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = vpn.example.com:51820
PersistentKeepalive = 25
";

    #[test]
    fn a_full_file_is_read() {
        let imported = parse(SAMPLE, "home-tunnel").unwrap();
        let p = &imported.profile;
        assert_eq!(p.id, "home-tunnel");
        assert_eq!(p.interface_name, "home-tunnel");
        assert_eq!(p.ipv4.method, IpMethod::Manual);
        assert_eq!(p.ipv4.addresses, vec![IpAddress { address: "10.8.0.2".into(), prefix: 24 }]);
        assert_eq!(p.ipv6.addresses, vec![IpAddress { address: "fd00::2".into(), prefix: 64 }]);
        assert_eq!(p.ipv4.dns, vec!["1.1.1.1".to_string()]);
        assert_eq!(p.ipv6.dns, vec!["fd00::53".to_string()]);
        let Detail::WireGuard(wg) = &p.detail else { panic!("not WireGuard") };
        assert_eq!(wg.listen_port, 51820);
        assert_eq!(wg.fwmark, 16);
        assert_eq!(wg.peers.len(), 1);
        assert_eq!(wg.peers[0].endpoint, "vpn.example.com:51820");
        assert_eq!(wg.peers[0].allowed_ips, vec!["0.0.0.0/0".to_string(), "::/0".to_string()]);
        assert_eq!(wg.peers[0].keepalive, 25);
        assert!(!wg.peers[0].preshared_key.is_empty());
        assert_eq!(imported.ignored, vec!["DNS search domain home.lan".to_string(), "PostUp".to_string()]);
        assert!(p.validate().is_empty(), "{:?}", p.validate());
    }

    #[test]
    fn a_file_without_ipv4_addresses_disables_ipv4() {
        let text = SAMPLE.replace("10.8.0.2/24, ", "");
        let p = parse(&text, "x").unwrap().profile;
        assert_eq!(p.ipv4.method, IpMethod::Disabled);
    }

    #[test]
    fn broken_files_say_why() {
        assert!(parse("PrivateKey = x", "x").unwrap_err().text().starts_with("Line 1"));
        assert!(parse("[Interface]\nAddress = nope\n", "x").unwrap_err().text().contains("nope"));
        assert!(parse("[Interface]\nListenPort = big\n", "x").unwrap_err().text().contains("Line 2"));
        assert!(parse("[Interface]\nAddress = 10.0.0.1/24\n[Peer]\nPublicKey = k\n", "x").unwrap_err().text().contains("PrivateKey"));
        let no_peer = SAMPLE.split("[Peer]").next().unwrap();
        assert!(parse(no_peer, "x").unwrap_err().text().contains("[Peer]"));
    }

    #[test]
    fn interface_names_are_cleaned() {
        assert_eq!(interface_name("my vpn (work)"), "myvpnwork");
        assert_eq!(interface_name("a-very-long-interface-name"), "a-very-long-int");
        assert_eq!(interface_name("()"), "wg0");
    }
}
