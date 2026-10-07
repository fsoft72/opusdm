//! The network state as the panel sees it.

/// How a Wi-Fi network is protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    Open,
    /// WPA or WPA2 with a pre-shared key.
    Psk,
    /// WPA3 personal.
    Sae,
    /// WEP: old and insecure, not supported here.
    Wep,
    /// 802.1X: needs more than a password, not supported here.
    Enterprise,
}

impl Security {
    /// Whether a password is needed to join a network never saved.
    pub fn needs_password(self) -> bool {
        matches!(self, Security::Psk | Security::Sae)
    }

    /// Whether this tray can join the network.
    pub fn is_supported(self) -> bool {
        !matches!(self, Security::Wep | Security::Enterprise)
    }
}

/// One Wi-Fi network in range (access points with the same name are merged).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPoint {
    pub ssid: String,
    /// Signal strength, 0 to 100.
    pub signal: u8,
    pub security: Security,
    /// The network the Wi-Fi device is connected to.
    pub active: bool,
    /// A connection profile for this name already exists.
    pub saved: bool,
}

/// Everything the tray shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetState {
    pub wifi_present: bool,
    pub wifi_enabled: bool,
    pub eth_present: bool,
    /// The Ethernet device has a cable, so it can be switched on.
    pub eth_available: bool,
    /// Ethernet is connected or connecting.
    pub eth_on: bool,
    pub eth_connected: bool,
    /// Networks in range: the active one first, then by signal.
    pub aps: Vec<AccessPoint>,
}

impl NetState {
    /// The network the Wi-Fi device is connected to.
    pub fn active_ap(&self) -> Option<&AccessPoint> {
        self.aps.iter().find(|a| a.active)
    }
}
