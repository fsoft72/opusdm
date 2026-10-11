//! The operations the panel needs, behind a trait so tests need no bus.

use crate::error::PowerError;
use crate::model::{PowerState, Profile};
use std::sync::Mutex;

/// Operations on the power services.
pub trait Backend: Send + Sync {
    /// The full current state.
    fn state(&self) -> Result<PowerState, PowerError>;
    /// Switch the power profile.
    fn set_profile(&self, profile: Profile) -> Result<(), PowerError>;
}

/// A backend for tests: records every call.
#[derive(Default)]
pub struct FakeBackend {
    pub state: Mutex<PowerState>,
    pub calls: Mutex<Vec<String>>,
}

impl FakeBackend {
    /// The recorded calls, oldest first.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Backend for FakeBackend {
    fn state(&self) -> Result<PowerState, PowerError> {
        Ok(self.state.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn set_profile(&self, profile: Profile) -> Result<(), PowerError> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(format!("profile {}", profile.id()));
        self.state.lock().unwrap_or_else(|e| e.into_inner()).active_profile = Some(profile);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_the_profile_and_applies_it() {
        let fake = FakeBackend::default();
        fake.set_profile(Profile::Performance).unwrap();
        assert_eq!(fake.calls(), vec!["profile performance"]);
        assert_eq!(fake.state().unwrap().active_profile, Some(Profile::Performance));
    }
}
