//! The events that turn one device list into the next.

use crate::model::{AudioDevice, Kind};

/// A change of the device list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioEvent {
    Added(AudioDevice),
    Removed { kind: Kind, index: u32 },
    /// Volume, mute or default flag changed.
    Changed(AudioDevice),
}

/// The events that take `old` to `now`: removals first, then additions and changes.
pub fn diff(old: &[AudioDevice], now: &[AudioDevice]) -> Vec<AudioEvent> {
    let mut out = Vec::new();
    for d in old {
        if !now.iter().any(|n| n.key() == d.key()) {
            out.push(AudioEvent::Removed { kind: d.kind, index: d.index });
        }
    }
    for n in now {
        match old.iter().find(|d| d.key() == n.key()) {
            None => out.push(AudioEvent::Added(n.clone())),
            Some(o) if o != n => out.push(AudioEvent::Changed(n.clone())),
            Some(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(kind: Kind, index: u32, percent: u32, is_default: bool) -> AudioDevice {
        AudioDevice {
            index,
            name: format!("dev{index}"),
            description: format!("Device {index}"),
            kind,
            volume_percent: percent,
            muted: false,
            is_default,
        }
    }

    #[test]
    fn nothing_changed_nothing_reported() {
        let list = vec![dev(Kind::Output, 1, 50, true)];
        assert!(diff(&list, &list).is_empty());
    }

    #[test]
    fn a_new_and_a_gone_device() {
        let old = vec![dev(Kind::Output, 1, 50, true)];
        let now = vec![dev(Kind::Output, 2, 50, true)];
        let ev = diff(&old, &now);
        assert_eq!(ev, vec![AudioEvent::Removed { kind: Kind::Output, index: 1 }, AudioEvent::Added(now[0].clone())]);
    }

    #[test]
    fn a_volume_change_is_one_changed_event() {
        let old = vec![dev(Kind::Input, 4, 50, true)];
        let now = vec![dev(Kind::Input, 4, 60, true)];
        assert_eq!(diff(&old, &now), vec![AudioEvent::Changed(now[0].clone())]);
    }

    #[test]
    fn switching_the_default_changes_both_devices() {
        let old = vec![dev(Kind::Output, 1, 50, true), dev(Kind::Output, 2, 50, false)];
        let now = vec![dev(Kind::Output, 1, 50, false), dev(Kind::Output, 2, 50, true)];
        assert_eq!(diff(&old, &now).len(), 2);
    }

    #[test]
    fn an_output_and_an_input_with_the_same_index_are_different_devices() {
        let old = vec![dev(Kind::Output, 3, 50, true)];
        let now = vec![dev(Kind::Output, 3, 50, true), dev(Kind::Input, 3, 50, true)];
        assert_eq!(diff(&old, &now), vec![AudioEvent::Added(now[1].clone())]);
    }
}
