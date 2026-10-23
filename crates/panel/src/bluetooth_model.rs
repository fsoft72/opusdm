//! What the Bluetooth tray shows and when it notifies, as plain functions
//! of the state, so they are tested without GTK.

use opusdm_bluetooth::BtState;
use std::collections::HashMap;

/// A battery at or below this percentage is low.
pub const BATTERY_LOW: u8 = 20;

/// What the icon draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Look {
    /// No BlueZ or no adapter.
    #[default]
    Hidden,
    Off,
    On,
    /// At least one device is connected.
    Connected,
}

/// The look of the icon.
pub fn look(available: bool, s: &BtState) -> Look {
    if !available || s.adapter.is_none() { return Look::Hidden; }
    if !s.is_on() { return Look::Off; }
    if s.devices.iter().any(|d| d.connected) { return Look::Connected; }
    Look::On
}

/// The tooltip: on or off, and the connected devices.
pub fn tooltip(s: &BtState) -> String {
    if s.rfkill.hard { return "Bluetooth: blocked by a hardware switch".to_string(); }
    if !s.is_on() { return "Bluetooth: off".to_string(); }
    let connected: Vec<String> = s.paired().into_iter().filter(|d| d.connected).map(|d| match d.battery {
        Some(b) => format!("{} ({b}%)", d.name),
        None => d.name.clone(),
    }).collect();
    if connected.is_empty() { return "Bluetooth: on, no device connected".to_string(); }
    format!("Bluetooth: {}", connected.join(", "))
}

/// A desktop notification to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub summary: String,
    pub body: String,
}

/// What the notifier remembers of a paired device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    connected: bool,
    /// The low battery note was sent and the battery did not go up since.
    low: bool,
}

/// Turns state changes into notes: a paired device connected or
/// disconnected, a battery going low (once per crossing). The first state
/// only sets what is known: nothing is notified for it.
#[derive(Default)]
pub struct Notifier {
    seen: Option<HashMap<String, Seen>>,
}

impl Notifier {
    /// The notes for the move to `s`.
    pub fn update(&mut self, s: &BtState) -> Vec<Note> {
        let first = self.seen.is_none();
        let old = self.seen.take().unwrap_or_default();
        let mut now = HashMap::new();
        let mut notes = Vec::new();
        for d in s.devices.iter().filter(|d| d.paired) {
            let was = old.get(&d.path).copied();
            let is_low = d.connected && d.battery.is_some_and(|b| b <= BATTERY_LOW);
            if !first {
                match was {
                    Some(w) if w.connected != d.connected => notes.push(Note {
                        summary: d.name.clone(),
                        body: if d.connected { "Connected".to_string() } else { "Disconnected".to_string() },
                    }),
                    None if d.connected => notes.push(Note { summary: d.name.clone(), body: "Connected".to_string() }),
                    _ => {}
                }
                if is_low && !was.is_some_and(|w| w.low) {
                    notes.push(Note { summary: d.name.clone(), body: format!("Battery low: {}%", d.battery.unwrap_or(0)) });
                }
            }
            // A battery already low at start counts as noted: no note pops
            // up when the panel starts.
            now.insert(d.path.clone(), Seen { connected: d.connected, low: is_low });
        }
        self.seen = Some(now);
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_bluetooth::model::test_device;
    use opusdm_bluetooth::{Adapter, Rfkill};

    /// A powered adapter with `devices`.
    fn on(devices: Vec<opusdm_bluetooth::Device>) -> BtState {
        BtState { adapter: Some(Adapter { powered: true, ..Default::default() }), devices, rfkill: Rfkill::default() }
    }

    #[test]
    fn icon_follows_service_adapter_and_devices() {
        assert_eq!(look(false, &on(vec![])), Look::Hidden);
        assert_eq!(look(true, &BtState::default()), Look::Hidden);
        let mut off = on(vec![]);
        off.adapter.as_mut().unwrap().powered = false;
        assert_eq!(look(true, &off), Look::Off);
        assert_eq!(look(true, &on(vec![test_device("/k", "Keys", true, false)])), Look::On);
        assert_eq!(look(true, &on(vec![test_device("/k", "Keys", true, true)])), Look::Connected);
    }

    #[test]
    fn tooltip_names_connected_devices() {
        let mut k = test_device("/k", "Keys", true, true);
        k.battery = Some(60);
        assert_eq!(tooltip(&on(vec![k, test_device("/m", "Mouse", true, false)])), "Bluetooth: Keys (60%)");
        assert_eq!(tooltip(&on(vec![])), "Bluetooth: on, no device connected");
        assert_eq!(tooltip(&BtState::default()), "Bluetooth: off");
    }

    #[test]
    fn first_state_sends_no_notes() {
        let mut k = test_device("/k", "Keys", true, true);
        k.battery = Some(5);
        let mut n = Notifier::default();
        assert!(n.update(&on(vec![k.clone()])).is_empty());
        // Still low: no note either.
        assert!(n.update(&on(vec![k])).is_empty());
    }

    #[test]
    fn connect_and_disconnect_notes() {
        let mut n = Notifier::default();
        n.update(&on(vec![test_device("/k", "Keys", true, false), test_device("/x", "Stranger", false, false)]));
        assert_eq!(n.update(&on(vec![test_device("/k", "Keys", true, true)])), vec![Note { summary: "Keys".into(), body: "Connected".into() }]);
        assert_eq!(n.update(&on(vec![test_device("/k", "Keys", true, false)])), vec![Note { summary: "Keys".into(), body: "Disconnected".into() }]);
        // A device just paired and connected.
        assert_eq!(n.update(&on(vec![test_device("/k", "Keys", true, false), test_device("/p", "Pixel", true, true)])), vec![Note { summary: "Pixel".into(), body: "Connected".into() }]);
        // A device that is not paired never notifies.
        assert!(n.update(&on(vec![test_device("/k", "Keys", true, false), test_device("/p", "Pixel", true, true), test_device("/x", "Stranger", false, true)])).is_empty());
    }

    #[test]
    fn battery_low_once_per_crossing() {
        let dev = |b: u8| {
            let mut d = test_device("/k", "Keys", true, true);
            d.battery = Some(b);
            d
        };
        let mut n = Notifier::default();
        n.update(&on(vec![dev(50)]));
        assert_eq!(n.update(&on(vec![dev(20)])), vec![Note { summary: "Keys".into(), body: "Battery low: 20%".into() }]);
        assert!(n.update(&on(vec![dev(15)])).is_empty());
        assert!(n.update(&on(vec![dev(60)])).is_empty());
        assert_eq!(n.update(&on(vec![dev(10)])).len(), 1);
    }
}
