//! Turns UPower and power-profiles-daemon properties into the model, with no
//! bus access.

use crate::model::{Battery, ChargeState, Profile};
use std::collections::HashMap;
use zbus::zvariant::OwnedValue;

/// `UP_DEVICE_KIND_BATTERY`.
pub const KIND_BATTERY: u32 = 2;

pub type Props = HashMap<String, OwnedValue>;

/// A `f64` property, 0 when missing or of another type.
fn get_f64(props: &Props, key: &str) -> f64 {
    props.get(key).and_then(|v| f64::try_from(v.clone()).ok()).unwrap_or(0.0)
}

/// A `u32` property, 0 when missing or of another type.
fn get_u32(props: &Props, key: &str) -> u32 {
    props.get(key).and_then(|v| u32::try_from(v.clone()).ok()).unwrap_or(0)
}

/// A `bool` property, false when missing or of another type.
pub fn get_bool(props: &Props, key: &str) -> bool {
    props.get(key).and_then(|v| bool::try_from(v.clone()).ok()).unwrap_or(false)
}

/// A seconds property (`x`), 0 when missing, negative or of another type.
fn get_secs(props: &Props, key: &str) -> u64 {
    props.get(key).and_then(|v| i64::try_from(v.clone()).ok()).map_or(0, |s| s.max(0) as u64)
}

/// A percentage rounded to 0..=100.
fn to_percent(value: f64) -> u8 {
    value.round().clamp(0.0, 100.0) as u8
}

/// Whether a `GetAll` reply of a UPower device is a battery that powers the machine.
pub fn is_system_battery(props: &Props) -> bool {
    get_u32(props, "Type") == KIND_BATTERY && get_bool(props, "PowerSupply")
}

/// The health of one battery device: its `Capacity`, when UPower knows it.
pub fn health_of(props: &Props) -> Option<u8> {
    let capacity = get_f64(props, "Capacity");
    (capacity > 0.0).then(|| to_percent(capacity))
}

/// The battery from the `GetAll` reply of UPower's display device, or `None`
/// when there is no battery. `health` comes from the battery device itself.
pub fn battery_from(display: &Props, health: Option<u8>) -> Option<Battery> {
    if !get_bool(display, "IsPresent") || get_u32(display, "Type") != KIND_BATTERY {
        return None;
    }
    Some(Battery {
        percent: to_percent(get_f64(display, "Percentage")),
        state: ChargeState::from_upower(get_u32(display, "State")),
        time_to_empty: get_secs(display, "TimeToEmpty"),
        time_to_full: get_secs(display, "TimeToFull"),
        health,
    })
}

/// The profiles in the daemon's `Profiles` list, in `Profile::ALL` order.
/// Names this tray does not know are skipped.
pub fn profiles_from(list: &[Props]) -> Vec<Profile> {
    let mut out: Vec<Profile> = list
        .iter()
        .filter_map(|p| p.get("Profile").and_then(|v| String::try_from(v.clone()).ok()))
        .filter_map(|id| Profile::from_id(&id))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn props(pairs: Vec<(&str, Value<'static>)>) -> Props {
        pairs.into_iter().map(|(k, v)| (k.to_string(), OwnedValue::try_from(v).unwrap())).collect()
    }

    fn display(percent: f64, state: u32) -> Props {
        props(vec![
            ("IsPresent", Value::from(true)),
            ("Type", Value::from(KIND_BATTERY)),
            ("Percentage", Value::from(percent)),
            ("State", Value::from(state)),
            ("TimeToEmpty", Value::from(5400i64)),
            ("TimeToFull", Value::from(0i64)),
        ])
    }

    #[test]
    fn the_display_device_becomes_a_battery() {
        let b = battery_from(&display(92.6, 2), Some(73)).unwrap();
        assert_eq!(b.percent, 93);
        assert_eq!(b.state, ChargeState::Discharging);
        assert_eq!(b.time_to_empty, 5400);
        assert_eq!(b.time_to_full, 0);
        assert_eq!(b.health, Some(73));
    }

    #[test]
    fn no_battery_when_absent_or_not_a_battery() {
        let mut d = display(50.0, 2);
        d.insert("IsPresent".into(), OwnedValue::from(false));
        assert!(battery_from(&d, None).is_none());
        let mut d = display(50.0, 2);
        d.insert("Type".into(), OwnedValue::from(0u32));
        assert!(battery_from(&d, None).is_none());
    }

    #[test]
    fn a_negative_time_reads_as_unknown() {
        let mut d = display(50.0, 2);
        d.insert("TimeToEmpty".into(), OwnedValue::from(-1i64));
        assert_eq!(battery_from(&d, None).unwrap().time_to_empty, 0);
    }

    #[test]
    fn health_is_the_capacity_when_known() {
        let p = props(vec![("Capacity", Value::from(72.58))]);
        assert_eq!(health_of(&p), Some(73));
        assert_eq!(health_of(&props(vec![("Capacity", Value::from(0.0))])), None);
        assert_eq!(health_of(&Props::new()), None);
    }

    #[test]
    fn only_a_battery_that_powers_the_machine_counts() {
        let bat = props(vec![("Type", Value::from(KIND_BATTERY)), ("PowerSupply", Value::from(true))]);
        let mouse = props(vec![("Type", Value::from(5u32)), ("PowerSupply", Value::from(false))]);
        let ups = props(vec![("Type", Value::from(KIND_BATTERY)), ("PowerSupply", Value::from(false))]);
        assert!(is_system_battery(&bat));
        assert!(!is_system_battery(&mouse));
        assert!(!is_system_battery(&ups));
    }

    #[test]
    fn profiles_are_sorted_and_unknown_ones_skipped() {
        let entry = |id: &str| props(vec![("Profile", Value::from(id.to_string())), ("Driver", Value::from("x".to_string()))]);
        let list = vec![entry("performance"), entry("turbo"), entry("power-saver"), entry("balanced")];
        assert_eq!(profiles_from(&list), vec![Profile::PowerSaver, Profile::Balanced, Profile::Performance]);
    }
}
