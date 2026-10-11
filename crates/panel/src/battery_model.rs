//! The battery as the panel sees it: pure logic, no GTK.

use opusdm_power::{Battery, ChargeState, PowerState, Profile, WatchMsg};

/// At or below this charge the icon turns red.
pub const LOW_PERCENT: u8 = 15;

/// What the panel icon draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IconInfo {
    /// Charge level, 0 to 100.
    pub percent: u8,
    /// On mains power: a bolt over the battery.
    pub plugged: bool,
    /// Low and not plugged in: drawn in red.
    pub low: bool,
}

/// The power state and whether UPower is reachable.
#[derive(Default)]
pub struct Model {
    state: PowerState,
    available: bool,
}

impl Model {
    /// Apply one message of the watcher.
    pub fn apply(&mut self, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(_) => {}
            WatchMsg::Unavailable(_) => {
                self.state = PowerState::default();
                self.available = false;
            }
            WatchMsg::State(state) => {
                self.state = state;
                self.available = true;
            }
        }
    }

    /// Whether the tray is shown: UPower is reachable and there is a battery.
    pub fn is_visible(&self) -> bool {
        self.available && self.state.battery.is_some()
    }

    /// The current state.
    pub fn state(&self) -> &PowerState {
        &self.state
    }

    /// What the icon draws; an empty battery when there is none.
    pub fn icon_info(&self) -> IconInfo {
        let Some(b) = &self.state.battery else { return IconInfo::default() };
        IconInfo { percent: b.percent, plugged: self.state.on_ac, low: !self.state.on_ac && b.percent <= LOW_PERCENT }
    }

    /// The icon tooltip: level, what the battery does and the time left.
    pub fn tooltip(&self) -> String {
        let Some(b) = &self.state.battery else { return "No battery".to_string() };
        let mut text = format!("Battery {}% - {}", b.percent, status_text(b, self.state.on_ac));
        if let Some(time) = time_text(b) {
            text.push('\n');
            text.push_str(&time);
        }
        text
    }
}

/// What the battery is doing, in words.
pub fn status_text(b: &Battery, on_ac: bool) -> &'static str {
    match b.state {
        ChargeState::Charging => "Charging",
        ChargeState::Discharging | ChargeState::PendingDischarge => "Discharging",
        ChargeState::FullyCharged => "Fully charged",
        ChargeState::Empty => "Empty",
        ChargeState::PendingCharge | ChargeState::Unknown if on_ac => "Plugged in, not charging",
        ChargeState::PendingCharge | ChargeState::Unknown => "Discharging",
    }
}

/// The time until empty or full, when UPower has an estimate.
pub fn time_text(b: &Battery) -> Option<String> {
    match b.state {
        ChargeState::Discharging if b.time_to_empty > 0 => Some(format!("{} remaining", format_duration(b.time_to_empty))),
        ChargeState::Charging if b.time_to_full > 0 => Some(format!("{} until full", format_duration(b.time_to_full))),
        _ => None,
    }
}

/// The battery health line, when UPower knows the capacity.
pub fn health_text(b: &Battery) -> Option<String> {
    b.health.map(|h| format!("Battery health: {h}%"))
}

/// `secs` as hours and minutes ("2h 05m"), or minutes alone below an hour.
pub fn format_duration(secs: u64) -> String {
    let minutes = secs.div_ceil(60);
    let (h, m) = (minutes / 60, minutes % 60);
    if h == 0 {
        return format!("{m}m");
    }
    format!("{h}h {m:02}m")
}

/// What the user asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    SetProfile(Profile),
}

/// Keep only the last profile change of a burst of clicks.
pub fn coalesce(requests: Vec<Request>) -> Vec<Request> {
    requests.into_iter().last().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn battery(percent: u8, state: ChargeState) -> Battery {
        Battery { percent, state, time_to_empty: 0, time_to_full: 0, health: None }
    }

    fn model(b: Option<Battery>, on_ac: bool) -> Model {
        let mut m = Model::default();
        m.apply(WatchMsg::State(PowerState { battery: b, on_ac, ..Default::default() }));
        m
    }

    #[test]
    fn hidden_without_upower_or_without_a_battery() {
        assert!(!Model::default().is_visible());
        assert!(!model(None, true).is_visible());
        let mut m = model(Some(battery(50, ChargeState::Discharging)), false);
        assert!(m.is_visible());
        m.apply(WatchMsg::Unavailable("gone".into()));
        assert!(!m.is_visible());
    }

    #[test]
    fn the_icon_is_red_only_when_low_and_unplugged() {
        let low = model(Some(battery(LOW_PERCENT, ChargeState::Discharging)), false).icon_info();
        assert!(low.low && !low.plugged);
        let charging = model(Some(battery(5, ChargeState::Charging)), true).icon_info();
        assert!(!charging.low && charging.plugged);
        assert!(!model(Some(battery(LOW_PERCENT + 1, ChargeState::Discharging)), false).icon_info().low);
    }

    #[test]
    fn the_status_names_a_plugged_in_battery_that_does_not_charge() {
        assert_eq!(status_text(&battery(80, ChargeState::PendingCharge), true), "Plugged in, not charging");
        assert_eq!(status_text(&battery(80, ChargeState::Unknown), true), "Plugged in, not charging");
        assert_eq!(status_text(&battery(80, ChargeState::Unknown), false), "Discharging");
        assert_eq!(status_text(&battery(100, ChargeState::FullyCharged), true), "Fully charged");
        assert_eq!(status_text(&battery(40, ChargeState::Charging), true), "Charging");
    }

    #[test]
    fn the_time_follows_the_direction_of_the_charge() {
        let mut b = battery(50, ChargeState::Discharging);
        b.time_to_empty = 2 * 3600 + 5 * 60;
        b.time_to_full = 999;
        assert_eq!(time_text(&b).as_deref(), Some("2h 05m remaining"));
        b.state = ChargeState::Charging;
        b.time_to_full = 40 * 60;
        assert_eq!(time_text(&b).as_deref(), Some("40m until full"));
        b.time_to_full = 0;
        assert_eq!(time_text(&b), None);
        assert_eq!(time_text(&battery(100, ChargeState::FullyCharged)), None);
    }

    #[test]
    fn durations_round_up_to_the_minute() {
        assert_eq!(format_duration(1), "1m");
        assert_eq!(format_duration(59 * 60 + 1), "1h 00m");
        assert_eq!(format_duration(3 * 3600 + 30 * 60), "3h 30m");
    }

    #[test]
    fn the_tooltip_has_level_status_and_time() {
        let mut b = battery(42, ChargeState::Discharging);
        b.time_to_empty = 90 * 60;
        assert_eq!(model(Some(b), false).tooltip(), "Battery 42% - Discharging\n1h 30m remaining");
        assert_eq!(model(None, true).tooltip(), "No battery");
    }

    #[test]
    fn health_is_shown_only_when_known() {
        let mut b = battery(50, ChargeState::Discharging);
        assert_eq!(health_text(&b), None);
        b.health = Some(73);
        assert_eq!(health_text(&b).as_deref(), Some("Battery health: 73%"));
    }

    #[test]
    fn a_burst_of_profile_clicks_sends_the_last_one() {
        let out = coalesce(vec![Request::SetProfile(Profile::Performance), Request::SetProfile(Profile::PowerSaver)]);
        assert_eq!(out, vec![Request::SetProfile(Profile::PowerSaver)]);
        assert!(coalesce(Vec::new()).is_empty());
    }
}
