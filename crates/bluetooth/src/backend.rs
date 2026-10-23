//! The operations on BlueZ, behind a trait so tests need no bus.

use crate::error::BtError;
use crate::model::BtState;
use std::sync::Mutex;

/// One change asked by the user. Device requests carry the device path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Power the adapter; on clears the rfkill soft block first.
    SetPowered(bool),
    /// Visible to other devices and open to their pairing, for a few minutes.
    SetDiscoverable(bool),
    SetAdapterAlias(String),
    /// Accept pairings started by other devices (while the pairing flow is open).
    SetPairable(bool),
    StartDiscovery,
    StopDiscovery,
    Connect(String),
    Disconnect(String),
    /// Pair; returns when the pairing ended. The agent of the caller is asked.
    Pair(String),
    CancelPairing(String),
    SetTrusted(String, bool),
    SetBlocked(String, bool),
    SetAlias(String, String),
    /// Forget the device.
    Remove(String),
}

/// Operations on Bluetooth.
pub trait Backend: Send + Sync {
    /// The full current state.
    fn state(&self) -> Result<BtState, BtError>;
    /// Run one request; returns when BlueZ answered.
    fn run(&self, request: &Request) -> Result<(), BtError>;
}

/// A backend for tests: records every request, fails when told to.
#[derive(Default)]
pub struct FakeBackend {
    pub state: Mutex<BtState>,
    pub calls: Mutex<Vec<Request>>,
    /// The error every request returns while set.
    pub fail: Mutex<Option<BtError>>,
}

impl FakeBackend {
    /// The recorded requests, oldest first.
    pub fn calls(&self) -> Vec<Request> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Backend for FakeBackend {
    fn state(&self) -> Result<BtState, BtError> {
        Ok(self.state.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn run(&self, request: &Request) -> Result<(), BtError> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(request.clone());
        match self.fail.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_calls() {
        let f = FakeBackend::default();
        assert!(f.run(&Request::Connect("/d".into())).is_ok());
        *f.fail.lock().unwrap() = Some(BtError::other("no"));
        assert!(f.run(&Request::StopDiscovery).is_err());
        assert_eq!(f.calls(), vec![Request::Connect("/d".into()), Request::StopDiscovery]);
    }
}
