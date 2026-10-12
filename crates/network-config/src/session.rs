//! The profile being edited: what it was, what it is now, and what saving it
//! needs.

use opusdm_network::import::{Imported, PendingFile};
use opusdm_network::profile::{Profile, ProfileKind};
use opusdm_network::settings::Settings;
use opusdm_network::Loaded;

/// One profile open in the editor.
#[derive(Clone, Debug)]
pub struct EditSession {
    /// The profile as loaded (or as created), to tell what changed.
    pub original: Profile,
    /// The profile with the user's changes.
    pub current: Profile,
    /// The map the profile was read from; `None` for a profile not saved yet.
    pub base: Option<Settings>,
    /// Files of an import, written when the profile is first saved.
    pub files: Vec<PendingFile>,
    /// What an import left out, shown above the editor.
    pub notes: Vec<String>,
}

impl EditSession {
    /// A profile read from NetworkManager.
    pub fn loaded(l: Loaded) -> Self {
        Self { original: l.profile.clone(), current: l.profile, base: Some(l.base), files: Vec::new(), notes: Vec::new() }
    }

    /// A new profile, not saved yet.
    pub fn created(p: Profile) -> Self {
        Self { original: p.clone(), current: p, base: None, files: Vec::new(), notes: Vec::new() }
    }

    /// An imported profile, not saved yet.
    pub fn imported(i: Imported) -> Self {
        Self { original: i.profile.clone(), current: i.profile, base: None, files: i.files, notes: i.ignored }
    }

    /// Whether the profile is not in NetworkManager yet.
    pub fn is_new(&self) -> bool {
        self.base.is_none()
    }

    /// Whether this program can edit the profile.
    pub fn is_editable(&self) -> bool {
        self.current.kind() != ProfileKind::Other
    }

    /// Whether there is something to save.
    pub fn is_dirty(&self) -> bool {
        self.is_new() || self.current != self.original
    }

    /// The problems that stop a save.
    pub fn errors(&self) -> Vec<(String, String)> {
        self.current.validate()
    }

    /// Whether Save may be pressed.
    pub fn can_save(&self) -> bool {
        self.is_editable() && self.is_dirty() && self.errors().is_empty()
    }

    /// Drop the changes.
    pub fn revert(&mut self) {
        self.current = self.original.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> EditSession {
        let p = Profile::new(ProfileKind::Ethernet).unwrap();
        EditSession::loaded(Loaded { base: p.to_settings(&Settings::new()), profile: p })
    }

    #[test]
    fn a_loaded_profile_is_clean_until_changed_and_revert_cleans_it() {
        let mut s = loaded();
        assert!(!s.is_new());
        assert!(!s.is_dirty());
        assert!(!s.can_save());
        s.current.id = "Office".into();
        assert!(s.is_dirty());
        assert!(s.can_save());
        s.revert();
        assert!(!s.is_dirty());
    }

    #[test]
    fn a_new_profile_is_dirty_but_saved_only_when_valid() {
        let mut s = EditSession::created(Profile::new(ProfileKind::Wifi).unwrap());
        assert!(s.is_new() && s.is_dirty());
        assert!(!s.can_save(), "no network name and no password yet");
        if let opusdm_network::profile::Detail::Wifi(w) = &mut s.current.detail {
            w.ssid = "home".into();
            w.psk = opusdm_network::profile::Secret::new("12345678");
        }
        assert!(s.can_save());
    }

    #[test]
    fn other_kinds_are_never_saved() {
        let mut base = Settings::new();
        opusdm_network::settings::set(&mut base, "connection", "uuid", "u");
        opusdm_network::settings::set(&mut base, "connection", "type", "bridge");
        let p = Profile::from_settings(&base).unwrap();
        let mut s = EditSession::loaded(Loaded { profile: p, base });
        s.current.id = "renamed".into();
        assert!(!s.is_editable());
        assert!(!s.can_save());
    }
}
