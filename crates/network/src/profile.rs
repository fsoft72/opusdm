//! The typed model of one connection profile, read from and written to the
//! `a{sa{sv}}` map of NetworkManager.
//!
//! Writing starts from the map that was read, so the keys this model does not
//! know are kept. Secrets are never written into the map: they live in the
//! keyring (see `secrets`) and the map only says who owns them.

use crate::error::NetError;
use crate::ipconfig::{Family, IpConfig, IpMethod};
use crate::settings::{self, as_text, as_texts, as_uint, Settings};
use std::collections::{BTreeMap, HashMap};
use zbus::zvariant::Value;

/// `connection.type` of a Wi-Fi profile.
pub const TYPE_WIFI: &str = "802-11-wireless";
/// `connection.type` of an Ethernet profile.
pub const TYPE_ETHERNET: &str = "802-3-ethernet";
/// `connection.type` of a WireGuard profile.
pub const TYPE_WIREGUARD: &str = "wireguard";
/// `connection.type` of a VPN plugin profile.
pub const TYPE_VPN: &str = "vpn";
/// `vpn.service-type` of the OpenVPN plugin.
pub const OPENVPN_SERVICE: &str = "org.freedesktop.NetworkManager.openvpn";
/// The group of the Wi-Fi key.
pub const WIFI_SECURITY: &str = "802-11-wireless-security";

const CONNECTION: &str = "connection";
const VPN: &str = "vpn";
/// `NM_SETTING_SECRET_FLAG_AGENT_OWNED`.
pub const FLAG_AGENT_OWNED: u32 = 1;
/// `NM_SETTING_SECRET_FLAG_NOT_SAVED`: asked at every connect.
pub const FLAG_NOT_SAVED: u32 = 2;
/// The longest Linux interface name.
const IFNAME_MAX: usize = 15;

/// A secret value. `Debug` never shows it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether there is no value.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "Secret(empty)" } else { "Secret(***)" })
    }
}

/// Where a secret is found: the setting and the key inside it. WireGuard
/// peer keys are `peers.<public key>.preshared-key`, as `nm-applet` names them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SecretKey {
    pub setting: String,
    pub key: String,
}

impl SecretKey {
    /// Build a key.
    pub fn new(setting: &str, key: &str) -> Self {
        Self { setting: setting.to_string(), key: key.to_string() }
    }
}

/// The kinds a new profile can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileKind {
    Ethernet,
    Wifi,
    WireGuard,
    OpenVpn,
    /// Anything this program does not edit.
    Other,
}

impl ProfileKind {
    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            ProfileKind::Ethernet => "Ethernet",
            ProfileKind::Wifi => "Wi-Fi",
            ProfileKind::WireGuard => "WireGuard",
            ProfileKind::OpenVpn => "OpenVPN",
            ProfileKind::Other => "Other",
        }
    }

    /// Whether the profile is a VPN.
    pub fn is_vpn(self) -> bool {
        matches!(self, ProfileKind::WireGuard | ProfileKind::OpenVpn)
    }

    /// The kind of a profile from its `connection.type` and `vpn.service-type`.
    pub fn of(conn_type: &str, vpn_service: Option<&str>) -> Self {
        match (conn_type, vpn_service) {
            (TYPE_WIFI, _) => ProfileKind::Wifi,
            (TYPE_ETHERNET, _) => ProfileKind::Ethernet,
            (TYPE_WIREGUARD, _) => ProfileKind::WireGuard,
            (TYPE_VPN, Some(OPENVPN_SERVICE)) => ProfileKind::OpenVpn,
            _ => ProfileKind::Other,
        }
    }
}

/// How a Wi-Fi network is protected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WifiSecurity {
    Open,
    /// WPA or WPA2 personal.
    WpaPsk,
    /// WPA3 personal.
    Sae,
    /// A `key-mgmt` this program does not edit (WEP, 802.1X); kept unchanged.
    Other(String),
}

/// The Wi-Fi part of a profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiDetail {
    pub ssid: String,
    /// `infrastructure`, `adhoc` or `ap`.
    pub mode: String,
    pub hidden: bool,
    pub security: WifiSecurity,
    pub psk: Secret,
}

/// The Ethernet part of a profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EthernetDetail {
    /// The MAC address the device uses (`assigned-mac-address`); empty for its own.
    pub cloned_mac: String,
    /// 0 for automatic.
    pub mtu: u32,
}

/// One WireGuard peer.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Peer {
    pub public_key: String,
    /// `host:port`, empty when the peer connects to us.
    pub endpoint: String,
    pub allowed_ips: Vec<String>,
    pub preshared_key: Secret,
    /// Seconds, 0 for off.
    pub keepalive: u32,
}

/// The WireGuard part of a profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireGuardDetail {
    pub private_key: Secret,
    /// 0 for a random port.
    pub listen_port: u32,
    /// 0 for automatic.
    pub mtu: u32,
    pub fwmark: u32,
    pub peers: Vec<Peer>,
}

/// How an OpenVPN client proves who it is (`connection-type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenVpnAuth {
    /// Certificates (TLS).
    Tls,
    Password,
    /// Password and certificates.
    PasswordTls,
    StaticKey,
}

impl OpenVpnAuth {
    /// Every value, in the order shown.
    pub const ALL: [OpenVpnAuth; 4] = [OpenVpnAuth::Tls, OpenVpnAuth::Password, OpenVpnAuth::PasswordTls, OpenVpnAuth::StaticKey];

    /// The `connection-type` value.
    pub fn name(self) -> &'static str {
        match self {
            OpenVpnAuth::Tls => "tls",
            OpenVpnAuth::Password => "password",
            OpenVpnAuth::PasswordTls => "password-tls",
            OpenVpnAuth::StaticKey => "static-key",
        }
    }

    /// The text shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            OpenVpnAuth::Tls => "Certificates (TLS)",
            OpenVpnAuth::Password => "Password",
            OpenVpnAuth::PasswordTls => "Password with certificates",
            OpenVpnAuth::StaticKey => "Static key",
        }
    }

    /// Read a `connection-type` value; unknown ones count as certificates,
    /// the plugin's own default.
    pub fn from_name(name: &str) -> Self {
        Self::ALL.into_iter().find(|a| a.name() == name).unwrap_or(OpenVpnAuth::Tls)
    }

    /// Whether a user name and password are used.
    pub fn uses_password(self) -> bool {
        matches!(self, OpenVpnAuth::Password | OpenVpnAuth::PasswordTls)
    }

    /// Whether a client certificate and key are used.
    pub fn uses_certificate(self) -> bool {
        matches!(self, OpenVpnAuth::Tls | OpenVpnAuth::PasswordTls)
    }
}

/// The keys of the OpenVPN plugin's `vpn.data` this program edits.
pub mod ovpn_key {
    pub const CONNECTION_TYPE: &str = "connection-type";
    pub const REMOTE: &str = "remote";
    pub const PORT: &str = "port";
    pub const PROTO_TCP: &str = "proto-tcp";
    pub const DEV_TYPE: &str = "dev-type";
    pub const USERNAME: &str = "username";
    pub const CA: &str = "ca";
    pub const CERT: &str = "cert";
    pub const KEY: &str = "key";
    pub const TA: &str = "ta";
    pub const TA_DIR: &str = "ta-dir";
    pub const TLS_CRYPT: &str = "tls-crypt";
    pub const STATIC_KEY: &str = "static-key";
    pub const STATIC_KEY_DIRECTION: &str = "static-key-direction";
    pub const LOCAL_IP: &str = "local-ip";
    pub const REMOTE_IP: &str = "remote-ip";
    pub const CIPHER: &str = "cipher";
    pub const AUTH: &str = "auth";
    pub const COMPRESS: &str = "compress";
    pub const COMP_LZO: &str = "comp-lzo";
    pub const VERIFY_X509_NAME: &str = "verify-x509-name";
    pub const REMOTE_CERT_TLS: &str = "remote-cert-tls";
    pub const TUNNEL_MTU: &str = "tunnel-mtu";
    pub const MSSFIX: &str = "mssfix";
    pub const PASSWORD_FLAGS: &str = "password-flags";
    pub const CERT_PASS_FLAGS: &str = "cert-pass-flags";
    /// The secret names inside `vpn.secrets`.
    pub const PASSWORD: &str = "password";
    pub const CERT_PASS: &str = "cert-pass";
}

/// The OpenVPN part of a profile. `data` holds every `vpn.data` key, so the
/// options this program does not show are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenVpnDetail {
    pub data: BTreeMap<String, String>,
    pub password: Secret,
    /// The passphrase of the private key.
    pub cert_pass: Secret,
    /// Ask for the password at every connect instead of keeping it.
    pub ask_password: bool,
}

impl OpenVpnDetail {
    /// One `vpn.data` value, empty when missing.
    pub fn get(&self, key: &str) -> &str {
        self.data.get(key).map(String::as_str).unwrap_or("")
    }

    /// Set one `vpn.data` value; an empty one removes the key.
    pub fn put(&mut self, key: &str, value: &str) {
        if value.is_empty() {
            self.data.remove(key);
        } else {
            self.data.insert(key.to_string(), value.to_string());
        }
    }

    /// How the client authenticates.
    pub fn auth(&self) -> OpenVpnAuth {
        OpenVpnAuth::from_name(self.get(ovpn_key::CONNECTION_TYPE))
    }
}

/// The part of a profile that depends on its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detail {
    Wifi(WifiDetail),
    Ethernet(EthernetDetail),
    WireGuard(WireGuardDetail),
    OpenVpn(OpenVpnDetail),
    /// A kind this program does not edit: the type, and the VPN service if any.
    Other { conn_type: String, vpn_service: Option<String> },
}

/// One connection profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub uuid: String,
    /// The name.
    pub id: String,
    /// The device the profile is bound to; empty for any. WireGuard needs one:
    /// it is the name of the interface it creates.
    pub interface_name: String,
    pub autoconnect: bool,
    pub autoconnect_priority: i32,
    /// The VPNs (UUIDs) started together with this connection.
    pub secondaries: Vec<String>,
    pub ipv4: IpConfig,
    pub ipv6: IpConfig,
    pub detail: Detail,
}

impl Profile {
    /// A new profile of `kind` with a fresh UUID and the usual defaults.
    pub fn new(kind: ProfileKind) -> Result<Self, NetError> {
        let auto = || IpConfig::with_method(IpMethod::Auto);
        let (id, detail, ipv4, ipv6, autoconnect) = match kind {
            ProfileKind::Ethernet => ("Wired connection", Detail::Ethernet(EthernetDetail { cloned_mac: String::new(), mtu: 0 }), auto(), auto(), true),
            ProfileKind::Wifi => (
                "Wi-Fi",
                Detail::Wifi(WifiDetail { ssid: String::new(), mode: "infrastructure".into(), hidden: false, security: WifiSecurity::WpaPsk, psk: Secret::default() }),
                auto(),
                auto(),
                true,
            ),
            ProfileKind::WireGuard => (
                "WireGuard",
                Detail::WireGuard(WireGuardDetail {
                    private_key: Secret::new(crate::wgkey::generate()?.0),
                    listen_port: 0,
                    mtu: 0,
                    fwmark: 0,
                    peers: vec![Peer { allowed_ips: vec!["0.0.0.0/0".into(), "::/0".into()], ..Peer::default() }],
                }),
                IpConfig::with_method(IpMethod::Manual),
                IpConfig::with_method(IpMethod::Disabled),
                false,
            ),
            ProfileKind::OpenVpn => {
                let data = BTreeMap::from([(ovpn_key::CONNECTION_TYPE.to_string(), OpenVpnAuth::Tls.name().to_string())]);
                let detail = OpenVpnDetail { data, password: Secret::default(), cert_pass: Secret::default(), ask_password: false };
                ("VPN", Detail::OpenVpn(detail), auto(), auto(), false)
            }
            ProfileKind::Other => return Err(NetError::other("Profiles of this kind cannot be created here")),
        };
        let interface_name = if kind == ProfileKind::WireGuard { "wg0".to_string() } else { String::new() };
        Ok(Self { uuid: new_uuid()?, id: id.to_string(), interface_name, autoconnect, autoconnect_priority: 0, secondaries: Vec::new(), ipv4, ipv6, detail })
    }

    /// The kind of the profile.
    pub fn kind(&self) -> ProfileKind {
        match &self.detail {
            Detail::Wifi(_) => ProfileKind::Wifi,
            Detail::Ethernet(_) => ProfileKind::Ethernet,
            Detail::WireGuard(_) => ProfileKind::WireGuard,
            Detail::OpenVpn(_) => ProfileKind::OpenVpn,
            Detail::Other { .. } => ProfileKind::Other,
        }
    }

    /// Read a profile. Secrets are not in the map: they stay empty until
    /// `set_secret` fills them.
    pub fn from_settings(s: &Settings) -> Result<Self, NetError> {
        let uuid = settings::text(s, CONNECTION, "uuid").ok_or_else(|| NetError::other("The profile has no uuid"))?;
        let conn_type = settings::text(s, CONNECTION, "type").unwrap_or_default();
        let vpn_service = settings::text(s, VPN, "service-type");
        let detail = match ProfileKind::of(&conn_type, vpn_service.as_deref()) {
            ProfileKind::Wifi => Detail::Wifi(read_wifi(s)),
            ProfileKind::Ethernet => Detail::Ethernet(read_ethernet(s)),
            ProfileKind::WireGuard => Detail::WireGuard(read_wireguard(s)),
            ProfileKind::OpenVpn => Detail::OpenVpn(read_openvpn(s)),
            ProfileKind::Other => Detail::Other { conn_type, vpn_service },
        };
        Ok(Self {
            id: settings::text(s, CONNECTION, "id").unwrap_or_else(|| uuid.clone()),
            uuid,
            interface_name: settings::text(s, CONNECTION, "interface-name").unwrap_or_default(),
            autoconnect: settings::boolean(s, CONNECTION, "autoconnect").unwrap_or(true),
            autoconnect_priority: settings::int(s, CONNECTION, "autoconnect-priority").unwrap_or(0),
            secondaries: settings::texts(s, CONNECTION, "secondaries"),
            ipv4: IpConfig::from_settings(s, Family::V4),
            ipv6: IpConfig::from_settings(s, Family::V6),
            detail,
        })
    }

    /// The map to send to NetworkManager: `base` (the map that was read, or
    /// an empty one for a new profile) with this model written over it. No
    /// secret value is written; each secret is marked as owned by the agent
    /// (or asked every time).
    pub fn to_settings(&self, base: &Settings) -> Settings {
        let mut s = base.clone();
        let conn_type = match &self.detail {
            Detail::Wifi(_) => TYPE_WIFI.to_string(),
            Detail::Ethernet(_) => TYPE_ETHERNET.to_string(),
            Detail::WireGuard(_) => TYPE_WIREGUARD.to_string(),
            Detail::OpenVpn(_) => TYPE_VPN.to_string(),
            Detail::Other { conn_type, .. } => conn_type.clone(),
        };
        settings::set(&mut s, CONNECTION, "id", self.id.trim().to_string());
        settings::set(&mut s, CONNECTION, "uuid", self.uuid.clone());
        settings::set(&mut s, CONNECTION, "type", conn_type);
        settings::set(&mut s, CONNECTION, "autoconnect", self.autoconnect);
        settings::set(&mut s, CONNECTION, "autoconnect-priority", self.autoconnect_priority);
        settings::set(&mut s, CONNECTION, "secondaries", self.secondaries.clone());
        if self.interface_name.trim().is_empty() {
            settings::remove(&mut s, CONNECTION, "interface-name");
        } else {
            settings::set(&mut s, CONNECTION, "interface-name", self.interface_name.trim().to_string());
        }
        match &self.detail {
            Detail::Wifi(w) => write_wifi(&mut s, w),
            Detail::Ethernet(e) => write_ethernet(&mut s, e),
            Detail::WireGuard(w) => write_wireguard(&mut s, w),
            Detail::OpenVpn(o) => write_openvpn(&mut s, o),
            Detail::Other { .. } => return s,
        }
        self.ipv4.write(&mut s, Family::V4);
        self.ipv6.write(&mut s, Family::V6);
        s
    }

    /// The secrets to keep, with where they belong. Empty ones are left out,
    /// and so is an OpenVPN password that is asked every time.
    pub fn secrets(&self) -> Vec<(SecretKey, &Secret)> {
        let mut out = self.secret_slots();
        out.retain(|(_, v)| !v.is_empty());
        out
    }

    /// Whether saving this profile manages its secrets in the keyring. Not
    /// for kinds (or Wi-Fi security) this program does not edit: their
    /// secrets, such as 802.1X passwords, belong to other tools.
    pub fn manages_secrets(&self) -> bool {
        match &self.detail {
            Detail::Wifi(w) => !matches!(w.security, WifiSecurity::Other(_)),
            Detail::WireGuard(_) | Detail::OpenVpn(_) => true,
            Detail::Ethernet(_) | Detail::Other { .. } => false,
        }
    }

    /// Every secret this profile can hold, empty or not, with its key.
    pub fn secret_slots(&self) -> Vec<(SecretKey, &Secret)> {
        let mut out = Vec::new();
        match &self.detail {
            Detail::Wifi(w) if matches!(w.security, WifiSecurity::WpaPsk | WifiSecurity::Sae) => {
                out.push((SecretKey::new(WIFI_SECURITY, "psk"), &w.psk));
            }
            Detail::WireGuard(w) => {
                out.push((SecretKey::new(TYPE_WIREGUARD, "private-key"), &w.private_key));
                for p in &w.peers {
                    out.push((peer_secret_key(&p.public_key), &p.preshared_key));
                }
            }
            Detail::OpenVpn(o) => {
                if o.auth().uses_password() && !o.ask_password {
                    out.push((SecretKey::new(VPN, ovpn_key::PASSWORD), &o.password));
                }
                out.push((SecretKey::new(VPN, ovpn_key::CERT_PASS), &o.cert_pass));
            }
            _ => {}
        }
        out
    }

    /// Fill the secrets found in `s`, an answer of NetworkManager's
    /// `GetSecrets` (the ones it keeps itself).
    pub fn fill_secrets(&mut self, s: &Settings) {
        if let Some(psk) = settings::text(s, WIFI_SECURITY, "psk") {
            self.set_secret(&SecretKey::new(WIFI_SECURITY, "psk"), &psk);
        }
        for (k, v) in settings::text_map(s, VPN, "secrets") {
            self.set_secret(&SecretKey::new(VPN, &k), &v);
        }
        if let Some(private) = settings::text(s, TYPE_WIREGUARD, "private-key") {
            self.set_secret(&SecretKey::new(TYPE_WIREGUARD, "private-key"), &private);
        }
        for peer in settings::dicts(s, TYPE_WIREGUARD, "peers") {
            let (Some(public), Some(psk)) = (peer.get("public-key").and_then(as_text), peer.get("preshared-key").and_then(as_text)) else { continue };
            self.set_secret(&peer_secret_key(&public), &psk);
        }
    }

    /// Fill one secret read from the keyring or from NetworkManager. A key
    /// this profile has no place for is ignored.
    pub fn set_secret(&mut self, key: &SecretKey, value: &str) {
        let value = Secret::new(value);
        match (&mut self.detail, key.setting.as_str(), key.key.as_str()) {
            (Detail::Wifi(w), WIFI_SECURITY, "psk") => w.psk = value,
            (Detail::WireGuard(w), TYPE_WIREGUARD, "private-key") => w.private_key = value,
            (Detail::WireGuard(w), TYPE_WIREGUARD, k) => {
                if let Some(p) = w.peers.iter_mut().find(|p| peer_secret_key(&p.public_key).key == k) {
                    p.preshared_key = value;
                }
            }
            (Detail::OpenVpn(o), VPN, ovpn_key::PASSWORD) => o.password = value,
            (Detail::OpenVpn(o), VPN, ovpn_key::CERT_PASS) => o.cert_pass = value,
            _ => {}
        }
    }

    /// The problems that stop a save, as `(field, text)`. Fields are named
    /// `general.*`, `wifi.*`, `ethernet.*`, `wireguard.*`, `openvpn.*`,
    /// `ipv4.*` and `ipv6.*`.
    pub fn validate(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut err = |field: &str, text: &str| out.push((field.to_string(), text.to_string()));
        if self.id.trim().is_empty() { err("general.id", "The name cannot be empty"); }
        let ifname = self.interface_name.trim();
        if ifname.len() > IFNAME_MAX || ifname.contains(['/', ' ']) {
            err("general.interface", "An interface name has up to 15 characters, no spaces or slashes");
        }
        match &self.detail {
            Detail::Wifi(w) => {
                if w.ssid.is_empty() || w.ssid.len() > 32 { err("wifi.ssid", "The network name must have 1 to 32 characters"); }
                let psk_len = w.psk.as_str().len();
                if matches!(w.security, WifiSecurity::WpaPsk | WifiSecurity::Sae) && !(8..=63).contains(&psk_len) && psk_len != 64 {
                    err("wifi.psk", "The password must have 8 to 63 characters");
                }
            }
            Detail::Ethernet(e) => {
                if !e.cloned_mac.is_empty() && !is_mac(&e.cloned_mac) { err("ethernet.mac", "Write the MAC address as 12:34:56:78:9a:bc"); }
            }
            Detail::WireGuard(w) => {
                if ifname.is_empty() { err("general.interface", "WireGuard needs an interface name"); }
                if !crate::wgkey::is_key(w.private_key.as_str()) { err("wireguard.private-key", "The private key is not a WireGuard key"); }
                if w.listen_port > u32::from(u16::MAX) { err("wireguard.port", "The port must be 0 to 65535"); }
                if w.peers.is_empty() { err("wireguard.peers", "Add at least one peer"); }
                for p in &w.peers {
                    if !crate::wgkey::is_key(&p.public_key) { err("wireguard.peers", "A peer public key is not a WireGuard key"); }
                    if !p.endpoint.trim().is_empty() && !is_endpoint(p.endpoint.trim()) { err("wireguard.endpoint", "Write the endpoint as host:port"); }
                    if !p.preshared_key.is_empty() && !crate::wgkey::is_key(p.preshared_key.as_str()) {
                        err("wireguard.peers", "A preshared key is not a WireGuard key");
                    }
                }
                for ip in [&self.ipv4, &self.ipv6] {
                    if !matches!(ip.method, IpMethod::Manual | IpMethod::Disabled) {
                        err("ipv4.method", "WireGuard needs manual addresses or no addresses");
                    }
                }
            }
            Detail::OpenVpn(o) => {
                if o.get(ovpn_key::REMOTE).trim().is_empty() { err("openvpn.remote", "The gateway cannot be empty"); }
                let auth = o.auth();
                if auth != OpenVpnAuth::StaticKey && o.get(ovpn_key::CA).is_empty() { err("openvpn.ca", "Choose the CA certificate"); }
                if auth.uses_certificate() && o.get(ovpn_key::CERT).is_empty() { err("openvpn.cert", "Choose the user certificate"); }
                if auth.uses_certificate() && o.get(ovpn_key::KEY).is_empty() { err("openvpn.key", "Choose the private key"); }
                if auth.uses_password() && o.get(ovpn_key::USERNAME).is_empty() { err("openvpn.username", "The user name cannot be empty"); }
                if auth.uses_password() && !o.ask_password && o.password.is_empty() {
                    err("openvpn.password", "Write the password, or ask for it at every connect");
                }
                if auth == OpenVpnAuth::StaticKey && o.get(ovpn_key::STATIC_KEY).is_empty() { err("openvpn.static-key", "Choose the static key"); }
            }
            Detail::Other { .. } => {}
        }
        out.extend(self.ipv4.validate(Family::V4));
        out.extend(self.ipv6.validate(Family::V6));
        out
    }
}

/// The keyring key of the preshared key of the peer `public_key`.
pub fn peer_secret_key(public_key: &str) -> SecretKey {
    SecretKey::new(TYPE_WIREGUARD, &format!("peers.{public_key}.preshared-key"))
}

/// Whether `text` is a WireGuard endpoint: `host:port` or `[v6 address]:port`.
fn is_endpoint(text: &str) -> bool {
    let Some((host, port)) = text.rsplit_once(':') else { return false };
    !host.is_empty() && !host.ends_with(':') && port.parse::<u16>().is_ok_and(|p| p > 0)
}

/// Whether `text` is a MAC address like `12:34:56:78:9a:bc`.
fn is_mac(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// A random (version 4) UUID.
pub fn new_uuid() -> Result<String, NetError> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| NetError::other(format!("No random numbers: {e}")))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]))
}

/// The Wi-Fi part of `s`.
fn read_wifi(s: &Settings) -> WifiDetail {
    let ssid = settings::bytes(s, TYPE_WIFI, "ssid").unwrap_or_default();
    let security = match settings::text(s, WIFI_SECURITY, "key-mgmt").as_deref() {
        None => WifiSecurity::Open,
        Some("wpa-psk") => WifiSecurity::WpaPsk,
        Some("sae") => WifiSecurity::Sae,
        Some(other) => WifiSecurity::Other(other.to_string()),
    };
    WifiDetail {
        ssid: String::from_utf8_lossy(&ssid).into_owned(),
        mode: settings::text(s, TYPE_WIFI, "mode").unwrap_or_else(|| "infrastructure".into()),
        hidden: settings::boolean(s, TYPE_WIFI, "hidden").unwrap_or(false),
        security,
        psk: Secret::default(),
    }
}

/// Write the Wi-Fi part. An open network loses its security group; a
/// security this program does not edit is left as it is.
fn write_wifi(s: &mut Settings, w: &WifiDetail) {
    settings::set(s, TYPE_WIFI, "ssid", w.ssid.as_bytes().to_vec());
    settings::set(s, TYPE_WIFI, "mode", w.mode.clone());
    settings::set(s, TYPE_WIFI, "hidden", w.hidden);
    let key_mgmt = match &w.security {
        WifiSecurity::Open => {
            s.remove(WIFI_SECURITY);
            settings::remove(s, TYPE_WIFI, "security");
            return;
        }
        WifiSecurity::WpaPsk => "wpa-psk",
        WifiSecurity::Sae => "sae",
        WifiSecurity::Other(_) => return,
    };
    settings::set(s, WIFI_SECURITY, "key-mgmt", key_mgmt);
    settings::set(s, WIFI_SECURITY, "psk-flags", FLAG_AGENT_OWNED);
    settings::remove(s, WIFI_SECURITY, "psk");
}

/// The Ethernet part of `s`.
fn read_ethernet(s: &Settings) -> EthernetDetail {
    EthernetDetail {
        cloned_mac: settings::text(s, TYPE_ETHERNET, "assigned-mac-address").unwrap_or_default(),
        mtu: settings::uint(s, TYPE_ETHERNET, "mtu").unwrap_or(0),
    }
}

/// Write the Ethernet part; the legacy `cloned-mac-address` is dropped
/// because NetworkManager would prefer it.
fn write_ethernet(s: &mut Settings, e: &EthernetDetail) {
    settings::remove(s, TYPE_ETHERNET, "cloned-mac-address");
    if e.cloned_mac.is_empty() {
        settings::remove(s, TYPE_ETHERNET, "assigned-mac-address");
    } else {
        settings::set(s, TYPE_ETHERNET, "assigned-mac-address", e.cloned_mac.clone());
    }
    settings::set(s, TYPE_ETHERNET, "mtu", e.mtu);
}

/// The WireGuard part of `s`.
fn read_wireguard(s: &Settings) -> WireGuardDetail {
    let peers = settings::dicts(s, TYPE_WIREGUARD, "peers")
        .iter()
        .map(|p| Peer {
            public_key: p.get("public-key").and_then(as_text).unwrap_or_default(),
            endpoint: p.get("endpoint").and_then(as_text).unwrap_or_default(),
            allowed_ips: p.get("allowed-ips").map(as_texts).unwrap_or_default(),
            preshared_key: Secret::default(),
            keepalive: p.get("persistent-keepalive").and_then(as_uint).unwrap_or(0),
        })
        .collect();
    WireGuardDetail {
        private_key: Secret::default(),
        listen_port: settings::uint(s, TYPE_WIREGUARD, "listen-port").unwrap_or(0),
        mtu: settings::uint(s, TYPE_WIREGUARD, "mtu").unwrap_or(0),
        fwmark: settings::uint(s, TYPE_WIREGUARD, "fwmark").unwrap_or(0),
        peers,
    }
}

/// Write the WireGuard part. The peers are written from the model.
fn write_wireguard(s: &mut Settings, w: &WireGuardDetail) {
    settings::remove(s, TYPE_WIREGUARD, "private-key");
    settings::set(s, TYPE_WIREGUARD, "private-key-flags", FLAG_AGENT_OWNED);
    settings::set(s, TYPE_WIREGUARD, "listen-port", w.listen_port);
    settings::set(s, TYPE_WIREGUARD, "mtu", w.mtu);
    settings::set(s, TYPE_WIREGUARD, "fwmark", w.fwmark);
    let peers: Vec<HashMap<String, Value>> = w
        .peers
        .iter()
        .map(|p| {
            let mut d = HashMap::from([
                ("public-key".to_string(), Value::from(p.public_key.trim().to_string())),
                ("allowed-ips".to_string(), Value::from(p.allowed_ips.iter().map(|a| a.trim().to_string()).collect::<Vec<_>>())),
                ("persistent-keepalive".to_string(), Value::from(p.keepalive)),
            ]);
            if !p.endpoint.trim().is_empty() {
                d.insert("endpoint".to_string(), Value::from(p.endpoint.trim().to_string()));
            }
            if !p.preshared_key.is_empty() {
                d.insert("preshared-key-flags".to_string(), Value::from(FLAG_AGENT_OWNED));
            }
            d
        })
        .collect();
    settings::set(s, TYPE_WIREGUARD, "peers", peers);
}

/// The OpenVPN part of `s`.
fn read_openvpn(s: &Settings) -> OpenVpnDetail {
    let mut data: BTreeMap<String, String> = settings::text_map(s, VPN, "data").into_iter().collect();
    let ask_password = data.get(ovpn_key::PASSWORD_FLAGS).is_some_and(|f| f.parse::<u32>().is_ok_and(|f| f & FLAG_NOT_SAVED != 0));
    // The flags follow the model and are written again on save.
    data.remove(ovpn_key::PASSWORD_FLAGS);
    data.remove(ovpn_key::CERT_PASS_FLAGS);
    OpenVpnDetail { data, password: Secret::default(), cert_pass: Secret::default(), ask_password }
}

/// Write the OpenVPN part: `vpn.data` with the secret flags, never `vpn.secrets`.
fn write_openvpn(s: &mut Settings, o: &OpenVpnDetail) {
    let mut data = o.data.clone();
    if o.auth().uses_password() {
        let flags = if o.ask_password { FLAG_NOT_SAVED } else { FLAG_AGENT_OWNED };
        data.insert(ovpn_key::PASSWORD_FLAGS.to_string(), flags.to_string());
    }
    let cert_flags = if o.cert_pass.is_empty() { 0 } else { FLAG_AGENT_OWNED };
    data.insert(ovpn_key::CERT_PASS_FLAGS.to_string(), cert_flags.to_string());
    settings::set(s, VPN, "service-type", OPENVPN_SERVICE);
    settings::set(s, VPN, "data", data.into_iter().collect::<HashMap<String, String>>());
    settings::remove(s, VPN, "secrets");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipconfig::IpAddress;

    const KEY_A: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const KEY_B: &str = "HIgo9xNzJMWLKASShiTqIybxZ0U3wGLiUeJ1PKf8ykw=";

    fn wireguard() -> Profile {
        let mut p = Profile::new(ProfileKind::WireGuard).unwrap();
        p.ipv4.addresses = vec![IpAddress { address: "10.0.0.2".into(), prefix: 32 }];
        if let Detail::WireGuard(w) = &mut p.detail {
            w.peers[0].public_key = KEY_B.into();
            w.peers[0].endpoint = "vpn.example.com:51820".into();
            w.peers[0].preshared_key = Secret::new(KEY_A);
            w.peers[0].keepalive = 25;
        }
        p
    }

    /// `p` written and read back, with its secrets put back from `secrets()`.
    fn round_trip(p: &Profile) -> Profile {
        let mut back = Profile::from_settings(&p.to_settings(&Settings::new())).unwrap();
        for (k, v) in p.secrets() {
            back.set_secret(&k, v.as_str());
        }
        back
    }

    #[test]
    fn every_editable_kind_survives_a_round_trip() {
        for kind in [ProfileKind::Ethernet, ProfileKind::Wifi, ProfileKind::OpenVpn] {
            let p = Profile::new(kind).unwrap();
            assert_eq!(round_trip(&p), p, "{kind:?}");
        }
        let wg = wireguard();
        assert_eq!(round_trip(&wg), wg);
    }

    #[test]
    fn no_secret_reaches_the_map() {
        let mut wifi = Profile::new(ProfileKind::Wifi).unwrap();
        if let Detail::Wifi(w) = &mut wifi.detail {
            w.psk = Secret::new("hunter2hunter2");
        }
        let s = wifi.to_settings(&Settings::new());
        assert!(settings::value(&s, WIFI_SECURITY, "psk").is_none());
        assert_eq!(settings::uint(&s, WIFI_SECURITY, "psk-flags"), Some(FLAG_AGENT_OWNED));
        let s = wireguard().to_settings(&Settings::new());
        assert!(settings::value(&s, TYPE_WIREGUARD, "private-key").is_none());
        assert!(!format!("{s:?}").contains(KEY_A));
    }

    #[test]
    fn unknown_keys_of_the_base_are_kept() {
        let mut base = Settings::new();
        settings::set(&mut base, "proxy", "method", "none");
        settings::set(&mut base, CONNECTION, "zone", "home");
        settings::set(&mut base, "ipv4", "route-metric", 100i64);
        let p = Profile::new(ProfileKind::Ethernet).unwrap();
        let s = p.to_settings(&base);
        assert_eq!(settings::text(&s, "proxy", "method").as_deref(), Some("none"));
        assert_eq!(settings::text(&s, CONNECTION, "zone").as_deref(), Some("home"));
        assert!(settings::value(&s, "ipv4", "route-metric").is_some());
    }

    #[test]
    fn an_open_wifi_loses_its_security_group() {
        let mut p = Profile::new(ProfileKind::Wifi).unwrap();
        let protected = p.to_settings(&Settings::new());
        assert!(protected.contains_key(WIFI_SECURITY));
        if let Detail::Wifi(w) = &mut p.detail {
            w.security = WifiSecurity::Open;
        }
        assert!(!p.to_settings(&protected).contains_key(WIFI_SECURITY));
    }

    #[test]
    fn an_enterprise_wifi_keeps_its_security_untouched() {
        let mut base = Settings::new();
        settings::set(&mut base, CONNECTION, "uuid", "u");
        settings::set(&mut base, CONNECTION, "type", TYPE_WIFI);
        settings::set(&mut base, WIFI_SECURITY, "key-mgmt", "wpa-eap");
        settings::set(&mut base, "802-1x", "eap", vec!["peap".to_string()]);
        let p = Profile::from_settings(&base).unwrap();
        assert!(matches!(&p.detail, Detail::Wifi(w) if w.security == WifiSecurity::Other("wpa-eap".into())));
        let s = p.to_settings(&base);
        assert_eq!(settings::text(&s, WIFI_SECURITY, "key-mgmt").as_deref(), Some("wpa-eap"));
        assert!(settings::value(&s, WIFI_SECURITY, "psk-flags").is_none());
        assert!(s.contains_key("802-1x"));
    }

    #[test]
    fn openvpn_password_flags_follow_the_ask_choice() {
        let mut p = Profile::new(ProfileKind::OpenVpn).unwrap();
        let Detail::OpenVpn(o) = &mut p.detail else { unreachable!() };
        o.put(ovpn_key::CONNECTION_TYPE, OpenVpnAuth::Password.name());
        o.password = Secret::new("pw");
        o.ask_password = true;
        let s = p.to_settings(&Settings::new());
        assert_eq!(settings::text_map(&s, VPN, "data")[ovpn_key::PASSWORD_FLAGS], "2");
        assert!(p.secrets().is_empty(), "an asked password is never kept");
        let back = Profile::from_settings(&s).unwrap();
        assert!(matches!(&back.detail, Detail::OpenVpn(o) if o.ask_password));
        let Detail::OpenVpn(o) = &mut p.detail else { unreachable!() };
        o.ask_password = false;
        let s = p.to_settings(&Settings::new());
        assert_eq!(settings::text_map(&s, VPN, "data")[ovpn_key::PASSWORD_FLAGS], "1");
        assert_eq!(p.secrets().len(), 1);
    }

    #[test]
    fn openvpn_data_keys_not_shown_are_kept() {
        let mut base = Settings::new();
        settings::set(&mut base, CONNECTION, "uuid", "u");
        settings::set(&mut base, CONNECTION, "type", TYPE_VPN);
        settings::set(&mut base, VPN, "service-type", OPENVPN_SERVICE);
        let data = HashMap::from([("ping".to_string(), "10".to_string()), ("remote".to_string(), "a:443, b:443".to_string())]);
        settings::set(&mut base, VPN, "data", data);
        let p = Profile::from_settings(&base).unwrap();
        let data = settings::text_map(&p.to_settings(&base), VPN, "data");
        assert_eq!(data["ping"], "10");
        assert_eq!(data["remote"], "a:443, b:443");
    }

    #[test]
    fn kinds_come_from_the_type_and_the_service() {
        assert_eq!(ProfileKind::of(TYPE_VPN, Some(OPENVPN_SERVICE)), ProfileKind::OpenVpn);
        assert_eq!(ProfileKind::of(TYPE_VPN, Some("org.freedesktop.NetworkManager.pptp")), ProfileKind::Other);
        assert_eq!(ProfileKind::of("bridge", None), ProfileKind::Other);
        assert!(ProfileKind::WireGuard.is_vpn());
        assert!(!ProfileKind::Wifi.is_vpn());
    }

    #[test]
    fn an_other_profile_is_written_back_untouched() {
        let mut base = Settings::new();
        settings::set(&mut base, CONNECTION, "uuid", "u");
        settings::set(&mut base, CONNECTION, "type", "bridge");
        settings::set(&mut base, "ipv4", "method", "shared");
        let p = Profile::from_settings(&base).unwrap();
        assert_eq!(p.kind(), ProfileKind::Other);
        let s = p.to_settings(&base);
        assert_eq!(settings::text(&s, "ipv4", "method").as_deref(), Some("shared"));
        assert!(Profile::new(ProfileKind::Other).is_err());
    }

    #[test]
    fn validation_catches_the_usual_mistakes() {
        assert!(wireguard().validate().is_empty(), "{:?}", wireguard().validate());
        let mut wifi = Profile::new(ProfileKind::Wifi).unwrap();
        let fields = |p: &Profile| p.validate().into_iter().map(|(f, _)| f).collect::<Vec<_>>();
        assert_eq!(fields(&wifi), vec!["wifi.ssid", "wifi.psk"]);
        if let Detail::Wifi(w) = &mut wifi.detail {
            w.ssid = "home".into();
            w.psk = Secret::new("12345678");
        }
        assert!(fields(&wifi).is_empty());
        let vpn = Profile::new(ProfileKind::OpenVpn).unwrap();
        assert_eq!(fields(&vpn), vec!["openvpn.remote", "openvpn.ca", "openvpn.cert", "openvpn.key"]);
        let mut wg = wireguard();
        wg.ipv4.method = IpMethod::Auto;
        wg.interface_name = String::new();
        assert_eq!(fields(&wg), vec!["general.interface", "ipv4.method"]);
        let mut eth = Profile::new(ProfileKind::Ethernet).unwrap();
        eth.id = " ".into();
        if let Detail::Ethernet(e) = &mut eth.detail {
            e.cloned_mac = "12:34".into();
        }
        assert_eq!(fields(&eth), vec!["general.id", "ethernet.mac"]);
    }

    #[test]
    fn secrets_kept_by_networkmanager_are_filled() {
        let mut p = wireguard();
        let Detail::WireGuard(w) = &mut p.detail else { unreachable!() };
        w.private_key = Secret::default();
        w.peers[0].preshared_key = Secret::default();
        let mut answer = Settings::new();
        settings::set(&mut answer, TYPE_WIREGUARD, "private-key", KEY_A);
        let peers: Vec<HashMap<String, Value>> =
            vec![HashMap::from([("public-key".to_string(), Value::from(KEY_B)), ("preshared-key".to_string(), Value::from(KEY_B))])];
        settings::set(&mut answer, TYPE_WIREGUARD, "peers", peers);
        p.fill_secrets(&answer);
        let Detail::WireGuard(w) = &p.detail else { unreachable!() };
        assert_eq!(w.private_key.as_str(), KEY_A);
        assert_eq!(w.peers[0].preshared_key.as_str(), KEY_B);
        let mut vpn = Profile::new(ProfileKind::OpenVpn).unwrap();
        let mut answer = Settings::new();
        settings::set(&mut answer, VPN, "secrets", HashMap::from([("cert-pass".to_string(), "pp".to_string())]));
        vpn.fill_secrets(&answer);
        assert!(matches!(&vpn.detail, Detail::OpenVpn(o) if o.cert_pass.as_str() == "pp"));
    }

    #[test]
    fn only_edited_kinds_manage_their_secrets() {
        assert!(wireguard().manages_secrets());
        assert!(Profile::new(ProfileKind::Wifi).unwrap().manages_secrets());
        assert!(!Profile::new(ProfileKind::Ethernet).unwrap().manages_secrets());
        let mut wifi = Profile::new(ProfileKind::Wifi).unwrap();
        if let Detail::Wifi(w) = &mut wifi.detail {
            w.security = WifiSecurity::Other("wpa-eap".into());
        }
        assert!(!wifi.manages_secrets());
    }

    #[test]
    fn endpoints_need_a_host_and_a_port() {
        assert!(is_endpoint("vpn.example.com:51820"));
        assert!(is_endpoint("[fd00::1]:51820"));
        assert!(!is_endpoint("vpn.example.com"));
        assert!(!is_endpoint(":51820"));
        assert!(!is_endpoint("host:0"));
        assert!(!is_endpoint("host:port"));
        let mut wg = wireguard();
        if let Detail::WireGuard(w) = &mut wg.detail {
            w.peers[0].endpoint = "a:1:2:x".into();
        }
        assert_eq!(wg.validate()[0].0, "wireguard.endpoint");
    }

    #[test]
    fn an_openvpn_password_is_needed_unless_it_is_asked() {
        let mut p = Profile::new(ProfileKind::OpenVpn).unwrap();
        let Detail::OpenVpn(o) = &mut p.detail else { unreachable!() };
        o.put(ovpn_key::CONNECTION_TYPE, "password");
        let has = |p: &Profile| p.validate().iter().any(|(f, _)| f == "openvpn.password");
        assert!(has(&p));
        let Detail::OpenVpn(o) = &mut p.detail else { unreachable!() };
        o.ask_password = true;
        assert!(!has(&p));
    }

    #[test]
    fn uuids_are_random_version_4() {
        let a = new_uuid().unwrap();
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert_ne!(a, new_uuid().unwrap());
    }

    #[test]
    fn debug_never_shows_a_secret() {
        let p = wireguard();
        assert!(!format!("{p:?}").contains(KEY_A));
    }
}
