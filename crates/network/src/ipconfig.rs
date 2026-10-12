//! The `ipv4` and `ipv6` settings of a profile.

use crate::settings::{self, as_text, as_uint, Settings};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use zbus::zvariant::Value;

/// Which IP family a configuration is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    /// The settings group of the family.
    pub fn group(self) -> &'static str {
        match self {
            Family::V4 => "ipv4",
            Family::V6 => "ipv6",
        }
    }

    /// The largest prefix length.
    pub fn max_prefix(self) -> u32 {
        match self {
            Family::V4 => 32,
            Family::V6 => 128,
        }
    }

    /// Whether `text` is an address of this family.
    pub fn parses(self, text: &str) -> bool {
        match self {
            Family::V4 => text.parse::<Ipv4Addr>().is_ok(),
            Family::V6 => text.parse::<Ipv6Addr>().is_ok(),
        }
    }
}

/// How the addresses are obtained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpMethod {
    /// DHCP (IPv4), or router advertisements plus DHCP (IPv6).
    Auto,
    Manual,
    LinkLocal,
    /// Share the connection with other computers.
    Shared,
    /// No address of this family (`disabled`, or `ignore` for IPv6).
    Disabled,
    /// A method this program does not edit; written back unchanged.
    Other(String),
}

impl IpMethod {
    /// Read the NetworkManager name.
    pub fn from_name(name: &str) -> Self {
        match name {
            "auto" => IpMethod::Auto,
            "manual" => IpMethod::Manual,
            "link-local" => IpMethod::LinkLocal,
            "shared" => IpMethod::Shared,
            "disabled" | "ignore" => IpMethod::Disabled,
            other => IpMethod::Other(other.to_string()),
        }
    }

    /// The NetworkManager name for `family`. IPv6 says `ignore`, which every
    /// NetworkManager version knows, for no address.
    pub fn name(&self, family: Family) -> String {
        match self {
            IpMethod::Auto => "auto".to_string(),
            IpMethod::Manual => "manual".to_string(),
            IpMethod::LinkLocal => "link-local".to_string(),
            IpMethod::Shared => "shared".to_string(),
            IpMethod::Disabled if family == Family::V6 => "ignore".to_string(),
            IpMethod::Disabled => "disabled".to_string(),
            IpMethod::Other(name) => name.clone(),
        }
    }
}

/// One address with its prefix length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpAddress {
    pub address: String,
    pub prefix: u32,
}

/// The IP settings of one family.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpConfig {
    pub method: IpMethod,
    pub addresses: Vec<IpAddress>,
    /// Empty for none.
    pub gateway: String,
    pub dns: Vec<String>,
    /// Use only the DNS servers above, not the ones the network gives.
    pub ignore_auto_dns: bool,
    /// Never use this connection for the default route (for a VPN: "use only
    /// for resources on its network").
    pub never_default: bool,
}

impl IpConfig {
    /// A configuration with `method` and nothing else.
    pub fn with_method(method: IpMethod) -> Self {
        Self { method, addresses: Vec::new(), gateway: String::new(), dns: Vec::new(), ignore_auto_dns: false, never_default: false }
    }

    /// Read the group of `family`. A profile without it gets `Auto`.
    pub fn from_settings(s: &Settings, family: Family) -> Self {
        let g = family.group();
        let method = settings::text(s, g, "method").map(|m| IpMethod::from_name(&m)).unwrap_or(IpMethod::Auto);
        let addresses = settings::dicts(s, g, "address-data")
            .iter()
            .filter_map(|d| Some(IpAddress { address: as_text(d.get("address")?)?, prefix: as_uint(d.get("prefix")?)? }))
            .collect();
        Self {
            method,
            addresses,
            gateway: settings::text(s, g, "gateway").unwrap_or_default(),
            dns: read_dns(s, family),
            ignore_auto_dns: settings::boolean(s, g, "ignore-auto-dns").unwrap_or(false),
            never_default: settings::boolean(s, g, "never-default").unwrap_or(false),
        }
    }

    /// Write the group of `family` over what `s` holds. The legacy
    /// `addresses` key is dropped, because NetworkManager would prefer it to
    /// `address-data`, and `dns-data` too, because `dns` is written instead.
    pub fn write(&self, s: &mut Settings, family: Family) {
        let g = family.group();
        settings::set(s, g, "method", self.method.name(family));
        let data: Vec<HashMap<String, Value>> = self
            .addresses
            .iter()
            .map(|a| HashMap::from([("address".to_string(), Value::from(a.address.clone())), ("prefix".to_string(), Value::from(a.prefix))]))
            .collect();
        settings::set(s, g, "address-data", data);
        settings::remove(s, g, "addresses");
        if self.gateway.trim().is_empty() {
            settings::remove(s, g, "gateway");
        } else {
            settings::set(s, g, "gateway", self.gateway.trim().to_string());
        }
        settings::remove(s, g, "dns-data");
        write_dns(s, family, &self.dns);
        settings::set(s, g, "ignore-auto-dns", self.ignore_auto_dns);
        settings::set(s, g, "never-default", self.never_default);
    }

    /// The problems of this configuration, as `(field, text)`; `prefix` names
    /// the tab (`ipv4` or `ipv6`).
    pub fn validate(&self, family: Family) -> Vec<(String, String)> {
        let g = family.group();
        let mut out = Vec::new();
        if self.method == IpMethod::Manual && self.addresses.is_empty() {
            out.push((format!("{g}.addresses"), "Manual needs at least one address".to_string()));
        }
        for a in &self.addresses {
            if a.address.trim().is_empty() {
                out.push((format!("{g}.addresses"), "Write the address or remove its line".to_string()));
            } else if !family.parses(a.address.trim()) {
                out.push((format!("{g}.addresses"), format!("{} is not a valid address", a.address)));
            }
            if a.prefix == 0 || a.prefix > family.max_prefix() {
                out.push((format!("{g}.addresses"), format!("The prefix must be 1 to {}", family.max_prefix())));
            }
        }
        if !self.gateway.trim().is_empty() && !family.parses(self.gateway.trim()) {
            out.push((format!("{g}.gateway"), format!("{} is not a valid gateway", self.gateway)));
        }
        for d in &self.dns {
            if !family.parses(d.trim()) {
                out.push((format!("{g}.dns"), format!("{d} is not a valid DNS server")));
            }
        }
        out
    }
}

/// The DNS servers: `dns-data` of newer NetworkManager, else the legacy `dns`.
fn read_dns(s: &Settings, family: Family) -> Vec<String> {
    let g = family.group();
    let data = settings::texts(s, g, "dns-data");
    if !data.is_empty() { return data; }
    match family {
        Family::V4 => settings::uints(s, g, "dns").into_iter().map(|n| Ipv4Addr::from(n.to_ne_bytes()).to_string()).collect(),
        Family::V6 => settings::byte_lists(s, g, "dns")
            .into_iter()
            .filter_map(|b| <[u8; 16]>::try_from(b).ok())
            .map(|b| Ipv6Addr::from(b).to_string())
            .collect(),
    }
}

/// Write the legacy `dns` key, which every NetworkManager version reads:
/// IPv4 as numbers in network byte order, IPv6 as byte arrays. Entries that
/// are not addresses are skipped (validation reports them).
fn write_dns(s: &mut Settings, family: Family, dns: &[String]) {
    let g = family.group();
    let addrs = dns.iter().filter_map(|d| d.trim().parse::<IpAddr>().ok());
    match family {
        Family::V4 => {
            let list: Vec<u32> = addrs
                .filter_map(|a| match a {
                    IpAddr::V4(a) => Some(u32::from_ne_bytes(a.octets())),
                    IpAddr::V6(_) => None,
                })
                .collect();
            settings::set(s, g, "dns", list);
        }
        Family::V6 => {
            let list: Vec<Vec<u8>> = addrs
                .filter_map(|a| match a {
                    IpAddr::V6(a) => Some(a.octets().to_vec()),
                    IpAddr::V4(_) => None,
                })
                .collect();
            settings::set(s, g, "dns", list);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual() -> IpConfig {
        IpConfig {
            method: IpMethod::Manual,
            addresses: vec![IpAddress { address: "192.168.1.10".into(), prefix: 24 }],
            gateway: "192.168.1.1".into(),
            dns: vec!["1.1.1.1".into(), "9.9.9.9".into()],
            ignore_auto_dns: true,
            never_default: true,
        }
    }

    #[test]
    fn an_ipv4_configuration_survives_a_round_trip() {
        let mut s = Settings::new();
        manual().write(&mut s, Family::V4);
        assert_eq!(IpConfig::from_settings(&s, Family::V4), manual());
    }

    #[test]
    fn an_ipv6_configuration_survives_a_round_trip() {
        let c = IpConfig {
            method: IpMethod::Manual,
            addresses: vec![IpAddress { address: "fd00::2".into(), prefix: 64 }],
            gateway: "fd00::1".into(),
            dns: vec!["2606:4700:4700::1111".into()],
            ignore_auto_dns: false,
            never_default: false,
        };
        let mut s = Settings::new();
        c.write(&mut s, Family::V6);
        assert_eq!(IpConfig::from_settings(&s, Family::V6), c);
    }

    #[test]
    fn ipv4_dns_is_in_network_byte_order() {
        let mut s = Settings::new();
        manual().write(&mut s, Family::V4);
        assert_eq!(settings::uints(&s, "ipv4", "dns")[0], u32::from_ne_bytes([1, 1, 1, 1]));
        settings::set(&mut s, "ipv4", "dns", vec![u32::from_ne_bytes([192, 168, 0, 1])]);
        assert_eq!(read_dns(&s, Family::V4), vec!["192.168.0.1".to_string()]);
    }

    #[test]
    fn the_legacy_keys_are_dropped_and_dns_data_is_preferred_when_reading() {
        let mut s = Settings::new();
        settings::set(&mut s, "ipv4", "addresses", vec![vec![1u32, 24, 0]]);
        settings::set(&mut s, "ipv4", "dns-data", vec!["8.8.8.8".to_string()]);
        assert_eq!(IpConfig::from_settings(&s, Family::V4).dns, vec!["8.8.8.8".to_string()]);
        manual().write(&mut s, Family::V4);
        assert!(settings::value(&s, "ipv4", "addresses").is_none());
        assert!(settings::value(&s, "ipv4", "dns-data").is_none());
    }

    #[test]
    fn ipv6_off_is_written_as_ignore_and_read_as_disabled() {
        let mut s = Settings::new();
        IpConfig::with_method(IpMethod::Disabled).write(&mut s, Family::V6);
        assert_eq!(settings::text(&s, "ipv6", "method").as_deref(), Some("ignore"));
        assert_eq!(IpConfig::from_settings(&s, Family::V6).method, IpMethod::Disabled);
        settings::set(&mut s, "ipv6", "method", "disabled");
        assert_eq!(IpConfig::from_settings(&s, Family::V6).method, IpMethod::Disabled);
    }

    #[test]
    fn an_unknown_method_is_kept() {
        let mut s = Settings::new();
        settings::set(&mut s, "ipv6", "method", "dhcp");
        let c = IpConfig::from_settings(&s, Family::V6);
        assert_eq!(c.method, IpMethod::Other("dhcp".into()));
        c.write(&mut s, Family::V6);
        assert_eq!(settings::text(&s, "ipv6", "method").as_deref(), Some("dhcp"));
    }

    #[test]
    fn an_empty_gateway_is_removed() {
        let mut s = Settings::new();
        manual().write(&mut s, Family::V4);
        IpConfig { gateway: String::new(), ..manual() }.write(&mut s, Family::V4);
        assert!(settings::value(&s, "ipv4", "gateway").is_none());
    }

    #[test]
    fn validation_names_the_wrong_fields() {
        assert!(manual().validate(Family::V4).is_empty());
        let empty = IpConfig::with_method(IpMethod::Manual);
        assert_eq!(empty.validate(Family::V4)[0].0, "ipv4.addresses");
        let bad = IpConfig {
            addresses: vec![IpAddress { address: "300.1.1.1".into(), prefix: 33 }],
            gateway: "x".into(),
            dns: vec!["fd00::1".into()],
            ..manual()
        };
        let fields: Vec<String> = bad.validate(Family::V4).into_iter().map(|(f, _)| f).collect();
        assert_eq!(fields, vec!["ipv4.addresses", "ipv4.addresses", "ipv4.gateway", "ipv4.dns"]);
        assert!(manual().validate(Family::V6).iter().any(|(f, _)| f == "ipv6.addresses"));
    }
}
