//! The operations on connection profiles, behind a trait so tests need no bus.

use crate::error::NetError;
use crate::profile::{Detail, Profile, ProfileKind};
use crate::settings::Settings;
use std::sync::Mutex;

/// What the list of profiles shows of one profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileSummary {
    pub uuid: String,
    pub name: String,
    pub kind: ProfileKind,
    /// `connection.type`, with the VPN service for VPN plugins (shown for
    /// the kinds this program does not edit).
    pub type_name: String,
    /// The connection is up.
    pub active: bool,
    /// The connection is coming up.
    pub activating: bool,
    /// An OpenVPN password asked at every connect.
    pub ask_password: bool,
}

impl ProfileSummary {
    /// The summary of a profile, not active.
    pub fn of(profile: &Profile) -> Self {
        let type_name = match &profile.detail {
            Detail::Other { conn_type, vpn_service: Some(service) } => format!("{conn_type} ({})", service.rsplit('.').next().unwrap_or(service)),
            Detail::Other { conn_type, vpn_service: None } => conn_type.clone(),
            _ => profile.kind().label().to_string(),
        };
        let ask_password = matches!(&profile.detail, Detail::OpenVpn(o) if o.ask_password && o.auth().uses_password());
        Self { uuid: profile.uuid.clone(), name: profile.id.clone(), kind: profile.kind(), type_name, active: false, activating: false, ask_password }
    }
}

/// A profile as read for editing: the model, with its secrets, and the map
/// it was read from (written back with the changes on save).
#[derive(Clone, Debug)]
pub struct Loaded {
    pub profile: Profile,
    pub base: Settings,
}

/// Operations on the connection profiles.
pub trait ProfileBackend: Send + Sync {
    /// Every profile, VPNs included, with whether it is active.
    fn list(&self) -> Result<Vec<ProfileSummary>, NetError>;
    /// One profile with its secrets.
    fn load(&self, uuid: &str) -> Result<Loaded, NetError>;
    /// Store a profile: add it when `base` is `None`, else update it. Its
    /// secrets go to the keyring.
    fn save(&self, profile: &Profile, base: Option<&Settings>) -> Result<(), NetError>;
    /// Remove a profile, its secrets and its imported files.
    fn delete(&self, uuid: &str) -> Result<(), NetError>;
    /// Bring a profile up; `password` is a VPN password asked from the user,
    /// used for this connect only. Returns when it is up or has failed.
    fn activate(&self, uuid: &str, password: Option<&str>) -> Result<(), NetError>;
    /// Bring a profile down.
    fn deactivate(&self, uuid: &str) -> Result<(), NetError>;
}

/// A backend for tests: profiles in memory, and every call recorded (never a secret).
#[derive(Default)]
pub struct FakeProfiles {
    pub profiles: Mutex<Vec<(Profile, bool)>>,
    pub calls: Mutex<Vec<String>>,
    pub fail: Mutex<Option<NetError>>,
}

impl FakeProfiles {
    /// The recorded calls, oldest first.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Record a call, then fail when a failure is set.
    fn record(&self, call: String) -> Result<(), NetError> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(call);
        match self.fail.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn profiles(&self) -> std::sync::MutexGuard<'_, Vec<(Profile, bool)>> {
        self.profiles.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Set whether the profile `uuid` is active.
    fn set_active(&self, uuid: &str, on: bool) -> Result<(), NetError> {
        let mut all = self.profiles();
        let entry = all.iter_mut().find(|(p, _)| p.uuid == uuid).ok_or_else(|| NetError::other("No such profile"))?;
        entry.1 = on;
        Ok(())
    }
}

impl ProfileBackend for FakeProfiles {
    fn list(&self) -> Result<Vec<ProfileSummary>, NetError> {
        self.record("list".to_string())?;
        Ok(self.profiles().iter().map(|(p, active)| ProfileSummary { active: *active, ..ProfileSummary::of(p) }).collect())
    }

    fn load(&self, uuid: &str) -> Result<Loaded, NetError> {
        self.record(format!("load {uuid}"))?;
        let all = self.profiles();
        let (p, _) = all.iter().find(|(p, _)| p.uuid == uuid).ok_or_else(|| NetError::other("No such profile"))?;
        Ok(Loaded { profile: p.clone(), base: p.to_settings(&Settings::new()) })
    }

    fn save(&self, profile: &Profile, base: Option<&Settings>) -> Result<(), NetError> {
        self.record(format!("{} {}", if base.is_some() { "update" } else { "add" }, profile.uuid))?;
        let mut all = self.profiles();
        match all.iter_mut().find(|(p, _)| p.uuid == profile.uuid) {
            Some(entry) => entry.0 = profile.clone(),
            None => all.push((profile.clone(), false)),
        }
        Ok(())
    }

    fn delete(&self, uuid: &str) -> Result<(), NetError> {
        self.record(format!("delete {uuid}"))?;
        self.profiles().retain(|(p, _)| p.uuid != uuid);
        Ok(())
    }

    fn activate(&self, uuid: &str, password: Option<&str>) -> Result<(), NetError> {
        self.record(format!("activate {uuid} password={}", password.is_some()))?;
        self.set_active(uuid, true)
    }

    fn deactivate(&self, uuid: &str) -> Result<(), NetError> {
        self.record(format!("deactivate {uuid}"))?;
        self.set_active(uuid, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_names_other_kinds_by_their_type() {
        let mut base = Settings::new();
        crate::settings::set(&mut base, "connection", "uuid", "u");
        crate::settings::set(&mut base, "connection", "type", "vpn");
        crate::settings::set(&mut base, "vpn", "service-type", "org.freedesktop.NetworkManager.pptp");
        let p = Profile::from_settings(&base).unwrap();
        assert_eq!(ProfileSummary::of(&p).type_name, "vpn (pptp)");
        let eth = Profile::new(ProfileKind::Ethernet).unwrap();
        assert_eq!(ProfileSummary::of(&eth).type_name, "Ethernet");
    }

    #[test]
    fn a_summary_says_when_the_password_is_asked() {
        let mut p = Profile::new(ProfileKind::OpenVpn).unwrap();
        let Detail::OpenVpn(o) = &mut p.detail else { unreachable!() };
        o.ask_password = true;
        assert!(!ProfileSummary::of(&p).ask_password, "certificates only: no password");
        let Detail::OpenVpn(o) = &mut p.detail else { unreachable!() };
        o.put(crate::profile::ovpn_key::CONNECTION_TYPE, "password");
        assert!(ProfileSummary::of(&p).ask_password);
    }

    #[test]
    fn the_fake_adds_updates_activates_and_deletes() {
        let fake = FakeProfiles::default();
        let p = Profile::new(ProfileKind::Ethernet).unwrap();
        fake.save(&p, None).unwrap();
        fake.activate(&p.uuid, Some("pw")).unwrap();
        assert!(fake.list().unwrap()[0].active);
        let loaded = fake.load(&p.uuid).unwrap();
        fake.save(&loaded.profile, Some(&loaded.base)).unwrap();
        fake.delete(&p.uuid).unwrap();
        assert!(fake.list().unwrap().is_empty());
        let calls = fake.calls();
        assert_eq!(calls[0], format!("add {}", p.uuid));
        assert_eq!(calls[1], format!("activate {} password=true", p.uuid));
        assert!(calls.contains(&format!("update {}", p.uuid)));
    }
}
