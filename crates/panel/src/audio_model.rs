//! The audio devices as the panel sees them: pure state, no GTK.

use opusdm_audio::{AudioDevice, AudioEvent, Kind, WatchMsg};
use std::time::{Duration, Instant};

/// How long after the last slider change a server event may not move the slider.
pub const HOLD_MS: u64 = 400;
/// Volumes below this are drawn with one wave.
const LOW_BELOW: u32 = 34;
/// Volumes below this are drawn with two waves.
const MEDIUM_BELOW: u32 = 67;

/// Which speaker symbol the panel icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconLevel {
    Muted,
    Low,
    Medium,
    High,
}

/// Every device, and whether the sound server is reachable.
#[derive(Default)]
pub struct Model {
    devices: Vec<AudioDevice>,
    available: bool,
}

impl Model {
    /// Apply one message of the watcher.
    pub fn apply(&mut self, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(_) => {}
            WatchMsg::Unavailable(_) => {
                self.devices.clear();
                self.available = false;
            }
            WatchMsg::Snapshot(list) => {
                self.devices = list;
                self.devices.sort_by_key(|d| (d.kind, d.index));
                self.available = true;
            }
            WatchMsg::Event(AudioEvent::Added(d)) => {
                if self.devices.iter().any(|x| x.key() == d.key()) {
                    return;
                }
                self.devices.push(d);
                self.devices.sort_by_key(|d| (d.kind, d.index));
            }
            WatchMsg::Event(AudioEvent::Changed(d)) => {
                if let Some(slot) = self.devices.iter_mut().find(|x| x.key() == d.key()) {
                    *slot = d;
                }
            }
            WatchMsg::Event(AudioEvent::Removed { kind, index }) => {
                self.devices.retain(|d| d.key() != (kind, index));
            }
        }
    }

    /// Whether the sound server is reachable.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// The devices of one kind, by index.
    pub fn devices_of(&self, kind: Kind) -> Vec<AudioDevice> {
        self.devices.iter().filter(|d| d.kind == kind).cloned().collect()
    }

    /// The default device of one kind.
    pub fn default_of(&self, kind: Kind) -> Option<&AudioDevice> {
        self.devices.iter().find(|d| d.kind == kind && d.is_default)
    }

    /// The icon symbol for the default output.
    pub fn icon_level(&self) -> IconLevel {
        let Some(d) = self.default_of(Kind::Output) else { return IconLevel::Muted };
        if d.muted || d.volume_percent == 0 {
            return IconLevel::Muted;
        }
        match d.volume_percent {
            p if p < LOW_BELOW => IconLevel::Low,
            p if p < MEDIUM_BELOW => IconLevel::Medium,
            _ => IconLevel::High,
        }
    }

    /// The icon tooltip: the default output's volume, "Muted", or why there is none.
    pub fn tooltip(&self) -> String {
        if !self.available {
            return "No sound server".to_string();
        }
        match self.default_of(Kind::Output) {
            None => "No output device".to_string(),
            Some(d) if d.muted => "Muted".to_string(),
            Some(d) => format!("Volume {}%", d.volume_percent),
        }
    }
}

/// Remembers when the user last moved a slider.
#[derive(Default)]
pub struct DragGuard {
    last: Option<Instant>,
}

impl DragGuard {
    /// The user moved the slider at `now`.
    pub fn touch(&mut self, now: Instant) {
        self.last = Some(now);
    }

    /// Whether a server event at `now` must leave the slider alone.
    pub fn is_held(&self, now: Instant) -> bool {
        self.last.is_some_and(|t| now.duration_since(t) < Duration::from_millis(HOLD_MS))
    }
}

/// What the user asked for. The `Set` prefix mirrors the `Backend` methods.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Request {
    SetVolume { kind: Kind, index: u32, percent: u32 },
    SetMute { kind: Kind, index: u32, muted: bool },
    SetDefault { kind: Kind, name: String },
}

/// Drop every volume request that a later one for the same device replaces.
/// The order of everything else is kept.
pub fn coalesce(requests: Vec<Request>) -> Vec<Request> {
    let mut out = Vec::new();
    for (i, r) in requests.iter().enumerate() {
        if let Request::SetVolume { kind, index, .. } = r {
            let replaced = requests[i + 1..]
                .iter()
                .any(|n| matches!(n, Request::SetVolume { kind: k, index: x, .. } if k == kind && x == index));
            if replaced {
                continue;
            }
        }
        out.push(r.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(kind: Kind, index: u32, percent: u32, muted: bool, is_default: bool) -> AudioDevice {
        AudioDevice {
            index,
            name: format!("dev{index}"),
            description: format!("Device {index}"),
            kind,
            volume_percent: percent,
            muted,
            is_default,
        }
    }

    fn snapshot(list: Vec<AudioDevice>) -> WatchMsg {
        WatchMsg::Snapshot(list)
    }

    #[test]
    fn nothing_is_available_before_the_first_snapshot() {
        let m = Model::default();
        assert!(!m.is_available());
        assert_eq!(m.tooltip(), "No sound server");
    }

    #[test]
    fn a_snapshot_makes_it_available_and_unavailable_clears_it() {
        let mut m = Model::default();
        m.apply(snapshot(vec![dev(Kind::Output, 1, 50, false, true)]));
        assert!(m.is_available());
        m.apply(WatchMsg::Unavailable("gone".into()));
        assert!(!m.is_available());
        assert!(m.devices_of(Kind::Output).is_empty());
    }

    #[test]
    fn a_new_snapshot_after_an_outage_replaces_the_list() {
        let mut m = Model::default();
        m.apply(snapshot(vec![dev(Kind::Output, 1, 50, false, true)]));
        m.apply(WatchMsg::Unavailable("gone".into()));
        m.apply(snapshot(vec![dev(Kind::Output, 7, 20, false, true)]));
        assert!(m.is_available());
        assert_eq!(m.devices_of(Kind::Output)[0].index, 7);
    }

    #[test]
    fn events_add_change_and_remove() {
        let mut m = Model::default();
        m.apply(snapshot(vec![]));
        m.apply(WatchMsg::Event(AudioEvent::Added(dev(Kind::Output, 2, 30, false, true))));
        m.apply(WatchMsg::Event(AudioEvent::Added(dev(Kind::Output, 2, 30, false, true))));
        assert_eq!(m.devices_of(Kind::Output).len(), 1, "an Added twice is one device");
        m.apply(WatchMsg::Event(AudioEvent::Changed(dev(Kind::Output, 2, 80, false, true))));
        assert_eq!(m.devices_of(Kind::Output)[0].volume_percent, 80);
        m.apply(WatchMsg::Event(AudioEvent::Removed { kind: Kind::Output, index: 2 }));
        assert!(m.devices_of(Kind::Output).is_empty());
    }

    #[test]
    fn a_changed_for_an_unknown_device_is_ignored() {
        let mut m = Model::default();
        m.apply(snapshot(vec![]));
        m.apply(WatchMsg::Event(AudioEvent::Changed(dev(Kind::Input, 5, 80, false, true))));
        assert!(m.devices_of(Kind::Input).is_empty());
    }

    #[test]
    fn removing_the_default_then_the_server_picks_another() {
        let mut m = Model::default();
        m.apply(snapshot(vec![dev(Kind::Output, 1, 50, false, true), dev(Kind::Output, 2, 50, false, false)]));
        m.apply(WatchMsg::Event(AudioEvent::Removed { kind: Kind::Output, index: 1 }));
        assert!(m.default_of(Kind::Output).is_none());
        m.apply(WatchMsg::Event(AudioEvent::Changed(dev(Kind::Output, 2, 50, false, true))));
        assert_eq!(m.default_of(Kind::Output).unwrap().index, 2);
        assert_eq!(m.devices_of(Kind::Output).iter().filter(|d| d.is_default).count(), 1);
    }

    #[test]
    fn devices_are_listed_by_kind_in_index_order() {
        let mut m = Model::default();
        m.apply(snapshot(vec![
            dev(Kind::Input, 1, 50, false, true),
            dev(Kind::Output, 9, 50, false, false),
            dev(Kind::Output, 3, 50, false, true),
        ]));
        let outs: Vec<u32> = m.devices_of(Kind::Output).iter().map(|d| d.index).collect();
        assert_eq!(outs, vec![3, 9]);
        assert_eq!(m.devices_of(Kind::Input).len(), 1);
    }

    #[test]
    fn the_icon_follows_the_default_output() {
        let level = |p: u32, muted: bool| {
            let mut m = Model::default();
            m.apply(snapshot(vec![dev(Kind::Output, 1, p, muted, true)]));
            m.icon_level()
        };
        assert_eq!(level(0, false), IconLevel::Muted);
        assert_eq!(level(80, true), IconLevel::Muted);
        assert_eq!(level(20, false), IconLevel::Low);
        assert_eq!(level(50, false), IconLevel::Medium);
        assert_eq!(level(90, false), IconLevel::High);
        assert_eq!(Model::default().icon_level(), IconLevel::Muted);
    }

    #[test]
    fn the_tooltip_shows_the_real_percentage_even_above_100() {
        let mut m = Model::default();
        m.apply(snapshot(vec![dev(Kind::Output, 1, 130, false, true)]));
        assert_eq!(m.tooltip(), "Volume 130%");
        m.apply(WatchMsg::Event(AudioEvent::Changed(dev(Kind::Output, 1, 130, true, true))));
        assert_eq!(m.tooltip(), "Muted");
    }

    #[test]
    fn the_drag_guard_holds_for_a_short_time_after_a_touch() {
        let mut g = DragGuard::default();
        let t0 = Instant::now();
        assert!(!g.is_held(t0));
        g.touch(t0);
        assert!(g.is_held(t0 + Duration::from_millis(HOLD_MS - 1)));
        assert!(!g.is_held(t0 + Duration::from_millis(HOLD_MS + 1)));
    }

    #[test]
    fn coalesce_keeps_only_the_last_volume_per_device() {
        let v = |index, percent| Request::SetVolume { kind: Kind::Output, index, percent };
        let out = coalesce(vec![v(1, 10), v(2, 5), v(1, 20), v(1, 30)]);
        assert_eq!(out, vec![v(2, 5), v(1, 30)]);
    }

    #[test]
    fn coalesce_never_drops_a_mute_or_a_default_change() {
        let reqs = vec![
            Request::SetVolume { kind: Kind::Input, index: 1, percent: 10 },
            Request::SetMute { kind: Kind::Input, index: 1, muted: true },
            Request::SetDefault { kind: Kind::Output, name: "a".into() },
            Request::SetVolume { kind: Kind::Input, index: 1, percent: 12 },
        ];
        let out = coalesce(reqs);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], Request::SetMute { .. }));
        assert!(matches!(out[2], Request::SetVolume { percent: 12, .. }));
    }

    #[test]
    fn an_output_and_an_input_volume_with_the_same_index_are_not_merged() {
        let out = coalesce(vec![
            Request::SetVolume { kind: Kind::Output, index: 1, percent: 10 },
            Request::SetVolume { kind: Kind::Input, index: 1, percent: 20 },
        ]);
        assert_eq!(out.len(), 2);
    }
}
