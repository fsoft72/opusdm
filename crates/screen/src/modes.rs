//! The three ways to arrange monitors at one click: extend the desktop over
//! all of them, duplicate one picture on all of them, or use only one. Pure
//! functions that give a new layout; the caller validates and applies it.

use crate::{Layout, Monitor, MonitorConfig, ScreenError};

/// A monitor set as it is in `current`, or at its default size when
/// `current` does not know it.
fn base_config(monitor: &Monitor, current: &Layout) -> Result<MonitorConfig, ScreenError> {
    if let Some(known) = current.get(&monitor.name) {
        return Ok(known.clone());
    }
    MonitorConfig::default_for(monitor).ok_or_else(|| ScreenError::Invalid(format!("{} lists no resolution", monitor.name)))
}

/// Put `config` at its default size, normal and unscaled.
fn reset(config: &mut MonitorConfig, monitor: &Monitor) -> Result<(), ScreenError> {
    let default = MonitorConfig::default_for(monitor).ok_or_else(|| ScreenError::Invalid(format!("{} lists no resolution", monitor.name)))?;
    config.enabled = true;
    config.mode = default.mode;
    config.rate = None;
    Ok(())
}

/// The name of the monitor that stays primary: the current one if it is
/// still on, else the first.
fn primary_name(configs: &[MonitorConfig]) -> Option<String> {
    configs.iter().find(|c| c.enabled && c.primary).or_else(|| configs.iter().find(|c| c.enabled)).map(|c| c.name.clone())
}

/// Set exactly one of `configs` as primary.
fn set_primary(configs: &mut [MonitorConfig], name: Option<String>) {
    for c in configs.iter_mut() {
        c.primary = Some(&c.name) == name.as_ref();
    }
}

/// Every monitor on at its preferred resolution, side by side from left to
/// right in the order they have now (tops aligned). Rotation and scale are
/// kept.
pub fn extend(monitors: &[Monitor], current: &Layout) -> Result<Layout, ScreenError> {
    let mut configs = monitors.iter().map(|m| base_config(m, current).map(|c| (c, m))).collect::<Result<Vec<_>, _>>()?;
    for (config, monitor) in &mut configs {
        if !config.enabled || current.get(&config.name).is_none() { reset(config, monitor)?; }
    }
    let mut configs: Vec<MonitorConfig> = configs.into_iter().map(|(c, _)| c).collect();
    // Monitors keep their left to right order; the ones that were off go last.
    configs.sort_by_key(|c| (!current.get(&c.name).is_some_and(|k| k.enabled), current.get(&c.name).map_or(0, |k| k.pos.0)));
    let mut x = 0;
    for config in &mut configs {
        config.pos = (x, 0);
        x += config.desktop_size().0 as i32;
    }
    let primary = primary_name(&configs);
    set_primary(&mut configs, primary);
    Ok(Layout { monitors: configs })
}

/// Every monitor on at the largest resolution they all have, all showing the
/// same picture from the corner of the desktop.
pub fn mirror(monitors: &[Monitor], current: &Layout) -> Result<Layout, ScreenError> {
    let first = monitors.first().ok_or_else(|| ScreenError::Invalid("no monitor is connected".to_string()))?;
    let common = first
        .modes
        .iter()
        .map(|m| (m.width, m.height))
        .filter(|size| monitors.iter().all(|other| other.mode(*size).is_some()))
        .max_by_key(|(w, h)| *w as u64 * *h as u64)
        .ok_or(ScreenError::NoCommonMode)?;
    let mut configs = monitors.iter().map(|m| base_config(m, current)).collect::<Result<Vec<_>, _>>()?;
    for config in &mut configs {
        config.enabled = true;
        config.mode = common;
        config.rate = None;
        config.pos = (0, 0);
        config.rotation = crate::Rotation::Normal;
        config.scale = 1.0;
    }
    let primary = primary_name(&configs);
    set_primary(&mut configs, primary);
    Ok(Layout { monitors: configs })
}

/// Only the monitor called `name` on; it becomes primary and moves to the
/// origin.
pub fn only(monitors: &[Monitor], current: &Layout, name: &str) -> Result<Layout, ScreenError> {
    if !monitors.iter().any(|m| m.name == name) {
        return Err(ScreenError::UnknownMonitor(name.to_string()));
    }
    let mut configs = monitors.iter().map(|m| base_config(m, current).map(|c| (c, m))).collect::<Result<Vec<_>, _>>()?;
    for (config, monitor) in &mut configs {
        if config.name == name {
            if !config.enabled { reset(config, monitor)?; }
            config.pos = (0, 0);
        }
        config.enabled = config.name == name;
        config.primary = config.name == name;
    }
    Ok(Layout { monitors: configs.into_iter().map(|(c, _)| c).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_support::{monitor, two_monitors};
    use crate::Rotation;

    fn current(ms: &[Monitor]) -> Layout {
        Layout { monitors: ms.iter().map(|m| MonitorConfig::default_for(m).unwrap()).collect() }
    }

    #[test]
    fn extend_places_the_monitors_side_by_side() {
        let ms = two_monitors();
        let layout = extend(&ms, &current(&ms)).unwrap();
        assert_eq!(layout.get("eDP-1").unwrap().pos, (0, 0));
        assert_eq!(layout.get("HDMI-1").unwrap().pos, (2560, 0));
        assert_eq!(layout.validate(&ms), Ok(()));
        assert!(!layout.is_mirrored());
    }

    #[test]
    fn extend_keeps_the_order_and_turns_on_what_was_off() {
        let ms = two_monitors();
        let mut cur = current(&ms);
        cur.get_mut("eDP-1").unwrap().pos = (3000, 0);
        cur.get_mut("HDMI-1").unwrap().pos = (0, 0);
        let layout = extend(&ms, &cur).unwrap();
        assert_eq!(layout.monitors[0].name, "HDMI-1");
        assert_eq!(layout.get("eDP-1").unwrap().pos, (2560, 0));

        cur.get_mut("HDMI-1").unwrap().enabled = false;
        let layout = extend(&ms, &cur).unwrap();
        assert!(layout.get("HDMI-1").unwrap().enabled);
        assert_eq!(layout.monitors[1].name, "HDMI-1", "a monitor that was off goes last");
    }

    #[test]
    fn extend_keeps_rotation_and_uses_the_turned_width() {
        let ms = two_monitors();
        let mut cur = current(&ms);
        cur.get_mut("eDP-1").unwrap().rotation = Rotation::Left;
        let layout = extend(&ms, &cur).unwrap();
        assert_eq!(layout.get("HDMI-1").unwrap().pos, (1600, 0));
    }

    #[test]
    fn extend_always_ends_with_one_primary() {
        let ms = two_monitors();
        let layout = extend(&ms, &current(&ms)).unwrap();
        assert_eq!(layout.monitors.iter().filter(|m| m.primary).count(), 1);
        let mut cur = current(&ms);
        cur.get_mut("HDMI-1").unwrap().primary = true;
        assert!(extend(&ms, &cur).unwrap().get("HDMI-1").unwrap().primary);
    }

    #[test]
    fn mirror_uses_the_largest_common_resolution_at_the_origin() {
        let ms = two_monitors();
        let layout = mirror(&ms, &current(&ms)).unwrap();
        assert!(layout.is_mirrored());
        assert!(layout.monitors.iter().all(|m| m.mode == (1920, 1080) && m.pos == (0, 0)));
        assert_eq!(layout.validate(&ms), Ok(()));
    }

    #[test]
    fn mirror_fails_without_a_common_resolution() {
        let ms = vec![monitor("A", "a", &[(1920, 1080, &[60.0])]), monitor("B", "b", &[(1280, 720, &[60.0])])];
        assert_eq!(mirror(&ms, &current(&ms)), Err(ScreenError::NoCommonMode));
    }

    #[test]
    fn only_keeps_one_monitor_on_and_makes_it_primary() {
        let ms = two_monitors();
        let layout = only(&ms, &current(&ms), "HDMI-1").unwrap();
        assert!(!layout.get("eDP-1").unwrap().enabled);
        let hdmi = layout.get("HDMI-1").unwrap();
        assert!(hdmi.enabled && hdmi.primary);
        assert_eq!(hdmi.pos, (0, 0));
        assert_eq!(layout.validate(&ms), Ok(()));
    }

    #[test]
    fn only_turns_the_monitor_on_when_it_was_off_and_rejects_unknown_names() {
        let ms = two_monitors();
        let mut cur = current(&ms);
        cur.get_mut("HDMI-1").unwrap().enabled = false;
        assert!(only(&ms, &cur, "HDMI-1").unwrap().get("HDMI-1").unwrap().enabled);
        assert_eq!(only(&ms, &cur, "DP-9"), Err(ScreenError::UnknownMonitor("DP-9".to_string())));
    }

    #[test]
    fn a_monitor_with_no_resolution_is_an_error() {
        let ms = vec![monitor("A", "a", &[])];
        assert!(matches!(extend(&ms, &Layout::default()), Err(ScreenError::Invalid(_))));
    }
}
