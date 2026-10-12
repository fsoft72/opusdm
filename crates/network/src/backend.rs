//! The operations the panel needs, behind a trait so tests need no bus.

use crate::error::NetError;
use crate::model::NetState;
use std::sync::Mutex;

/// Operations on the network.
pub trait Backend: Send + Sync {
    /// The full current state.
    fn state(&self) -> Result<NetState, NetError>;
    /// Switch the Wi-Fi radio.
    fn set_wifi_enabled(&self, on: bool) -> Result<(), NetError>;
    /// Connect (`on`) or disconnect the Ethernet device.
    fn set_eth_enabled(&self, on: bool) -> Result<(), NetError>;
    /// Ask for a new scan of the Wi-Fi networks.
    fn scan(&self) -> Result<(), NetError>;
    /// Join the network `ssid`. `password` is only used for a network with no
    /// saved profile. Returns when the connection is up or has failed.
    fn connect(&self, ssid: &str, password: Option<&str>) -> Result<(), NetError>;
    /// Disconnect the Wi-Fi device.
    fn disconnect_wifi(&self) -> Result<(), NetError>;
    /// Bring the VPN `uuid` up; `password` is asked from the user and used
    /// for this connect only. Returns when it is up or has failed.
    fn activate_vpn(&self, uuid: &str, password: Option<&str>) -> Result<(), NetError>;
    /// Bring the VPN `uuid` down.
    fn deactivate_vpn(&self, uuid: &str) -> Result<(), NetError>;
}

/// A backend for tests: records every call (never the password).
#[derive(Default)]
pub struct FakeBackend {
    pub state: Mutex<NetState>,
    pub calls: Mutex<Vec<String>>,
    pub fail_connect: Mutex<Option<NetError>>,
}

impl FakeBackend {
    /// The recorded calls, oldest first.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(call);
    }
}

impl Backend for FakeBackend {
    fn state(&self) -> Result<NetState, NetError> {
        Ok(self.state.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn set_wifi_enabled(&self, on: bool) -> Result<(), NetError> {
        self.record(format!("wifi {on}"));
        Ok(())
    }

    fn set_eth_enabled(&self, on: bool) -> Result<(), NetError> {
        self.record(format!("eth {on}"));
        Ok(())
    }

    fn scan(&self) -> Result<(), NetError> {
        self.record("scan".to_string());
        Ok(())
    }

    fn connect(&self, ssid: &str, password: Option<&str>) -> Result<(), NetError> {
        self.record(format!("connect {ssid} password={}", password.is_some()));
        match self.fail_connect.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn disconnect_wifi(&self) -> Result<(), NetError> {
        self.record("disconnect".to_string());
        Ok(())
    }

    fn activate_vpn(&self, uuid: &str, password: Option<&str>) -> Result<(), NetError> {
        self.record(format!("vpn up {uuid} password={}", password.is_some()));
        match self.fail_connect.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn deactivate_vpn(&self, uuid: &str) -> Result<(), NetError> {
        self.record(format!("vpn down {uuid}"));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_calls_without_the_password() {
        let fake = FakeBackend::default();
        fake.connect("home", Some("secret")).unwrap();
        fake.set_wifi_enabled(false).unwrap();
        assert_eq!(fake.calls(), vec!["connect home password=true", "wifi false"]);
    }

    #[test]
    fn fake_can_fail_a_connect() {
        let fake = FakeBackend::default();
        *fake.fail_connect.lock().unwrap_or_else(|e| e.into_inner()) = Some(NetError::other("bad"));
        assert_eq!(fake.connect("home", None).unwrap_err().text(), "bad");
    }
}
