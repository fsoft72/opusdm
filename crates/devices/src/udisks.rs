//! The real backend: udisks2 over the D-Bus system bus.

use crate::backend::Backend;
use crate::error::DeviceError;
use crate::model::Device;
use crate::parse::{self, ManagedObjects};
use std::collections::HashMap;
use std::path::PathBuf;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

const SERVICE: &str = "org.freedesktop.UDisks2";
const ROOT_PATH: &str = "/org/freedesktop/UDisks2";
const MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const FS_IFACE: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE_IFACE: &str = "org.freedesktop.UDisks2.Drive";

/// udisks2 methods take an `a{sv}` of options; none are needed here.
type NoOptions<'a> = HashMap<&'a str, Value<'a>>;

/// udisks2 on the system bus.
pub struct UdisksBackend {
    conn: Connection,
}

impl UdisksBackend {
    /// Connect to the system bus. Does not need udisks2 to be running yet.
    pub fn connect() -> Result<Self, DeviceError> {
        let conn = Connection::system().map_err(|e| DeviceError::from_zbus(&e))?;
        Ok(Self { conn })
    }

    /// The connection, for the signal watcher.
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    /// A proxy for `iface` on object `path`.
    fn proxy(&self, path: &str, iface: &'static str) -> Result<Proxy<'static>, DeviceError> {
        Proxy::new(&self.conn, SERVICE, path.to_string(), iface).map_err(|e| DeviceError::from_zbus(&e))
    }
}

impl Backend for UdisksBackend {
    fn list(&self) -> Result<Vec<Device>, DeviceError> {
        let proxy = self.proxy(ROOT_PATH, MANAGER_IFACE)?;
        let objects: ManagedObjects =
            proxy.call("GetManagedObjects", &()).map_err(|e| DeviceError::from_zbus(&e))?;
        Ok(parse::devices(&objects))
    }

    fn mount(&self, path: &str) -> Result<PathBuf, DeviceError> {
        let proxy = self.proxy(path, FS_IFACE)?;
        let point: String = proxy
            .call("Mount", &(NoOptions::new(),))
            .map_err(|e| DeviceError::from_zbus(&e))?;
        Ok(PathBuf::from(point))
    }

    fn unmount(&self, path: &str) -> Result<(), DeviceError> {
        let proxy = self.proxy(path, FS_IFACE)?;
        proxy.call::<_, _, ()>("Unmount", &(NoOptions::new(),)).map_err(|e| DeviceError::from_zbus(&e))
    }

    fn eject_drive(&self, drive_path: &str, power_off: bool) -> Result<(), DeviceError> {
        let proxy = self.proxy(drive_path, DRIVE_IFACE)?;
        proxy.call::<_, _, ()>("Eject", &(NoOptions::new(),)).map_err(|e| DeviceError::from_zbus(&e))?;
        if !power_off { return Ok(()); }
        proxy.call::<_, _, ()>("PowerOff", &(NoOptions::new(),)).map_err(|e| DeviceError::from_zbus(&e))
    }
}
