//! The operations the panel needs, behind a trait so tests need no server.

use crate::error::AudioError;
use crate::model::{AudioDevice, Kind};
use crate::volume::MAX_PERCENT;
use std::sync::Mutex;

/// Operations on the sound devices.
pub trait Backend: Send + Sync {
    /// Every output and every input (monitors left out), outputs first.
    fn list(&self) -> Result<Vec<AudioDevice>, AudioError>;
    /// Set the volume of a device to `percent` (at most 100) on every channel.
    fn set_volume(&self, kind: Kind, index: u32, percent: u32) -> Result<(), AudioError>;
    /// Mute or unmute a device.
    fn set_mute(&self, kind: Kind, index: u32, muted: bool) -> Result<(), AudioError>;
    /// Make the device called `name` the default of its `kind`.
    fn set_default(&self, kind: Kind, name: &str) -> Result<(), AudioError>;
}

/// A backend for tests: records every call and applies it to its own list.
#[derive(Default)]
pub struct FakeBackend {
    pub devices: Mutex<Vec<AudioDevice>>,
    pub calls: Mutex<Vec<String>>,
}

impl FakeBackend {
    /// The recorded calls, oldest first.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Record `call` in the log.
    fn record(&self, call: String) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(call);
    }
}

impl Backend for FakeBackend {
    fn list(&self) -> Result<Vec<AudioDevice>, AudioError> {
        Ok(self.devices.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn set_volume(&self, kind: Kind, index: u32, percent: u32) -> Result<(), AudioError> {
        self.record(format!("volume {kind:?} {index} {percent}"));
        let mut devices = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        let d = devices.iter_mut().find(|d| d.key() == (kind, index)).ok_or(AudioError::NoSuchDevice)?;
        d.volume_percent = percent.min(MAX_PERCENT);
        Ok(())
    }

    fn set_mute(&self, kind: Kind, index: u32, muted: bool) -> Result<(), AudioError> {
        self.record(format!("mute {kind:?} {index} {muted}"));
        let mut devices = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        let d = devices.iter_mut().find(|d| d.key() == (kind, index)).ok_or(AudioError::NoSuchDevice)?;
        d.muted = muted;
        Ok(())
    }

    fn set_default(&self, kind: Kind, name: &str) -> Result<(), AudioError> {
        self.record(format!("default {kind:?} {name}"));
        let mut devices = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        if !devices.iter().any(|d| d.kind == kind && d.name == name) {
            return Err(AudioError::NoSuchDevice);
        }
        for d in devices.iter_mut().filter(|d| d.kind == kind) {
            d.is_default = d.name == name;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(kind: Kind, index: u32, is_default: bool) -> AudioDevice {
        AudioDevice {
            index,
            name: format!("dev{index}"),
            description: String::new(),
            kind,
            volume_percent: 40,
            muted: false,
            is_default,
        }
    }

    fn fake() -> FakeBackend {
        let f = FakeBackend::default();
        *f.devices.lock().unwrap_or_else(|e| e.into_inner()) = vec![
            dev(Kind::Output, 1, true),
            dev(Kind::Output, 2, false),
            dev(Kind::Input, 1, true),
        ];
        f
    }

    #[test]
    fn set_volume_is_recorded_and_clamped() {
        let f = fake();
        f.set_volume(Kind::Output, 2, 250).unwrap();
        assert_eq!(f.calls(), vec!["volume Output 2 250"]);
        assert_eq!(f.list().unwrap()[1].volume_percent, 100);
    }

    #[test]
    fn an_unknown_device_is_no_such_device() {
        let f = fake();
        assert_eq!(f.set_volume(Kind::Output, 9, 10), Err(AudioError::NoSuchDevice));
        assert_eq!(f.set_mute(Kind::Input, 9, true), Err(AudioError::NoSuchDevice));
        assert_eq!(f.set_default(Kind::Output, "nope"), Err(AudioError::NoSuchDevice));
    }

    #[test]
    fn mute_only_touches_the_chosen_device() {
        let f = fake();
        f.set_mute(Kind::Input, 1, true).unwrap();
        let list = f.list().unwrap();
        assert!(list[2].muted);
        assert!(!list[0].muted);
    }

    #[test]
    fn the_default_moves_within_its_kind_only() {
        let f = fake();
        f.set_default(Kind::Output, "dev2").unwrap();
        let list = f.list().unwrap();
        assert!(!list[0].is_default);
        assert!(list[1].is_default);
        assert!(list[2].is_default, "the input default must not change");
    }
}
