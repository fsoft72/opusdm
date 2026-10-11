//! The real backend: UPower and power-profiles-daemon over the D-Bus system bus.

use crate::backend::Backend;
use crate::error::PowerError;
use crate::model::{PowerState, Profile};
use crate::parse::{self, Props};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

/// The bus name of UPower.
pub const UPOWER_SERVICE: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const UPOWER_IFACE: &str = "org.freedesktop.UPower";
const DISPLAY_PATH: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const DEVICE_IFACE: &str = "org.freedesktop.UPower.Device";
/// The bus name of power-profiles-daemon (0.20 and later; it also owns the
/// older `net.hadess.PowerProfiles`).
pub const PROFILES_SERVICE: &str = "org.freedesktop.UPower.PowerProfiles";
const PROFILES_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
const PROFILES_IFACE: &str = "org.freedesktop.UPower.PowerProfiles";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";

/// UPower and power-profiles-daemon on the system bus.
pub struct UPowerBackend {
    conn: Connection,
}

impl UPowerBackend {
    /// Connect to the system bus. Does not need either service to be running yet.
    pub fn connect() -> Result<Self, PowerError> {
        let conn = Connection::system().map_err(|e| PowerError::from_zbus(&e))?;
        Ok(Self { conn })
    }

    /// The connection, for the signal watcher.
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    /// A proxy for `iface` on object `path` of `service`.
    fn proxy(&self, service: &'static str, path: &str, iface: &'static str) -> Result<Proxy<'static>, PowerError> {
        Proxy::new(&self.conn, service, path.to_string(), iface).map_err(|e| PowerError::from_zbus(&e))
    }

    /// `GetAll` of the UPower device interface of one object.
    fn device_props(&self, path: &str) -> Result<Props, PowerError> {
        let props = self.proxy(UPOWER_SERVICE, path, PROPERTIES_IFACE)?;
        props.call("GetAll", &(DEVICE_IFACE,)).map_err(|e| PowerError::from_zbus(&e))
    }

    /// The health of the first battery that powers the machine, when known.
    fn battery_health(&self) -> Result<Option<u8>, PowerError> {
        let upower = self.proxy(UPOWER_SERVICE, UPOWER_PATH, UPOWER_IFACE)?;
        let paths: Vec<OwnedObjectPath> = upower.call("EnumerateDevices", &()).map_err(|e| PowerError::from_zbus(&e))?;
        for path in paths {
            // A device can vanish between the two calls.
            let Ok(props) = self.device_props(path.as_str()) else { continue };
            if parse::is_system_battery(&props) {
                return Ok(parse::health_of(&props));
            }
        }
        Ok(None)
    }

    /// The profiles offered and the active one; nothing when the daemon is missing.
    fn profiles(&self) -> (Vec<Profile>, Option<Profile>) {
        let Ok(proxy) = self.proxy(PROFILES_SERVICE, PROFILES_PATH, PROFILES_IFACE) else { return (Vec::new(), None) };
        let list: Vec<Props> = proxy.get_property("Profiles").unwrap_or_default();
        let active: Option<String> = proxy.get_property("ActiveProfile").ok();
        (parse::profiles_from(&list), active.as_deref().and_then(Profile::from_id))
    }
}

impl Backend for UPowerBackend {
    fn state(&self) -> Result<PowerState, PowerError> {
        let upower = self.proxy(UPOWER_SERVICE, UPOWER_PATH, UPOWER_IFACE)?;
        let on_battery: bool = upower.get_property("OnBattery").map_err(|e| PowerError::from_zbus(&e))?;
        let display = self.device_props(DISPLAY_PATH)?;
        let battery = parse::battery_from(&display, self.battery_health()?);
        let (profiles, active_profile) = self.profiles();
        Ok(PowerState { battery, on_ac: !on_battery, profiles, active_profile })
    }

    fn set_profile(&self, profile: Profile) -> Result<(), PowerError> {
        let proxy = self.proxy(PROFILES_SERVICE, PROFILES_PATH, PROFILES_IFACE)?;
        proxy.set_property("ActiveProfile", profile.id()).map_err(|e| PowerError::from_zbus(&zbus::Error::from(e)))
    }
}
