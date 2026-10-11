//! The power state as the panel sees it.

/// What the battery is doing, as UPower reports it (`State` of a device).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChargeState {
    #[default]
    Unknown,
    Charging,
    Discharging,
    Empty,
    FullyCharged,
    /// Plugged in, but the battery is not taking charge (charge threshold).
    PendingCharge,
    PendingDischarge,
}

impl ChargeState {
    /// Map the UPower `State` number; anything unknown is `Unknown`.
    pub fn from_upower(state: u32) -> Self {
        match state {
            1 => Self::Charging,
            2 => Self::Discharging,
            3 => Self::Empty,
            4 => Self::FullyCharged,
            5 => Self::PendingCharge,
            6 => Self::PendingDischarge,
            _ => Self::Unknown,
        }
    }
}

/// The battery (all batteries combined, as UPower's display device).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Battery {
    /// Charge level, 0 to 100.
    pub percent: u8,
    pub state: ChargeState,
    /// Seconds until empty while discharging, 0 when unknown.
    pub time_to_empty: u64,
    /// Seconds until full while charging, 0 when unknown.
    pub time_to_full: u64,
    /// Full capacity now against the design capacity, 0 to 100, when known.
    pub health: Option<u8>,
}

/// A power profile of power-profiles-daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Profile {
    PowerSaver,
    Balanced,
    Performance,
}

impl Profile {
    /// Every profile, from the most frugal to the fastest.
    pub const ALL: [Profile; 3] = [Profile::PowerSaver, Profile::Balanced, Profile::Performance];

    /// The name power-profiles-daemon uses.
    pub fn id(self) -> &'static str {
        match self {
            Self::PowerSaver => "power-saver",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }

    /// The profile with this daemon name, if it is one of ours.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            Self::PowerSaver => "Power Save",
            Self::Balanced => "Balanced",
            Self::Performance => "Performance",
        }
    }
}

/// Everything the battery tray shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PowerState {
    /// `None` when the machine has no battery.
    pub battery: Option<Battery>,
    /// Running on mains power.
    pub on_ac: bool,
    /// Profiles the daemon offers, in `Profile::ALL` order; empty without the daemon.
    pub profiles: Vec<Profile>,
    pub active_profile: Option<Profile>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upower_states_map_to_charge_states() {
        assert_eq!(ChargeState::from_upower(1), ChargeState::Charging);
        assert_eq!(ChargeState::from_upower(4), ChargeState::FullyCharged);
        assert_eq!(ChargeState::from_upower(5), ChargeState::PendingCharge);
        assert_eq!(ChargeState::from_upower(99), ChargeState::Unknown);
    }

    #[test]
    fn profile_ids_round_trip() {
        for p in Profile::ALL {
            assert_eq!(Profile::from_id(p.id()), Some(p));
        }
        assert_eq!(Profile::from_id("turbo"), None);
    }
}
