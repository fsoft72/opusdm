//! [`Backend`] over the system bus, talking to BlueZ.

use crate::backend::{Backend, Request};
use crate::error::BtError;
use crate::model::BtState;
use crate::parse::{self, ManagedObjects, ADAPTER_IFACE, DEVICE_IFACE};
use crate::rfkill;
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, Value};

/// The bus name of BlueZ.
pub const SERVICE: &str = "org.bluez";
const ROOT_PATH: &str = "/";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
/// Seconds the adapter stays discoverable and pairable after "Discoverable".
pub const VISIBLE_SECS: u32 = 180;

/// BlueZ on the system bus. The agent of this process is served on the same
/// connection, so BlueZ sends the requests of a `Pair` call made here to it.
pub struct BluezBackend {
    conn: Connection,
}

impl BluezBackend {
    /// Connect to the system bus. Does not need BlueZ to be running yet.
    pub fn connect() -> Result<Self, BtError> {
        Ok(Self { conn: Connection::system()? })
    }

    /// The connection, for the watcher and the agent.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Every BlueZ object with its properties.
    fn objects(&self) -> Result<ManagedObjects, BtError> {
        let reply = self.conn.call_method(Some(SERVICE), ROOT_PATH, Some(OBJECT_MANAGER_IFACE), "GetManagedObjects", &())?;
        reply.body().deserialize::<ManagedObjects>().map_err(BtError::from)
    }

    /// The path of the adapter, or an error when there is none.
    fn adapter_path(&self) -> Result<String, BtError> {
        let state = self.state()?;
        state.adapter.map(|a| a.path).ok_or_else(|| BtError::other("No Bluetooth adapter"))
    }

    /// Call `method` of `iface` on `path` with no arguments.
    fn call(&self, path: &str, iface: &str, method: &str) -> Result<(), BtError> {
        self.conn.call_method(Some(SERVICE), path, Some(iface), method, &())?;
        Ok(())
    }

    /// Set the property `name` of `iface` on `path`.
    fn set<'a>(&self, path: &str, iface: &str, name: &str, value: impl Into<Value<'a>>) -> Result<(), BtError> {
        let value: Value = value.into();
        self.conn.call_method(Some(SERVICE), path, Some(PROPERTIES_IFACE), "Set", &(iface, name, value))?;
        Ok(())
    }

    /// Power the adapter on or off; on clears the rfkill soft block first.
    fn set_powered(&self, on: bool) -> Result<(), BtError> {
        let rf = rfkill::read();
        if on && rf.hard { return Err(BtError::other("Bluetooth is blocked by a hardware switch")); }
        if on && rf.soft {
            rfkill::unblock()?;
            // The adapter comes back a moment after the block is cleared.
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let path = self.adapter_path()?;
        self.set(&path, ADAPTER_IFACE, "Powered", on)
    }

    /// Discoverable and pairable together for [`VISIBLE_SECS`], or neither.
    fn set_discoverable(&self, on: bool) -> Result<(), BtError> {
        let path = self.adapter_path()?;
        if on {
            self.set(&path, ADAPTER_IFACE, "DiscoverableTimeout", VISIBLE_SECS)?;
            self.set(&path, ADAPTER_IFACE, "PairableTimeout", VISIBLE_SECS)?;
        }
        self.set(&path, ADAPTER_IFACE, "Pairable", on)?;
        self.set(&path, ADAPTER_IFACE, "Discoverable", on)
    }
}

impl Backend for BluezBackend {
    fn state(&self) -> Result<BtState, BtError> {
        Ok(parse::state(&self.objects()?, rfkill::read()))
    }

    fn run(&self, request: &Request) -> Result<(), BtError> {
        match request {
            Request::SetPowered(on) => self.set_powered(*on),
            Request::SetDiscoverable(on) => self.set_discoverable(*on),
            Request::SetAdapterAlias(alias) => self.set(&self.adapter_path()?, ADAPTER_IFACE, "Alias", alias.as_str()),
            Request::SetPairable(on) => {
                let path = self.adapter_path()?;
                // A timeout too, so a flow left open does not keep the
                // adapter pairable for good.
                if *on { self.set(&path, ADAPTER_IFACE, "PairableTimeout", VISIBLE_SECS)?; }
                self.set(&path, ADAPTER_IFACE, "Pairable", *on)
            }
            Request::StartDiscovery => self.call(&self.adapter_path()?, ADAPTER_IFACE, "StartDiscovery"),
            Request::StopDiscovery => self.call(&self.adapter_path()?, ADAPTER_IFACE, "StopDiscovery"),
            Request::Connect(dev) => self.call(dev, DEVICE_IFACE, "Connect"),
            Request::Disconnect(dev) => self.call(dev, DEVICE_IFACE, "Disconnect"),
            Request::Pair(dev) => self.call(dev, DEVICE_IFACE, "Pair"),
            Request::CancelPairing(dev) => self.call(dev, DEVICE_IFACE, "CancelPairing"),
            Request::SetTrusted(dev, on) => self.set(dev, DEVICE_IFACE, "Trusted", *on),
            Request::SetBlocked(dev, on) => self.set(dev, DEVICE_IFACE, "Blocked", *on),
            Request::SetAlias(dev, alias) => self.set(dev, DEVICE_IFACE, "Alias", alias.as_str()),
            Request::Remove(dev) => {
                let device = ObjectPath::try_from(dev.as_str()).map_err(|e| BtError::other(e.to_string()))?;
                self.conn.call_method(Some(SERVICE), self.adapter_path()?.as_str(), Some(ADAPTER_IFACE), "RemoveDevice", &(device,))?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the real BlueZ: run by hand with `--ignored`.
    #[test]
    #[ignore]
    fn live_reads_the_adapter() {
        let b = BluezBackend::connect().unwrap();
        let s = b.state().unwrap();
        eprintln!("{s:#?}");
        assert!(s.adapter.is_some());
    }
}
