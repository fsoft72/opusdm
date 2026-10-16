//! The real backend: NetworkManager over the D-Bus system bus.

use crate::agent;
use crate::backend::Backend;
use crate::error::NetError;
use crate::keystore::{psk_key, KeyStore, OneShotSecrets, SecretServiceStore, SECURITY_GROUP};
use crate::import;
use crate::model::{NetState, Security, VpnEntry};
use crate::parse::{self, RawAp, DEVICE_ETHERNET, DEVICE_WIFI};
use crate::profile::{Profile, ProfileKind, SecretKey};
use crate::profiles::{Loaded, ProfileBackend, ProfileSummary};
use crate::settings::{self as nmsettings, Settings};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const SERVICE: &str = "org.freedesktop.NetworkManager";
const ROOT_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION_IFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const ACTIVE_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const NO_OBJECT: &str = "/";

/// `NM_ACTIVE_CONNECTION_STATE_ACTIVATING`.
const ACTIVE_ACTIVATING: u32 = 1;
/// `NM_ACTIVE_CONNECTION_STATE_ACTIVATED`.
const ACTIVE_ACTIVATED: u32 = 2;
/// `NM_ACTIVE_CONNECTION_STATE_DEACTIVATED`.
const ACTIVE_DEACTIVATED: u32 = 4;
/// `NM_SETTING_SECRET_FLAG_AGENT_OWNED | NM_SETTING_SECRET_FLAG_NOT_SAVED`: the
/// key is not in the profile, a secret agent has to provide it.
const SECRET_NOT_STORED: u32 = SECRET_AGENT_OWNED | 0x2;
/// `NM_SETTING_SECRET_FLAG_AGENT_OWNED`: the key lives with the secret agent.
const SECRET_AGENT_OWNED: u32 = 0x1;
/// How long a connection may take to come up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the state of a connecting connection is read.
const CONNECT_POLL: Duration = Duration::from_millis(250);

type Props = HashMap<String, OwnedValue>;
/// `a{sv}` with no entries: the options of the calls that take none.
type NoOptions<'a> = HashMap<&'a str, Value<'a>>;

/// NetworkManager on the system bus.
pub struct NmBackend {
    conn: Connection,
    /// Where the keys of the profiles that keep them out of NetworkManager live.
    keys: Arc<dyn KeyStore>,
    /// Secrets given for one connect (a VPN password asked every time).
    once: Arc<OneShotSecrets>,
}

impl NmBackend {
    /// Connect to the system bus. Does not need NetworkManager to be running yet.
    pub fn connect() -> Result<Self, NetError> {
        let conn = Connection::system()?;
        Ok(Self { conn, keys: Arc::new(SecretServiceStore), once: Arc::new(OneShotSecrets::default()) })
    }

    /// Serve the secret agent and register it under `identifier`;
    /// NetworkManager must be running. Without it a profile that keeps its
    /// key out of NetworkManager cannot join.
    pub fn register_agent(&self, identifier: &str) -> Result<(), NetError> {
        agent::register(&self.conn, agent::SecretSources { keys: self.keys.clone(), once: self.once.clone() }, identifier)
    }

    /// The connection, for the signal watcher.
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    /// A proxy for `iface` on object `path`.
    fn proxy(&self, path: &str, iface: &'static str) -> Result<Proxy<'static>, NetError> {
        Proxy::new(&self.conn, SERVICE, path.to_string(), iface).map_err(NetError::from)
    }

    /// Every device with its type and state.
    fn devices(&self) -> Result<Vec<Device>, NetError> {
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        let paths: Vec<OwnedObjectPath> = nm.call("GetDevices", &())?;
        let mut out = Vec::new();
        for path in paths {
            let dev = self.proxy(path.as_str(), DEVICE_IFACE)?;
            let kind: u32 = dev.get_property("DeviceType")?;
            let state: u32 = dev.get_property("State")?;
            out.push(Device { path, kind, state });
        }
        Ok(out)
    }

    /// The first device of `kind`, or an error that names what is missing.
    fn device_of(&self, kind: u32, what: &str) -> Result<Device, NetError> {
        self.devices()?.into_iter().find(|d| d.kind == kind).ok_or_else(|| NetError::other(format!("No {what} device")))
    }

    /// `GetAll` of one interface of one object.
    fn get_all(&self, path: &str, iface: &str) -> Result<Props, NetError> {
        let props = self.proxy(path, PROPERTIES_IFACE)?;
        props.call("GetAll", &(iface,)).map_err(NetError::from)
    }

    /// Every access point the Wi-Fi device sees.
    fn read_aps(&self, wifi: &Device) -> Result<Vec<RawAp>, NetError> {
        let proxy = self.proxy(wifi.path.as_str(), WIRELESS_IFACE)?;
        let paths: Vec<OwnedObjectPath> = proxy.call("GetAllAccessPoints", &())?;
        let mut out = Vec::new();
        for path in paths {
            // An access point can vanish between the two calls.
            let Ok(props) = self.get_all(path.as_str(), AP_IFACE) else { continue };
            out.push(RawAp::from_props(path.as_str(), &props));
        }
        Ok(out)
    }

    /// Every profile: object path and settings (without secrets). A profile
    /// removed between the two calls is skipped.
    fn all_settings(&self) -> Result<Vec<(OwnedObjectPath, Settings)>, NetError> {
        let proxy = self.proxy(SETTINGS_PATH, SETTINGS_IFACE)?;
        let paths: Vec<OwnedObjectPath> = proxy.call("ListConnections", &())?;
        let mut out = Vec::new();
        for path in paths {
            let Ok(conn) = self.proxy(path.as_str(), CONNECTION_IFACE) else { continue };
            let Ok(settings) = conn.call::<_, _, Settings>("GetSettings", &()) else { continue };
            out.push((path, settings));
        }
        Ok(out)
    }

    /// The Wi-Fi profiles among `all`: object path and network name.
    fn wifi_profiles(all: &[(OwnedObjectPath, Settings)]) -> Vec<(OwnedObjectPath, String)> {
        all.iter()
            .filter_map(|(path, s)| Some((path.clone(), String::from_utf8_lossy(&nmsettings::bytes(s, "802-11-wireless", "ssid")?).into_owned())))
            .collect()
    }

    /// The Wi-Fi profiles: object path and network name.
    fn saved_profiles(&self) -> Result<Vec<(OwnedObjectPath, String)>, NetError> {
        Ok(Self::wifi_profiles(&self.all_settings()?))
    }

    /// The active connections: object path, profile UUID and state.
    fn active_connections(&self) -> Result<Vec<(OwnedObjectPath, String, u32)>, NetError> {
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        let paths: Vec<OwnedObjectPath> = nm.get_property("ActiveConnections")?;
        let mut out = Vec::new();
        for path in paths {
            // An active connection can vanish between the calls.
            let Ok(active) = self.proxy(path.as_str(), ACTIVE_IFACE) else { continue };
            let Ok(uuid) = active.get_property::<String>("Uuid") else { continue };
            let state = active.get_property::<u32>("State").unwrap_or(ACTIVE_DEACTIVATED);
            out.push((path, uuid, state));
        }
        Ok(out)
    }

    /// The summaries of `all`, marked with the active connections.
    fn summaries(all: &[(OwnedObjectPath, Settings)], active: &[(OwnedObjectPath, String, u32)]) -> Vec<ProfileSummary> {
        let mut out: Vec<ProfileSummary> = all
            .iter()
            .filter_map(|(_, s)| Profile::from_settings(s).ok())
            .map(|p| {
                let state = active.iter().find(|(_, u, _)| *u == p.uuid).map(|(_, _, st)| *st);
                ProfileSummary { active: state == Some(ACTIVE_ACTIVATED), activating: state == Some(ACTIVE_ACTIVATING), ..ProfileSummary::of(&p) }
            })
            .collect();
        out.sort_by_key(|p| p.name.to_lowercase());
        out
    }

    /// The object path of the profile `uuid`.
    fn profile_path(&self, uuid: &str) -> Result<OwnedObjectPath, NetError> {
        let proxy = self.proxy(SETTINGS_PATH, SETTINGS_IFACE)?;
        proxy.call("GetConnectionByUuid", &(uuid,)).map_err(NetError::from)
    }

    /// The secrets NetworkManager keeps itself for one setting of a profile
    /// (flag 0). Asking may fail for a setting with no secrets, which only
    /// means there are none, so a failure reads as an empty answer.
    fn stored_secrets(&self, path: &OwnedObjectPath, setting: &str) -> Settings {
        let Ok(conn) = self.proxy(path.as_str(), CONNECTION_IFACE) else { return Settings::new() };
        conn.call::<_, _, Settings>("GetSecrets", &(setting,)).unwrap_or_default()
    }

    /// Wait until the active connection is up. Returns the failure text otherwise.
    fn wait_active(&self, active: &OwnedObjectPath) -> Result<(), String> {
        let started = Instant::now();
        loop {
            // The object is removed when the attempt fails, so a read error means it failed.
            let state = self
                .proxy(active.as_str(), ACTIVE_IFACE)
                .and_then(|p| p.get_property::<u32>("State").map_err(NetError::from))
                .unwrap_or(ACTIVE_DEACTIVATED);
            if state == ACTIVE_ACTIVATED { return Ok(()); }
            if state == ACTIVE_DEACTIVATED { return Err("Could not connect".to_string()); }
            if started.elapsed() > CONNECT_TIMEOUT { return Err("Connection timed out".to_string()); }
            std::thread::sleep(CONNECT_POLL);
        }
    }

    /// Make sure a saved profile can be activated: the secret agent must find
    /// its key.
    ///
    /// A profile whose key NetworkManager does not keep needs one in the
    /// keyring. Without a `password` and without a key there, this asks the
    /// caller for one. With a `password` it goes into the keyring. Returns the
    /// UUID to forget if the connection then fails, `None` when nothing was kept.
    fn ensure_key(&self, profile: &OwnedObjectPath, password: Option<&str>) -> Result<Option<String>, NetError> {
        let conn = self.proxy(profile.as_str(), CONNECTION_IFACE)?;
        let settings: Settings = conn.call("GetSettings", &())?;
        if !key_not_stored(&settings) { return Ok(None); }
        let uuid = text_of(&settings, "connection", "uuid").ok_or_else(|| NetError::other("The profile has no uuid"))?;
        let Some(password) = password else {
            return match self.keys.get(&uuid, &psk_key())? {
                Some(_) => Ok(None),
                None => Err(NetError::password_needed()),
            };
        };
        let name = text_of(&settings, "connection", "id").unwrap_or_else(|| uuid.clone());
        self.keys.set(&uuid, &name, &psk_key(), password)?;
        Ok(Some(uuid))
    }

    /// Remove a profile, ignoring the error: it only cleans up after a failure.
    fn delete_profile(&self, path: &OwnedObjectPath) {
        let Ok(conn) = self.proxy(path.as_str(), CONNECTION_IFACE) else { return };
        let _ = conn.call::<_, _, ()>("Delete", &());
    }
}

/// A network device.
struct Device {
    path: OwnedObjectPath,
    kind: u32,
    state: u32,
}

/// Whether the profile has a key that is not stored in it.
fn key_not_stored(settings: &Settings) -> bool {
    let flags = settings.get(SECURITY_GROUP).and_then(|g| g.get("psk-flags")).and_then(|v| u32::try_from(v.clone()).ok());
    flags.is_some_and(|f| f & SECRET_NOT_STORED != 0)
}

/// One text value of a profile.
fn text_of(settings: &Settings, group: &str, key: &str) -> Option<String> {
    String::try_from(settings.get(group)?.get(key)?.clone()).ok()
}

/// The settings of a new profile: the name, the network and the key.
fn new_profile<'a>(ssid: &'a str, key: Option<(&'static str, &'a str)>) -> HashMap<&'static str, HashMap<&'static str, Value<'a>>> {
    let mut s: HashMap<&'static str, HashMap<&'static str, Value<'a>>> = HashMap::new();
    let Some((key_mgmt, psk)) = key else { return s };
    s.insert("connection", HashMap::from([("type", Value::from("802-11-wireless")), ("id", Value::from(ssid))]));
    s.insert("802-11-wireless", HashMap::from([("ssid", Value::from(ssid.as_bytes().to_vec())), ("mode", Value::from("infrastructure"))]));
    s.insert("802-11-wireless-security", HashMap::from([("key-mgmt", Value::from(key_mgmt)), ("psk", Value::from(psk)), ("psk-flags", Value::from(SECRET_AGENT_OWNED))]));
    s
}

impl Backend for NmBackend {
    fn state(&self) -> Result<NetState, NetError> {
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        let wifi_enabled: bool = nm.get_property("WirelessEnabled")?;
        let devices = self.devices()?;
        let all = self.all_settings()?;
        let vpns = vpn_entries(&Self::summaries(&all, &self.active_connections()?));
        let mut state = NetState { wifi_enabled, vpns, ..NetState::default() };
        if let Some(eth) = devices.iter().find(|d| d.kind == DEVICE_ETHERNET) {
            let (available, on, connected) = parse::eth_flags(eth.state);
            state.eth_present = true;
            state.eth_available = available;
            state.eth_on = on;
            state.eth_connected = connected;
        }
        let Some(wifi) = devices.iter().find(|d| d.kind == DEVICE_WIFI) else { return Ok(state) };
        state.wifi_present = true;
        if !wifi_enabled { return Ok(state); }
        let raws = self.read_aps(wifi)?;
        let active: OwnedObjectPath = self
            .proxy(wifi.path.as_str(), WIRELESS_IFACE)?
            .get_property("ActiveAccessPoint")
            ?;
        let active = (active.as_str() != NO_OBJECT).then(|| active.to_string());
        let saved: HashSet<String> = Self::wifi_profiles(&all).into_iter().map(|(_, ssid)| ssid).collect();
        state.aps = parse::build_aps(&raws, active.as_deref(), &saved);
        Ok(state)
    }

    fn set_wifi_enabled(&self, on: bool) -> Result<(), NetError> {
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        nm.set_property("WirelessEnabled", on).map_err(NetError::from)
    }

    fn set_eth_enabled(&self, on: bool) -> Result<(), NetError> {
        let eth = self.device_of(DEVICE_ETHERNET, "Ethernet")?;
        if !on {
            let dev = self.proxy(eth.path.as_str(), DEVICE_IFACE)?;
            return dev.call::<_, _, ()>("Disconnect", &()).map_err(NetError::from);
        }
        // "/" for the profile and the access point lets NetworkManager choose.
        let none = ObjectPath::try_from(NO_OBJECT).map_err(|e| NetError::other(e.to_string()))?;
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        nm.call::<_, _, OwnedObjectPath>("ActivateConnection", &(&none, &eth.path, &none))
            .map(|_| ())
            .map_err(NetError::from)
    }

    fn scan(&self) -> Result<(), NetError> {
        let wifi = self.device_of(DEVICE_WIFI, "Wi-Fi")?;
        let dev = self.proxy(wifi.path.as_str(), WIRELESS_IFACE)?;
        dev.call::<_, _, ()>("RequestScan", &(NoOptions::new(),)).map_err(NetError::from)
    }

    fn connect(&self, ssid: &str, password: Option<&str>) -> Result<(), NetError> {
        let wifi = self.device_of(DEVICE_WIFI, "Wi-Fi")?;
        let raws = self.read_aps(&wifi)?;
        let target = raws
            .iter()
            .filter(|r| r.ssid == ssid.as_bytes())
            .max_by_key(|r| r.strength)
            .ok_or_else(|| NetError::other(format!("{ssid} is out of range")))?;
        let security = target.security();
        if !security.is_supported() { return Err(NetError::other("This kind of network is not supported")); }
        let ap = ObjectPath::try_from(target.path.as_str()).map_err(|e| NetError::other(e.to_string()))?;
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        let profile = self.saved_profiles()?.into_iter().find(|(_, name)| name == ssid).map(|(p, _)| p);
        if let Some(profile) = profile {
            let kept = self.ensure_key(&profile, password)?;
            let active: OwnedObjectPath = nm
                .call("ActivateConnection", &(&profile, &wifi.path, &ap))
                ?;
            return self.wait_active(&active).map_err(|why| {
                // A key that did not work must not stay in the keyring.
                if let Some(uuid) = &kept {
                    let _ = self.keys.delete(uuid, &psk_key());
                }
                NetError::other(if kept.is_some() { format!("{why}, wrong password?") } else { why })
            });
        }
        let key = match (security, password) {
            (Security::Open, _) => None,
            (_, None) => return Err(NetError::password_needed()),
            (Security::Sae, Some(p)) => Some(("sae", p)),
            (_, Some(p)) => Some(("wpa-psk", p)),
        };
        let (created, active): (OwnedObjectPath, OwnedObjectPath) = nm
            .call("AddAndActivateConnection", &(new_profile(ssid, key), &wifi.path, &ap))
            ?;
        self.wait_active(&active).map_err(|why| {
            // A profile that never worked must not stay, or it would be
            // tried again with the same wrong password.
            self.delete_profile(&created);
            NetError::other(if key.is_some() { format!("{why}, wrong password?") } else { why })
        })
    }

    fn disconnect_wifi(&self) -> Result<(), NetError> {
        let wifi = self.device_of(DEVICE_WIFI, "Wi-Fi")?;
        let dev = self.proxy(wifi.path.as_str(), DEVICE_IFACE)?;
        dev.call::<_, _, ()>("Disconnect", &()).map_err(NetError::from)
    }

    fn activate_vpn(&self, uuid: &str, password: Option<&str>) -> Result<(), NetError> {
        ProfileBackend::activate(self, uuid, password)
    }

    fn deactivate_vpn(&self, uuid: &str) -> Result<(), NetError> {
        ProfileBackend::deactivate(self, uuid)
    }
}

/// The VPN rows of the panel, from the profile summaries.
fn vpn_entries(summaries: &[ProfileSummary]) -> Vec<VpnEntry> {
    summaries
        .iter()
        .filter(|p| p.kind.is_vpn())
        .map(|p| VpnEntry { uuid: p.uuid.clone(), name: p.name.clone(), active: p.active, activating: p.activating, ask_password: p.ask_password })
        .collect()
}

/// The setting that holds the secrets of `profile`, if it has any.
fn secret_setting(profile: &Profile) -> Option<&'static str> {
    match profile.kind() {
        ProfileKind::Wifi => Some(SECURITY_GROUP),
        ProfileKind::OpenVpn => Some("vpn"),
        ProfileKind::WireGuard => Some(crate::profile::TYPE_WIREGUARD),
        ProfileKind::Ethernet | ProfileKind::Other => None,
    }
}

impl ProfileBackend for NmBackend {
    fn list(&self) -> Result<Vec<ProfileSummary>, NetError> {
        Ok(Self::summaries(&self.all_settings()?, &self.active_connections()?))
    }

    fn load(&self, uuid: &str) -> Result<Loaded, NetError> {
        let path = self.profile_path(uuid)?;
        let conn = self.proxy(path.as_str(), CONNECTION_IFACE)?;
        let base: Settings = conn.call("GetSettings", &())?;
        let mut profile = Profile::from_settings(&base)?;
        if let Some(setting) = secret_setting(&profile) {
            profile.fill_secrets(&self.stored_secrets(&path, setting));
        }
        // A keyring error stops the load: saving a profile whose secrets
        // could not be read would forget them.
        let missing: Vec<SecretKey> = profile.secret_slots().into_iter().filter(|(_, v)| v.is_empty()).map(|(k, _)| k).collect();
        for key in missing {
            if let Some(found) = self.keys.get(uuid, &key)? {
                profile.set_secret(&key, &found);
            }
        }
        Ok(Loaded { profile, base })
    }

    fn save(&self, profile: &Profile, base: Option<&Settings>) -> Result<(), NetError> {
        let settings = profile.to_settings(base.unwrap_or(&Settings::new()));
        match base {
            None => {
                let proxy = self.proxy(SETTINGS_PATH, SETTINGS_IFACE)?;
                proxy.call::<_, _, OwnedObjectPath>("AddConnection", &(settings,))?;
            }
            Some(_) => {
                let conn = self.proxy(self.profile_path(&profile.uuid)?.as_str(), CONNECTION_IFACE)?;
                conn.call::<_, _, ()>("Update", &(settings,))?;
            }
        }
        if !profile.manages_secrets() { return Ok(()); }
        self.keys.delete_all(&profile.uuid)?;
        for (key, value) in profile.secrets() {
            self.keys.set(&profile.uuid, &profile.id, &key, value.as_str())?;
        }
        Ok(())
    }

    fn delete(&self, uuid: &str) -> Result<(), NetError> {
        let conn = self.proxy(self.profile_path(uuid)?.as_str(), CONNECTION_IFACE)?;
        conn.call::<_, _, ()>("Delete", &())?;
        self.keys.delete_all(uuid)?;
        import::remove_files(uuid)
    }

    fn activate(&self, uuid: &str, password: Option<&str>) -> Result<(), NetError> {
        let path = self.profile_path(uuid)?;
        if let Some(password) = password {
            self.once.put(uuid, &SecretKey::new("vpn", crate::profile::ovpn_key::PASSWORD), password);
        }
        // "/" for the device and the specific object lets NetworkManager choose.
        let none = ObjectPath::try_from(NO_OBJECT).map_err(|e| NetError::other(e.to_string()))?;
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        let result = nm
            .call::<_, _, OwnedObjectPath>("ActivateConnection", &(&path, &none, &none))
            .map_err(NetError::from)
            .and_then(|active| self.wait_active(&active).map_err(NetError::other));
        self.once.clear(uuid);
        result
    }

    fn deactivate(&self, uuid: &str) -> Result<(), NetError> {
        let active = self.active_connections()?;
        let (path, _, _) = active.iter().find(|(_, u, _)| u == uuid).ok_or_else(|| NetError::other("The connection is not active"))?;
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        nm.call::<_, _, ()>("DeactivateConnection", &(path,)).map_err(NetError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_protected_profile_has_the_three_settings() {
        let s = new_profile("home", Some(("wpa-psk", "secret")));
        assert_eq!(s.len(), 3);
        assert!(s["802-11-wireless-security"].contains_key("psk"));
        assert!(s["connection"].contains_key("id"));
        assert!(s["802-11-wireless"].contains_key("ssid"));
    }

    /// A saved profile with the key flags `flags`, or none at all.
    fn saved(flags: Option<u32>) -> Settings {
        let mut group = Props::new();
        if let Some(flags) = flags {
            group.insert("psk-flags".to_string(), OwnedValue::try_from(Value::from(flags)).unwrap());
        }
        Settings::from([(SECURITY_GROUP.to_string(), group)])
    }

    #[test]
    fn a_key_owned_by_an_agent_or_not_saved_is_not_stored() {
        assert!(key_not_stored(&saved(Some(1))));
        assert!(key_not_stored(&saved(Some(2))));
        assert!(key_not_stored(&saved(Some(3))));
    }

    #[test]
    fn a_stored_key_or_an_open_profile_needs_nothing() {
        assert!(!key_not_stored(&saved(Some(0))));
        assert!(!key_not_stored(&saved(None)));
        assert!(!key_not_stored(&Settings::new()));
    }

    #[test]
    fn text_of_reads_a_text_value_and_nothing_else() {
        let mut s = saved(Some(1));
        s.get_mut(SECURITY_GROUP).unwrap().insert("psk".to_string(), OwnedValue::try_from(Value::from("secret")).unwrap());
        assert_eq!(text_of(&s, SECURITY_GROUP, "psk").as_deref(), Some("secret"));
        assert_eq!(text_of(&s, SECURITY_GROUP, "psk-flags"), None);
        assert_eq!(text_of(&s, SECURITY_GROUP, "missing"), None);
        assert_eq!(text_of(&s, "missing", "psk"), None);
    }

    #[test]
    fn a_new_profile_leaves_its_key_to_the_secret_agent() {
        let s = new_profile("home", Some(("wpa-psk", "secret")));
        assert_eq!(u32::try_from(s["802-11-wireless-security"]["psk-flags"].clone()).unwrap(), SECRET_AGENT_OWNED);
    }

    #[test]
    fn an_open_profile_is_empty_so_networkmanager_fills_it() {
        assert!(new_profile("cafe", None).is_empty());
    }
}
