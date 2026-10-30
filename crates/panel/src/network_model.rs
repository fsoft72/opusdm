//! The network state as the panel sees it: pure logic, no GTK.

use opusdm_network::{AccessPoint, NetState, Security, WatchMsg};
#[cfg(test)]
use opusdm_network::VpnEntry;

/// Signals below this are drawn with one arc.
const LOW_BELOW: u8 = 34;
/// Signals below this are drawn with two arcs.
const MEDIUM_BELOW: u8 = 67;

/// Which symbol the panel icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconState {
    Ethernet,
    /// Wi-Fi with 1 to 3 arcs.
    Wifi(u8),
    Disconnected,
}

/// The state and whether NetworkManager is reachable.
#[derive(Default)]
pub struct Model {
    state: NetState,
    available: bool,
}

impl Model {
    /// Apply one message of the watcher.
    pub fn apply(&mut self, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(_) => {}
            WatchMsg::Unavailable(_) => {
                self.state = NetState::default();
                self.available = false;
            }
            WatchMsg::State(state) => {
                self.state = state;
                self.available = true;
            }
        }
    }

    /// Whether NetworkManager is reachable.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// The current state.
    pub fn state(&self) -> &NetState {
        &self.state
    }

    /// The icon symbol: Ethernet when it is connected, else the Wi-Fi signal.
    pub fn icon_state(&self) -> IconState {
        if self.state.eth_connected { return IconState::Ethernet; }
        match self.state.active_ap() {
            Some(ap) => IconState::Wifi(signal_arcs(ap.signal)),
            None => IconState::Disconnected,
        }
    }

    /// Whether a VPN is up: the icon gets a lock.
    pub fn vpn_active(&self) -> bool {
        self.state.active_vpn().is_some()
    }

    /// The icon tooltip: what is connected, or why nothing is, and the VPN.
    pub fn tooltip(&self) -> String {
        if !self.available { return "No network manager".to_string(); }
        let link = if self.state.eth_connected {
            "Ethernet connected".to_string()
        } else {
            match self.state.active_ap() {
                Some(ap) => format!("{} ({}%)", ap.ssid, ap.signal),
                None => "Not connected".to_string(),
            }
        };
        match self.state.active_vpn() {
            Some(vpn) => format!("{link}\nVPN: {}", vpn.name),
            None => link,
        }
    }
}

/// How many arcs (1 to 3) a signal strength is drawn with.
pub fn signal_arcs(signal: u8) -> u8 {
    match signal {
        s if s < LOW_BELOW => 1,
        s if s < MEDIUM_BELOW => 2,
        _ => 3,
    }
}

/// What the user asked for. The `Set` prefix mirrors the `Backend` methods.
#[derive(Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Request {
    SetWifi(bool),
    SetEth(bool),
    Scan,
    Connect { ssid: String, password: Option<String> },
    DisconnectWifi,
    /// Bring a VPN up; `password` is asked from the user for this connect only.
    VpnUp { uuid: String, password: Option<String> },
    VpnDown(String),
}

// The password must never reach a log through `{:?}`.
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::SetWifi(on) => write!(f, "SetWifi({on})"),
            Request::SetEth(on) => write!(f, "SetEth({on})"),
            Request::Scan => write!(f, "Scan"),
            Request::Connect { ssid, password } => write!(f, "Connect({ssid}, password: {})", password.is_some()),
            Request::DisconnectWifi => write!(f, "DisconnectWifi"),
            Request::VpnUp { uuid, password } => write!(f, "VpnUp({uuid}, password: {})", password.is_some()),
            Request::VpnDown(uuid) => write!(f, "VpnDown({uuid})"),
        }
    }
}

/// Drop every switch request that a later one for the same switch replaces,
/// and repeated scans. The order of everything else is kept.
pub fn coalesce(requests: Vec<Request>) -> Vec<Request> {
    let mut out = Vec::new();
    for (i, r) in requests.iter().enumerate() {
        let later = &requests[i + 1..];
        let replaced = match r {
            Request::SetWifi(_) => later.iter().any(|n| matches!(n, Request::SetWifi(_))),
            Request::SetEth(_) => later.iter().any(|n| matches!(n, Request::SetEth(_))),
            Request::Scan => later.iter().any(|n| matches!(n, Request::Scan)),
            _ => false,
        };
        if !replaced { out.push(r.clone()); }
    }
    out
}

/// What a click on a network row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowAction {
    /// The network in use: disconnect.
    Disconnect,
    /// A saved or open network: connect at once.
    Connect,
    /// A new protected network: ask for the password first.
    AskPassword,
    /// A network this tray cannot join, with the reason to show.
    Unsupported(&'static str),
}

/// The action of a click on `ap`.
pub fn row_action(ap: &AccessPoint) -> RowAction {
    if ap.active { return RowAction::Disconnect; }
    if !ap.security.is_supported() { return RowAction::Unsupported("This kind of network is not supported"); }
    if ap.saved || !ap.security.needs_password() { return RowAction::Connect; }
    RowAction::AskPassword
}

/// Whether a secured network is drawn with a lock.
pub fn is_locked(security: Security) -> bool {
    security != Security::Open
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap(ssid: &str, signal: u8, security: Security, active: bool, saved: bool) -> AccessPoint {
        AccessPoint { ssid: ssid.to_string(), signal, security, active, saved }
    }

    fn state(eth_connected: bool, aps: Vec<AccessPoint>) -> NetState {
        NetState { wifi_present: true, wifi_enabled: true, eth_present: true, eth_available: true, eth_on: eth_connected, eth_connected, aps, vpns: Vec::new(), ..NetState::default() }
    }

    #[test]
    fn nothing_is_available_before_the_first_state() {
        let m = Model::default();
        assert!(!m.is_available());
        assert_eq!(m.tooltip(), "No network manager");
    }

    #[test]
    fn unavailable_clears_the_state() {
        let mut m = Model::default();
        m.apply(WatchMsg::State(state(true, vec![])));
        assert!(m.is_available());
        m.apply(WatchMsg::Unavailable("gone".into()));
        assert!(!m.is_available());
        assert!(!m.state().eth_connected);
    }

    #[test]
    fn ethernet_wins_over_wifi_for_the_icon() {
        let mut m = Model::default();
        m.apply(WatchMsg::State(state(true, vec![ap("home", 80, Security::Psk, true, true)])));
        assert_eq!(m.icon_state(), IconState::Ethernet);
        assert_eq!(m.tooltip(), "Ethernet connected");
    }

    #[test]
    fn wifi_icon_follows_the_signal_of_the_active_network() {
        let mut m = Model::default();
        m.apply(WatchMsg::State(state(false, vec![ap("home", 50, Security::Psk, true, true), ap("x", 99, Security::Open, false, false)])));
        assert_eq!(m.icon_state(), IconState::Wifi(2));
        assert_eq!(m.tooltip(), "home (50%)");
    }

    #[test]
    fn no_connection_is_disconnected() {
        let mut m = Model::default();
        m.apply(WatchMsg::State(state(false, vec![ap("x", 99, Security::Open, false, false)])));
        assert_eq!(m.icon_state(), IconState::Disconnected);
        assert_eq!(m.tooltip(), "Not connected");
    }

    #[test]
    fn arcs_thresholds() {
        assert_eq!(signal_arcs(0), 1);
        assert_eq!(signal_arcs(33), 1);
        assert_eq!(signal_arcs(34), 2);
        assert_eq!(signal_arcs(66), 2);
        assert_eq!(signal_arcs(67), 3);
        assert_eq!(signal_arcs(100), 3);
    }

    #[test]
    fn the_last_switch_value_wins() {
        let batch = vec![Request::SetWifi(false), Request::Scan, Request::SetEth(true), Request::SetWifi(true), Request::Scan, Request::SetEth(false)];
        assert_eq!(coalesce(batch), vec![Request::SetWifi(true), Request::Scan, Request::SetEth(false)]);
    }

    #[test]
    fn connects_are_never_merged() {
        let c = |s: &str| Request::Connect { ssid: s.to_string(), password: None };
        assert_eq!(coalesce(vec![c("a"), c("b"), Request::DisconnectWifi]), vec![c("a"), c("b"), Request::DisconnectWifi]);
    }

    #[test]
    fn debug_never_shows_the_password() {
        let r = Request::Connect { ssid: "home".into(), password: Some("hunter2".into()) };
        let text = format!("{r:?}");
        assert!(!text.contains("hunter2"));
        assert!(text.contains("home"));
    }

    #[test]
    fn click_actions() {
        assert_eq!(row_action(&ap("a", 50, Security::Psk, true, true)), RowAction::Disconnect);
        assert_eq!(row_action(&ap("a", 50, Security::Psk, false, true)), RowAction::Connect);
        assert_eq!(row_action(&ap("a", 50, Security::Open, false, false)), RowAction::Connect);
        assert_eq!(row_action(&ap("a", 50, Security::Psk, false, false)), RowAction::AskPassword);
        assert_eq!(row_action(&ap("a", 50, Security::Sae, false, false)), RowAction::AskPassword);
        assert!(matches!(row_action(&ap("a", 50, Security::Enterprise, false, true)), RowAction::Unsupported(_)));
        assert!(matches!(row_action(&ap("a", 50, Security::Wep, false, false)), RowAction::Unsupported(_)));
    }

    fn vpn(name: &str, active: bool) -> VpnEntry {
        VpnEntry { uuid: name.to_string(), name: name.to_string(), active, activating: false, ask_password: false }
    }

    #[test]
    fn an_active_vpn_is_in_the_tooltip_and_locks_the_icon() {
        let mut m = Model::default();
        let mut s = state(true, vec![]);
        s.vpns = vec![vpn("home", false), vpn("work", true)];
        m.apply(WatchMsg::State(s));
        assert!(m.vpn_active());
        assert_eq!(m.tooltip(), "Ethernet connected\nVPN: work");
        let mut s = state(true, vec![]);
        s.vpns = vec![vpn("home", false)];
        m.apply(WatchMsg::State(s));
        assert!(!m.vpn_active());
        assert_eq!(m.tooltip(), "Ethernet connected");
    }

    #[test]
    fn vpn_requests_are_never_merged_and_hide_the_password() {
        let up = Request::VpnUp { uuid: "u".into(), password: Some("hunter2".into()) };
        assert!(!format!("{up:?}").contains("hunter2"));
        let batch = vec![up.clone(), Request::VpnDown("u".into()), up.clone()];
        assert_eq!(coalesce(batch.clone()), batch);
    }

    #[test]
    fn lock_only_for_protected_networks() {
        assert!(!is_locked(Security::Open));
        assert!(is_locked(Security::Psk));
        assert!(is_locked(Security::Enterprise));
    }
}
