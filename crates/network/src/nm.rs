//! The real backend: NetworkManager over the D-Bus system bus.

use crate::agent;
use crate::backend::Backend;
use crate::error::NetError;
use crate::keystore::{psk_key, KeyStore, OneShotSecrets, SecretServiceStore, SECURITY_GROUP};
use crate::import;
use crate::model::{ConnectStep, NetState, Security, VpnEntry};
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
const IP4_CONFIG_IFACE: &str = "org.freedesktop.NetworkManager.IP4Config";
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
/// How long a connection may take to come up. NetworkManager waits up to
/// 45 s for an address from DHCP, so the tray waits a little more.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(50);
/// How long a failed wait still listens for the reason of the device.
const LATE_REASON: Duration = Duration::from_millis(500);

/// `NM_DEVICE_STATE_REASON_*` values the tray explains.
mod device_reason {
    pub const CONFIG_FAILED: u32 = 4;
    pub const IP_CONFIG_UNAVAILABLE: u32 = 5;
    pub const IP_CONFIG_EXPIRED: u32 = 6;
    pub const NO_SECRETS: u32 = 7;
    pub const SUPPLICANT_DISCONNECT: u32 = 8;
    pub const SUPPLICANT_CONFIG_FAILED: u32 = 9;
    pub const SUPPLICANT_FAILED: u32 = 10;
    pub const SUPPLICANT_TIMEOUT: u32 = 11;
    pub const DHCP_START_FAILED: u32 = 15;
    pub const DHCP_ERROR: u32 = 16;
    pub const DHCP_FAILED: u32 = 17;
    pub const REMOVED: u32 = 36;
    pub const SLEEPING: u32 = 37;
    pub const USER_REQUESTED: u32 = 39;
    pub const SSID_NOT_FOUND: u32 = 53;
}

/// `NM_ACTIVE_CONNECTION_STATE_REASON_*` values the tray explains.
mod active_reason {
    pub const USER_DISCONNECTED: u32 = 2;
    pub const IP_CONFIG_INVALID: u32 = 5;
    pub const CONNECT_TIMEOUT: u32 = 6;
    pub const SERVICE_START_TIMEOUT: u32 = 7;
    pub const SERVICE_START_FAILED: u32 = 8;
    pub const NO_SECRETS: u32 = 9;
    pub const LOGIN_FAILED: u32 = 10;
    pub const DEPENDENCY_FAILED: u32 = 12;
}

/// Why a connect failed, as the user reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Failure {
    text: String,
    /// The network refused the key: ask for it again.
    refused: bool,
}

impl Failure {
    fn other(text: impl Into<String>) -> Self {
        Self { text: text.into(), refused: false }
    }

    fn refused(text: impl Into<String>) -> Self {
        Self { text: text.into(), refused: true }
    }

    /// The error the callers return.
    fn into_error(self) -> NetError {
        if self.refused { NetError::password_refused(self.text) } else { NetError::other(self.text) }
    }
}

/// The failure text of a connect from the last reasons NetworkManager gave:
/// the device reason first (Wi-Fi), then the active connection reason
/// (VPN). `protected` tells a network that needs a key.
fn failure_of(device: Option<u32>, active: Option<u32>, protected: bool) -> Failure {
    use device_reason as d;
    match device {
        Some(d::NO_SECRETS) if protected => return Failure::refused("The password was refused"),
        Some(d::NO_SECRETS) => return Failure::other("The network asked for a key"),
        Some(d::SUPPLICANT_DISCONNECT) if protected => return Failure::other("The access point dropped the connection (wrong password?)"),
        Some(d::SUPPLICANT_DISCONNECT) => return Failure::other("The access point dropped the connection"),
        Some(d::SUPPLICANT_TIMEOUT) => return Failure::other("The access point did not answer"),
        Some(d::SUPPLICANT_CONFIG_FAILED) => return Failure::other("The Wi-Fi settings of this network are not valid"),
        Some(d::SUPPLICANT_FAILED) => return Failure::other("The Wi-Fi service (wpa_supplicant) failed"),
        Some(d::IP_CONFIG_UNAVAILABLE | d::IP_CONFIG_EXPIRED | d::DHCP_START_FAILED | d::DHCP_ERROR | d::DHCP_FAILED) => {
            return Failure::other("No IP address from the network (DHCP failed)")
        }
        Some(d::SSID_NOT_FOUND) => return Failure::other("The network is out of range"),
        Some(d::USER_REQUESTED) => return Failure::other("The connect was cancelled"),
        Some(d::REMOVED) => return Failure::other("The Wi-Fi device was removed"),
        Some(d::SLEEPING) => return Failure::other("The computer is going to sleep"),
        Some(d::CONFIG_FAILED) => return Failure::other("The device could not be configured"),
        _ => {}
    }
    use active_reason as a;
    match active {
        Some(a::NO_SECRETS) => Failure::other("No password or key for this connection"),
        Some(a::LOGIN_FAILED) => Failure::other("Login failed (wrong password?)"),
        Some(a::CONNECT_TIMEOUT) => Failure::other("Connection timed out"),
        Some(a::SERVICE_START_TIMEOUT) => Failure::other("The VPN service did not start in time"),
        Some(a::SERVICE_START_FAILED) => Failure::other("The VPN service could not start"),
        Some(a::IP_CONFIG_INVALID) => Failure::other("The IP settings are not valid"),
        Some(a::DEPENDENCY_FAILED) => Failure::other("A connection this one needs failed"),
        Some(a::USER_DISCONNECTED) => Failure::other("The connect was cancelled"),
        _ if protected => Failure::other("Could not connect (wrong password?)"),
        _ => Failure::other("Could not connect"),
    }
}

/// What a wait has seen of a connect so far.
#[derive(Default)]
struct Seen {
    /// The last step reported.
    step: Option<ConnectStep>,
    /// The reason of the device failing or giving up.
    device_reason: Option<u32>,
    /// The reason of the last state change of the active connection.
    active_reason: Option<u32>,
}

impl Seen {
    /// A new device state: report its step once, keep the reason of a
    /// failure. A failure reason always wins over the reason of a device
    /// that only went down, and a new step forgets the reason of the device
    /// leaving the network it was on before.
    fn on_device(&mut self, state: u32, reason: Option<u32>, progress: &dyn Fn(ConnectStep)) {
        if state == parse::STATE_FAILED {
            if reason.is_some() { self.device_reason = reason; }
            return;
        }
        if state == parse::STATE_DEACTIVATING {
            if let Some(r) = reason { self.device_reason.get_or_insert(r); }
            return;
        }
        let Some(step) = parse::step_of_device_state(state) else { return };
        if step != ConnectStep::Connected { self.device_reason = None; }
        if self.step == Some(step) { return; }
        self.step = Some(step);
        progress(step);
    }
}

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

    /// A proxy for `iface` on object `path`. It does not cache properties:
    /// the proxies here live for a call or two, and a caching proxy first
    /// reads every property and subscribes to their changes.
    fn proxy(&self, path: &str, iface: &'static str) -> Result<Proxy<'static>, NetError> {
        let proxy = zbus::blocking::proxy::Builder::<Proxy<'static>>::new(&self.conn)
            .destination(SERVICE)?
            .path(path.to_string())?
            .interface(iface)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()?;
        Ok(proxy)
    }

    /// Every device with its type and state, one `GetAll` per device. A
    /// device that cannot be read is logged and left out.
    fn devices(&self) -> Result<Vec<Device>, NetError> {
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        let paths: Vec<OwnedObjectPath> = nm.call("GetDevices", &())?;
        let mut out = Vec::new();
        for path in paths {
            // A device can vanish between the two calls (a virtual
            // interface going away): skip it, the next read sees the truth.
            let props = match self.get_all(path.as_str(), DEVICE_IFACE) {
                Ok(props) => props,
                Err(e) => {
                    eprintln!("[opusdm-network:{}] device {path} skipped: {}", std::process::id(), e.text());
                    continue;
                }
            };
            let Some(device) = device_from_props(path.clone(), &props) else {
                eprintln!("[opusdm-network:{}] device {path} skipped: no type or state", std::process::id());
                continue;
            };
            out.push(device);
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

    /// The first IPv4 address of a device, `None` while it has no address.
    /// A read error is logged and reads as no address, so the tray still works.
    fn ip4_of(&self, device: &Device) -> Option<String> {
        if device.ip4_config == NO_OBJECT { return None; }
        let props = match self.get_all(&device.ip4_config, IP4_CONFIG_IFACE) {
            Ok(props) => props,
            Err(e) => {
                eprintln!("[opusdm-network:{}] IPv4 address: {}", std::process::id(), e.text());
                return None;
            }
        };
        let list = Vec::<HashMap<String, OwnedValue>>::try_from(props.get("AddressData")?.try_clone().ok()?).ok()?;
        String::try_from(list.first()?.get("address")?.try_clone().ok()?).ok()
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

    /// The active connections: object path, profile UUID and state, one
    /// `GetAll` per connection.
    fn active_connections(&self) -> Result<Vec<(OwnedObjectPath, String, u32)>, NetError> {
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        let paths: Vec<OwnedObjectPath> = nm.get_property("ActiveConnections")?;
        let mut out = Vec::new();
        for path in paths {
            // An active connection can vanish between the calls.
            let Ok(props) = self.get_all(path.as_str(), ACTIVE_IFACE) else { continue };
            let Some((uuid, state)) = active_from_props(&props) else { continue };
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

    /// Wait until the active connection is up, woken by its `StateChanged`
    /// signal instead of polling. With a `device`, its state changes are
    /// reported to `progress` and its failure reason explains the failure.
    /// `protected` tells a network that needs a key.
    fn wait_active(&self, active: &OwnedObjectPath, device: Option<&OwnedObjectPath>, protected: bool, progress: &dyn Fn(ConnectStep)) -> Result<(), Failure> {
        zbus::block_on(self.wait_active_async(active, device, protected, progress))
    }

    /// An async proxy on `path` that does not cache properties.
    async fn async_proxy(&self, path: &str, iface: &'static str) -> zbus::Result<zbus::Proxy<'static>> {
        zbus::proxy::Builder::<zbus::Proxy<'static>>::new(self.conn.inner())
            .destination(SERVICE)?
            .path(path.to_string())?
            .interface(iface)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
    }

    /// The body of [`Self::wait_active`]: subscribe to the `StateChanged`
    /// signals, then read the states, so no change slips in between; read
    /// the connection state again on every signal until it settles or
    /// [`CONNECT_TIMEOUT`] passes.
    async fn wait_active_async(&self, active: &OwnedObjectPath, device: Option<&OwnedObjectPath>, protected: bool, progress: &dyn Fn(ConnectStep)) -> Result<(), Failure> {
        use futures_lite::StreamExt;
        /// One thing the wait woke up for.
        enum Event {
            Active(Option<zbus::Message>),
            Device(Option<zbus::Message>),
            Timeout,
        }
        let failed = |e: zbus::Error| Failure::other(format!("Could not follow the connection: {e}"));
        let proxy = self.async_proxy(active.as_str(), ACTIVE_IFACE).await.map_err(failed)?;
        let mut changes = proxy.receive_signal("StateChanged").await.map_err(failed)?;
        let dev = match device {
            Some(path) => Some(self.async_proxy(path.as_str(), DEVICE_IFACE).await.map_err(failed)?),
            None => None,
        };
        let mut dev_changes = match &dev {
            Some(d) => Some(d.receive_signal("StateChanged").await.map_err(failed)?),
            None => None,
        };
        let mut seen = Seen::default();
        if let Some(d) = &dev {
            // Not fatal: the signals still bring the next states.
            // A device still up on the network it is leaving is not a step.
            match d.get_property::<u32>("State").await {
                Ok(state) if state == parse::STATE_ACTIVATED => {}
                Ok(state) => seen.on_device(state, None, progress),
                Err(e) => eprintln!("[opusdm-network:{}] connect: device state: {e}", std::process::id()),
            }
        }
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        loop {
            // The object is removed when the attempt fails, so a read error means it failed.
            let state = proxy.get_property::<u32>("State").await.unwrap_or(ACTIVE_DEACTIVATED);
            match activation_outcome(state) {
                Some(Ok(())) => {
                    if seen.step != Some(ConnectStep::Connected) { progress(ConnectStep::Connected); }
                    return Ok(());
                }
                Some(Err(_)) => {
                    // The device reason may come a moment after the connection ends.
                    if dev_changes.is_some() && seen.device_reason.is_none() {
                        self.late_device_reason(&mut dev_changes, &mut seen, progress).await;
                    }
                    return Err(failure_of(seen.device_reason, seen.active_reason, protected));
                }
                None => {}
            }
            let event = {
                let on_active = async { Event::Active(changes.next().await) };
                let on_device = async {
                    match dev_changes.as_mut() {
                        Some(stream) => Event::Device(stream.next().await),
                        None => futures_lite::future::pending().await,
                    }
                };
                let on_timeout = async {
                    async_io::Timer::at(deadline).await;
                    Event::Timeout
                };
                futures_lite::future::or(on_active, futures_lite::future::or(on_device, on_timeout)).await
            };
            match event {
                Event::Active(Some(msg)) => {
                    if let Ok((_, reason)) = msg.body().deserialize::<(u32, u32)>() { seen.active_reason = Some(reason); }
                }
                Event::Active(None) => return Err(Failure::other("Lost the connection to NetworkManager")),
                Event::Device(Some(msg)) => {
                    if let Ok((new, _old, reason)) = msg.body().deserialize::<(u32, u32, u32)>() { seen.on_device(new, Some(reason), progress); }
                }
                Event::Device(None) => dev_changes = None,
                Event::Timeout => return Err(Failure::other(format!("Connection timed out after {} seconds", CONNECT_TIMEOUT.as_secs()))),
            }
        }
    }

    /// Wait a moment for the device signal that carries the failure reason,
    /// when the connection ended before it arrived.
    async fn late_device_reason<S>(&self, dev_changes: &mut Option<S>, seen: &mut Seen, progress: &dyn Fn(ConnectStep))
    where
        S: futures_lite::Stream<Item = zbus::Message> + Unpin,
    {
        use futures_lite::StreamExt;
        let Some(stream) = dev_changes.as_mut() else { return };
        let until = Instant::now() + LATE_REASON;
        while seen.device_reason.is_none() {
            let next = async { stream.next().await };
            let timeout = async {
                async_io::Timer::at(until).await;
                None
            };
            let Some(msg) = futures_lite::future::or(next, timeout).await else { return };
            if let Ok((new, _old, reason)) = msg.body().deserialize::<(u32, u32, u32)>() { seen.on_device(new, Some(reason), progress); }
        }
    }

    /// Make sure a saved profile can be activated with the right key.
    ///
    /// A profile whose key NetworkManager does not keep needs one in the
    /// keyring. Without a `password` and without a key there, this asks the
    /// caller for one. With a `password` it goes into the keyring. Returns the
    /// UUID to forget if the connection then fails, `None` when nothing was kept.
    ///
    /// A profile whose key NetworkManager keeps itself gets the `password`
    /// written into it, when one is given: it replaces a key that was refused.
    fn ensure_key(&self, profile: &OwnedObjectPath, password: Option<&str>, progress: &dyn Fn(ConnectStep)) -> Result<Option<String>, NetError> {
        let conn = self.proxy(profile.as_str(), CONNECTION_IFACE)?;
        let settings: Settings = conn.call("GetSettings", &())?;
        if !key_not_stored(&settings) {
            if let Some(password) = password { replace_stored_key(&conn, settings, password)?; }
            return Ok(None);
        }
        progress(ConnectStep::Keyring);
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
    /// The `Ip4Config` object of the device; `/` while it has none.
    ip4_config: String,
}

/// The device at `path` from its `GetAll` properties; `None` without a
/// type or a state.
fn device_from_props(path: OwnedObjectPath, props: &Props) -> Option<Device> {
    let kind = u32::try_from(props.get("DeviceType")?).ok()?;
    let state = u32::try_from(props.get("State")?).ok()?;
    let ip4_config = props.get("Ip4Config").and_then(|v| OwnedObjectPath::try_from(v.try_clone().ok()?).ok()).map(|p| p.to_string()).unwrap_or_else(|| NO_OBJECT.to_string());
    Some(Device { path, kind, state, ip4_config })
}

/// The profile UUID and state of an active connection from its `GetAll`
/// properties; `None` without a UUID. A missing state reads as deactivated.
fn active_from_props(props: &Props) -> Option<(String, u32)> {
    let uuid = String::try_from(props.get("Uuid")?.try_clone().ok()?).ok()?;
    let state = props.get("State").and_then(|v| u32::try_from(v).ok()).unwrap_or(ACTIVE_DEACTIVATED);
    Some((uuid, state))
}

/// How a wait for an active connection ends at `state`: `Ok` once it is
/// activated, the failure text once it is deactivated, `None` while it is
/// still on its way (activating or deactivating).
fn activation_outcome(state: u32) -> Option<Result<(), String>> {
    match state {
        ACTIVE_ACTIVATED => Some(Ok(())),
        ACTIVE_DEACTIVATED => Some(Err("Could not connect".to_string())),
        _ => None,
    }
}

/// Whether the profile has a key that is not stored in it.
fn key_not_stored(settings: &Settings) -> bool {
    let flags = settings.get(SECURITY_GROUP).and_then(|g| g.get("psk-flags")).and_then(|v| u32::try_from(v.clone()).ok());
    flags.is_some_and(|f| f & SECRET_NOT_STORED != 0)
}

/// Write `password` as the key of a Wi-Fi profile whose key NetworkManager
/// keeps itself (`settings` is the profile without its secrets).
fn replace_stored_key(conn: &Proxy<'static>, mut settings: Settings, password: &str) -> Result<(), NetError> {
    let Some(group) = settings.get_mut(SECURITY_GROUP) else { return Ok(()) };
    let value = OwnedValue::try_from(Value::from(password)).map_err(|e| NetError::other(e.to_string()))?;
    group.insert("psk".to_string(), value);
    conn.call::<_, _, ()>("Update", &(settings,)).map_err(NetError::from)
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
            state.eth_busy = parse::is_busy(eth.state);
            state.eth_ip4 = if connected { self.ip4_of(eth) } else { None };
        }
        let Some(wifi) = devices.iter().find(|d| d.kind == DEVICE_WIFI) else { return Ok(state) };
        state.wifi_present = true;
        if !wifi_enabled { return Ok(state); }
        state.wifi_busy = parse::is_busy(wifi.state);
        state.wifi_ip4 = if wifi.state == parse::STATE_ACTIVATED { self.ip4_of(wifi) } else { None };
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

    fn connect(&self, ssid: &str, password: Option<&str>, progress: &dyn Fn(ConnectStep)) -> Result<(), NetError> {
        progress(ConnectStep::Searching);
        let wifi = self.device_of(DEVICE_WIFI, "Wi-Fi")?;
        let raws = self.read_aps(&wifi)?;
        let target = raws
            .iter()
            .filter(|r| r.ssid == ssid.as_bytes())
            .max_by_key(|r| r.strength)
            .ok_or_else(|| NetError::other(format!("{ssid} is out of range")))?;
        let security = target.security();
        if !security.is_supported() { return Err(NetError::other("This kind of network is not supported")); }
        let protected = security != Security::Open;
        let ap = ObjectPath::try_from(target.path.as_str()).map_err(|e| NetError::other(e.to_string()))?;
        let nm = self.proxy(ROOT_PATH, NM_IFACE)?;
        let profile = self.saved_profiles()?.into_iter().find(|(_, name)| name == ssid).map(|(p, _)| p);
        if let Some(profile) = profile {
            let kept = self.ensure_key(&profile, password, progress)?;
            progress(ConnectStep::Starting);
            let active: OwnedObjectPath = nm
                .call("ActivateConnection", &(&profile, &wifi.path, &ap))
                ?;
            return self.wait_active(&active, Some(&wifi.path), protected, progress).map_err(|why| {
                // A key that did not work must not stay in the keyring.
                if let Some(uuid) = &kept {
                    if let Err(e) = self.keys.delete(uuid, &psk_key()) {
                        eprintln!("[opusdm-network:{}] connect {ssid}: the refused key stays in the keyring: {}", std::process::id(), e.text());
                    }
                }
                why.into_error()
            });
        }
        let key = match (security, password) {
            (Security::Open, _) => None,
            (_, None) => return Err(NetError::password_needed()),
            (Security::Sae, Some(p)) => Some(("sae", p)),
            (_, Some(p)) => Some(("wpa-psk", p)),
        };
        progress(ConnectStep::Starting);
        let (created, active): (OwnedObjectPath, OwnedObjectPath) = nm
            .call("AddAndActivateConnection", &(new_profile(ssid, key), &wifi.path, &ap))
            ?;
        self.wait_active(&active, Some(&wifi.path), protected, progress).map_err(|why| {
            // A profile that never worked must not stay, or it would be
            // tried again with the same wrong password.
            self.delete_profile(&created);
            why.into_error()
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
            .and_then(|active| self.wait_active(&active, None, false, &|_| {}).map_err(Failure::into_error));
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

    /// Properties as one `GetAll` returns them.
    fn props(pairs: Vec<(&str, Value<'static>)>) -> Props {
        pairs.into_iter().map(|(k, v)| (k.to_string(), OwnedValue::try_from(v).unwrap())).collect()
    }

    #[test]
    fn a_device_is_read_from_one_get_all() {
        let path = OwnedObjectPath::try_from("/org/freedesktop/NetworkManager/Devices/3").unwrap();
        let all = props(vec![("DeviceType", Value::from(DEVICE_WIFI)), ("State", Value::from(100u32)), ("Interface", Value::from("wlan0"))]);
        let dev = device_from_props(path.clone(), &all).unwrap();
        assert_eq!((dev.path, dev.kind, dev.state), (path.clone(), DEVICE_WIFI, 100));
        assert!(device_from_props(path, &props(vec![("State", Value::from(100u32))])).is_none());
    }

    #[test]
    fn an_active_connection_is_read_from_one_get_all() {
        let all = props(vec![("Uuid", Value::from("abc")), ("State", Value::from(ACTIVE_ACTIVATED))]);
        assert_eq!(active_from_props(&all), Some(("abc".to_string(), ACTIVE_ACTIVATED)));
        let no_state = props(vec![("Uuid", Value::from("abc"))]);
        assert_eq!(active_from_props(&no_state), Some(("abc".to_string(), ACTIVE_DEACTIVATED)));
        assert_eq!(active_from_props(&props(vec![("State", Value::from(1u32))])), None);
    }

    /// Needs NetworkManager on the system bus; read only. Run with
    /// `cargo test -p opusdm-network -- --ignored`.
    #[test]
    #[ignore]
    fn the_live_state_and_profiles_can_be_read() {
        let nm = NmBackend::connect().unwrap();
        nm.state().unwrap();
        ProfileBackend::list(&nm).unwrap();
    }

    #[test]
    fn only_activated_and_deactivated_end_the_wait() {
        assert_eq!(activation_outcome(ACTIVE_ACTIVATED), Some(Ok(())));
        assert_eq!(activation_outcome(ACTIVE_DEACTIVATED), Some(Err("Could not connect".to_string())));
        assert_eq!(activation_outcome(ACTIVE_ACTIVATING), None);
        assert_eq!(activation_outcome(3), None, "deactivating keeps waiting for the end");
    }

    #[test]
    fn a_refused_key_asks_again_only_on_a_protected_network() {
        let f = failure_of(Some(device_reason::NO_SECRETS), Some(3), true);
        assert!(f.refused);
        assert_eq!(f.text, "The password was refused");
        assert!(f.clone().into_error().password_refused);
        assert!(!failure_of(Some(device_reason::NO_SECRETS), None, false).refused);
    }

    #[test]
    fn the_device_reason_wins_and_the_active_reason_explains_a_vpn() {
        assert_eq!(failure_of(Some(device_reason::DHCP_FAILED), Some(active_reason::LOGIN_FAILED), true).text, "No IP address from the network (DHCP failed)");
        assert_eq!(failure_of(Some(device_reason::SSID_NOT_FOUND), None, true).text, "The network is out of range");
        assert_eq!(failure_of(None, Some(active_reason::LOGIN_FAILED), false).text, "Login failed (wrong password?)");
        assert_eq!(failure_of(None, Some(active_reason::SERVICE_START_FAILED), false).text, "The VPN service could not start");
        assert!(!failure_of(Some(device_reason::SUPPLICANT_DISCONNECT), None, true).refused, "a dropped link is not a sure wrong password");
    }

    #[test]
    fn an_unknown_reason_falls_back_to_the_old_texts() {
        assert_eq!(failure_of(None, None, false).text, "Could not connect");
        assert_eq!(failure_of(Some(999), Some(999), true).text, "Could not connect (wrong password?)");
    }

    #[test]
    fn device_states_are_reported_once_and_the_failure_reason_is_kept() {
        let steps = std::cell::RefCell::new(Vec::new());
        let progress = |s: ConnectStep| steps.borrow_mut().push(s);
        let mut seen = Seen::default();
        // Leaving the old network: its reason is kept only until a new step.
        seen.on_device(parse::STATE_DEACTIVATING, Some(60), &progress);
        assert_eq!(seen.device_reason, Some(60));
        seen.on_device(40, Some(0), &progress);
        assert_eq!(seen.device_reason, None);
        seen.on_device(50, Some(0), &progress);
        seen.on_device(50, Some(0), &progress);
        seen.on_device(60, Some(0), &progress);
        seen.on_device(parse::STATE_FAILED, Some(device_reason::NO_SECRETS), &progress);
        // The device going down after the failure does not hide its reason.
        seen.on_device(parse::STATE_DEACTIVATING, Some(1), &progress);
        assert_eq!(seen.device_reason, Some(device_reason::NO_SECRETS));
        assert_eq!(*steps.borrow(), vec![ConnectStep::Preparing, ConnectStep::Associating, ConnectStep::Authenticating]);
    }

    #[test]
    fn an_open_profile_is_empty_so_networkmanager_fills_it() {
        assert!(new_profile("cafe", None).is_empty());
    }
}
