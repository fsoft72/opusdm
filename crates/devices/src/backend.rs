//! The operations the panel needs, behind a trait so tests need no bus.

use crate::error::DeviceError;
use crate::model::Device;
use std::path::PathBuf;
use std::sync::Mutex;

/// Operations on removable volumes.
pub trait Backend: Send + Sync {
    /// Every volume that belongs in the notifier.
    fn list(&self) -> Result<Vec<Device>, DeviceError>;
    /// Mount the volume at object path `path`; returns the mount point.
    fn mount(&self, path: &str) -> Result<PathBuf, DeviceError>;
    /// Unmount the volume at object path `path`.
    fn unmount(&self, path: &str) -> Result<(), DeviceError>;
    /// Eject the drive at `drive_path`, powering it off when `power_off`.
    fn eject_drive(&self, drive_path: &str, power_off: bool) -> Result<(), DeviceError>;
}

/// A backend for tests: records every call and can fail the unmounts.
#[derive(Default)]
pub struct FakeBackend {
    pub devices: Mutex<Vec<Device>>,
    pub calls: Mutex<Vec<String>>,
    pub fail_unmount: Mutex<Option<DeviceError>>,
}

impl FakeBackend {
    /// The recorded calls, oldest first.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Backend for FakeBackend {
    fn list(&self) -> Result<Vec<Device>, DeviceError> {
        Ok(self.devices.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn mount(&self, path: &str) -> Result<PathBuf, DeviceError> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(format!("mount {path}"));
        Ok(PathBuf::from("/media/test"))
    }

    fn unmount(&self, path: &str) -> Result<(), DeviceError> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(format!("unmount {path}"));
        match self.fail_unmount.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn eject_drive(&self, drive_path: &str, power_off: bool) -> Result<(), DeviceError> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(format!("eject {drive_path} {power_off}"));
        Ok(())
    }
}
