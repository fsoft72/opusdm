//! Turns NetworkManager properties into the model, with no bus access.

use crate::model::{AccessPoint, Security};
use std::collections::{HashMap, HashSet};
use zbus::zvariant::OwnedValue;

/// `NM_802_11_AP_FLAGS_PRIVACY`: the network needs some kind of key.
const FLAG_PRIVACY: u32 = 0x1;
/// `NM_802_11_AP_SEC_KEY_MGMT_PSK`.
const KEY_MGMT_PSK: u32 = 0x100;
/// `NM_802_11_AP_SEC_KEY_MGMT_802_1X`.
const KEY_MGMT_8021X: u32 = 0x200;
/// `NM_802_11_AP_SEC_KEY_MGMT_SAE`.
const KEY_MGMT_SAE: u32 = 0x400;

/// `NM_DEVICE_STATE_UNAVAILABLE`: no cable, or the radio is off.
const STATE_UNAVAILABLE: u32 = 20;
/// First state of a device that is being connected.
const STATE_PREPARE: u32 = 40;
/// `NM_DEVICE_STATE_ACTIVATED`.
const STATE_ACTIVATED: u32 = 100;

/// `NM_DEVICE_TYPE_ETHERNET`.
pub const DEVICE_ETHERNET: u32 = 1;
/// `NM_DEVICE_TYPE_WIFI`.
pub const DEVICE_WIFI: u32 = 2;

type Props = HashMap<String, OwnedValue>;

/// The facts of one access point, as NetworkManager reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawAp {
    /// D-Bus object path, to tell which one is the active access point.
    pub path: String,
    pub ssid: Vec<u8>,
    pub strength: u8,
    pub flags: u32,
    pub wpa_flags: u32,
    pub rsn_flags: u32,
}

impl RawAp {
    /// Read the `GetAll` reply of an `AccessPoint` object.
    pub fn from_props(path: &str, props: &Props) -> Self {
        let get_u32 = |k: &str| props.get(k).and_then(|v| u32::try_from(v.clone()).ok()).unwrap_or(0);
        Self {
            path: path.to_string(),
            ssid: props.get("Ssid").and_then(|v| Vec::<u8>::try_from(v.clone()).ok()).unwrap_or_default(),
            strength: props.get("Strength").and_then(|v| u8::try_from(v.clone()).ok()).unwrap_or(0),
            flags: get_u32("Flags"),
            wpa_flags: get_u32("WpaFlags"),
            rsn_flags: get_u32("RsnFlags"),
        }
    }

    /// How the network is protected.
    pub fn security(&self) -> Security {
        security(self.flags, self.wpa_flags, self.rsn_flags)
    }
}

/// The protection of a network from its three flag words.
pub fn security(flags: u32, wpa: u32, rsn: u32) -> Security {
    let keys = wpa | rsn;
    if keys & KEY_MGMT_8021X != 0 { return Security::Enterprise; }
    if keys & KEY_MGMT_PSK != 0 { return Security::Psk; }
    if keys & KEY_MGMT_SAE != 0 { return Security::Sae; }
    if flags & FLAG_PRIVACY != 0 { return Security::Wep; }
    Security::Open
}

/// The networks to list: hidden names dropped, access points with one name
/// merged (the best signal wins, the active one always wins), the active
/// network first and then by signal. `saved` holds the names that have a profile.
pub fn build_aps(raws: &[RawAp], active_path: Option<&str>, saved: &HashSet<String>) -> Vec<AccessPoint> {
    let mut merged: Vec<AccessPoint> = Vec::new();
    for raw in raws {
        let ssid = String::from_utf8_lossy(&raw.ssid).into_owned();
        if ssid.trim().is_empty() { continue; }
        let active = active_path == Some(raw.path.as_str());
        let ap = AccessPoint { saved: saved.contains(&ssid), ssid, signal: raw.strength.min(100), security: raw.security(), active };
        match merged.iter_mut().find(|m| m.ssid == ap.ssid) {
            None => merged.push(ap),
            Some(m) => {
                let wins = ap.active || (!m.active && ap.signal > m.signal);
                if wins {
                    let active = m.active || ap.active;
                    *m = AccessPoint { active, ..ap };
                }
            }
        }
    }
    merged.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)).then(a.ssid.cmp(&b.ssid)));
    merged
}

/// The Ethernet flags `(available, on, connected)` of a device state.
pub fn eth_flags(state: u32) -> (bool, bool, bool) {
    let available = state > STATE_UNAVAILABLE;
    let on = (STATE_PREPARE..=STATE_ACTIVATED).contains(&state);
    (available, on, state == STATE_ACTIVATED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn raw(path: &str, ssid: &str, strength: u8, rsn: u32) -> RawAp {
        RawAp { path: path.to_string(), ssid: ssid.as_bytes().to_vec(), strength, flags: 1, wpa_flags: 0, rsn_flags: rsn }
    }

    #[test]
    fn security_from_flags() {
        assert_eq!(security(0, 0, 0), Security::Open);
        assert_eq!(security(1, 0, 0), Security::Wep);
        assert_eq!(security(1, 0, KEY_MGMT_PSK), Security::Psk);
        assert_eq!(security(1, KEY_MGMT_PSK, 0), Security::Psk);
        assert_eq!(security(1, 0, KEY_MGMT_SAE), Security::Sae);
        assert_eq!(security(1, 0, KEY_MGMT_SAE | KEY_MGMT_PSK), Security::Psk);
        assert_eq!(security(1, 0, KEY_MGMT_8021X | KEY_MGMT_PSK), Security::Enterprise);
        assert!(Security::Psk.needs_password());
        assert!(!Security::Open.needs_password());
        assert!(!Security::Enterprise.is_supported());
        assert!(!Security::Wep.is_supported());
    }

    #[test]
    fn hidden_names_are_dropped() {
        let aps = build_aps(&[raw("/1", "", 80, 0), raw("/2", "  ", 80, 0), raw("/3", "home", 50, KEY_MGMT_PSK)], None, &HashSet::new());
        assert_eq!(aps.len(), 1);
        assert_eq!(aps[0].ssid, "home");
    }

    #[test]
    fn duplicates_keep_the_best_signal() {
        let aps = build_aps(&[raw("/1", "home", 40, KEY_MGMT_PSK), raw("/2", "home", 70, KEY_MGMT_PSK)], None, &HashSet::new());
        assert_eq!(aps.len(), 1);
        assert_eq!(aps[0].signal, 70);
        assert!(!aps[0].active);
    }

    #[test]
    fn the_active_access_point_wins_even_with_a_weaker_signal() {
        let raws = [raw("/1", "home", 90, KEY_MGMT_PSK), raw("/2", "home", 30, KEY_MGMT_PSK)];
        let aps = build_aps(&raws, Some("/2"), &HashSet::new());
        assert_eq!(aps.len(), 1);
        assert!(aps[0].active);
        // The same, with the active one seen first.
        let aps = build_aps(&[raws[1].clone(), raws[0].clone()], Some("/2"), &HashSet::new());
        assert!(aps[0].active);
    }

    #[test]
    fn order_is_active_first_then_signal_then_name() {
        let raws = [raw("/1", "b", 60, 0), raw("/2", "a", 60, 0), raw("/3", "c", 90, 0), raw("/4", "weak", 10, 0)];
        let aps = build_aps(&raws, Some("/4"), &HashSet::new());
        let names: Vec<&str> = aps.iter().map(|a| a.ssid.as_str()).collect();
        assert_eq!(names, vec!["weak", "c", "a", "b"]);
    }

    #[test]
    fn saved_names_are_marked() {
        let saved: HashSet<String> = ["home".to_string()].into();
        let aps = build_aps(&[raw("/1", "home", 50, 0), raw("/2", "cafe", 50, 0)], None, &saved);
        assert!(aps.iter().find(|a| a.ssid == "home").unwrap().saved);
        assert!(!aps.iter().find(|a| a.ssid == "cafe").unwrap().saved);
    }

    #[test]
    fn eth_flags_follow_the_device_state() {
        assert_eq!(eth_flags(20), (false, false, false));
        assert_eq!(eth_flags(30), (true, false, false));
        assert_eq!(eth_flags(50), (true, true, false));
        assert_eq!(eth_flags(100), (true, true, true));
    }

    #[test]
    fn raw_ap_reads_the_property_map() {
        let mut props: Props = HashMap::new();
        props.insert("Ssid".into(), OwnedValue::try_from(Value::from(b"home".to_vec())).unwrap());
        props.insert("Strength".into(), OwnedValue::try_from(Value::from(72u8)).unwrap());
        props.insert("Flags".into(), OwnedValue::try_from(Value::from(1u32)).unwrap());
        props.insert("RsnFlags".into(), OwnedValue::try_from(Value::from(KEY_MGMT_PSK)).unwrap());
        let ap = RawAp::from_props("/ap/1", &props);
        assert_eq!(ap.ssid, b"home");
        assert_eq!(ap.strength, 72);
        assert_eq!(ap.security(), Security::Psk);
        assert_eq!(ap.wpa_flags, 0);
    }
}
