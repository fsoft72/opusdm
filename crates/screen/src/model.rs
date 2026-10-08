//! Plain data of the screen manager: what a monitor can do (`Monitor`), how it
//! is set (`MonitorConfig`) and the set of all of them (`Layout`).

use serde::{Deserialize, Serialize};

use crate::ScreenError;

/// Two refresh rates closer than this (Hz) are the same rate.
const RATE_EPSILON: f64 = 0.01;

/// Whether two refresh rates are the same one.
pub fn same_rate(a: f64, b: f64) -> bool {
    (a - b).abs() < RATE_EPSILON
}

/// How a monitor is turned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rotation {
    #[default]
    Normal,
    Left,
    Right,
    Inverted,
}

impl Rotation {
    /// Every rotation, in the order shown to the user.
    pub const ALL: [Rotation; 4] = [Rotation::Normal, Rotation::Left, Rotation::Right, Rotation::Inverted];

    /// The word `xrandr --rotate` takes.
    pub fn xrandr_arg(self) -> &'static str {
        match self {
            Rotation::Normal => "normal",
            Rotation::Left => "left",
            Rotation::Right => "right",
            Rotation::Inverted => "inverted",
        }
    }

    /// The rotation named by an `xrandr` word, if it is one.
    pub fn from_xrandr(word: &str) -> Option<Rotation> {
        Rotation::ALL.into_iter().find(|r| r.xrandr_arg() == word)
    }

    /// Whether width and height swap (a monitor on its side).
    pub fn is_sideways(self) -> bool {
        matches!(self, Rotation::Left | Rotation::Right)
    }

    /// The name shown in the window.
    pub fn label(self) -> &'static str {
        match self {
            Rotation::Normal => "Normal",
            Rotation::Left => "Left",
            Rotation::Right => "Right",
            Rotation::Inverted => "Upside down",
        }
    }
}

/// A resolution and the refresh rates it comes with.
#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    /// Refresh rates in Hz, as `xrandr` lists them (highest first).
    pub rates: Vec<f64>,
}

/// What a connected monitor can do.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// The `xrandr` output name, e.g. `HDMI-1`.
    pub name: String,
    /// A stable id taken from the EDID (the output name when there is none).
    pub edid_id: String,
    /// Resolutions, in the order `xrandr` lists them (largest first).
    pub modes: Vec<Mode>,
    /// The resolution the monitor prefers.
    pub preferred: Option<(u32, u32)>,
}

impl Monitor {
    /// The mode with this resolution.
    pub fn mode(&self, size: (u32, u32)) -> Option<&Mode> {
        self.modes.iter().find(|m| (m.width, m.height) == size)
    }

    /// The resolution to use when nothing else is asked: the preferred one,
    /// or the first (largest) one.
    pub fn default_size(&self) -> Option<(u32, u32)> {
        self.preferred.filter(|p| self.mode(*p).is_some()).or_else(|| self.modes.first().map(|m| (m.width, m.height)))
    }
}

/// How one monitor is set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MonitorConfig {
    /// The `xrandr` output name.
    pub name: String,
    /// The EDID id of the monitor (see `Monitor::edid_id`).
    pub edid_id: String,
    /// Whether the monitor is on.
    pub enabled: bool,
    /// The resolution (before rotation).
    pub mode: (u32, u32),
    /// The refresh rate in Hz; `None` lets `xrandr` choose.
    pub rate: Option<f64>,
    /// The top left corner on the shared desktop.
    pub pos: (i32, i32),
    pub rotation: Rotation,
    /// The `xrandr --scale` factor: 2.0 draws the desktop twice as large and
    /// shrinks it to the screen (things look smaller), 0.5 the opposite.
    pub scale: f64,
    pub primary: bool,
}

impl MonitorConfig {
    /// A monitor set to its default resolution at the origin, or `None` when
    /// it lists no resolution.
    pub fn default_for(monitor: &Monitor) -> Option<MonitorConfig> {
        Some(MonitorConfig {
            name: monitor.name.clone(),
            edid_id: monitor.edid_id.clone(),
            enabled: true,
            mode: monitor.default_size()?,
            rate: None,
            pos: (0, 0),
            rotation: Rotation::Normal,
            scale: 1.0,
            primary: false,
        })
    }

    /// The size this monitor takes on the desktop: the resolution, scaled and
    /// turned.
    pub fn desktop_size(&self) -> (u32, u32) {
        let w = (self.mode.0 as f64 * self.scale).round() as u32;
        let h = (self.mode.1 as f64 * self.scale).round() as u32;
        if self.rotation.is_sideways() { (h, w) } else { (w, h) }
    }
}

/// The settings of all the connected monitors.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub monitors: Vec<MonitorConfig>,
}

impl Layout {
    /// The setting of the monitor called `name`.
    pub fn get(&self, name: &str) -> Option<&MonitorConfig> {
        self.monitors.iter().find(|m| m.name == name)
    }

    /// The setting of the monitor called `name`, to change it.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut MonitorConfig> {
        self.monitors.iter_mut().find(|m| m.name == name)
    }

    /// The monitors that are on.
    pub fn enabled(&self) -> impl Iterator<Item = &MonitorConfig> {
        self.monitors.iter().filter(|m| m.enabled)
    }

    /// Whether several monitors are on and all show the same corner of the
    /// desktop (duplicated screens).
    pub fn is_mirrored(&self) -> bool {
        let mut on = self.enabled();
        let Some(first) = on.next() else { return false };
        let mut others = on.peekable();
        others.peek().is_some() && others.all(|m| m.pos == first.pos)
    }

    /// The same layout moved so the monitors that are on start at 0,0 (X has
    /// no negative positions). Monitors that are off do not count.
    pub fn shifted_to_origin(&self) -> Layout {
        let min_x = self.enabled().map(|m| m.pos.0).min().unwrap_or(0);
        let min_y = self.enabled().map(|m| m.pos.1).min().unwrap_or(0);
        let mut shifted = self.clone();
        for m in shifted.monitors.iter_mut().filter(|m| m.enabled) {
            m.pos = (m.pos.0 - min_x, m.pos.1 - min_y);
        }
        shifted
    }

    /// Check the layout against the monitors it is for: at least one monitor
    /// is on, at most one is primary, and every mode and rate exists.
    pub fn validate(&self, monitors: &[Monitor]) -> Result<(), ScreenError> {
        if self.enabled().next().is_none() {
            return Err(ScreenError::Invalid("at least one monitor must stay on".to_string()));
        }
        if self.enabled().filter(|m| m.primary).count() > 1 {
            return Err(ScreenError::Invalid("only one monitor can be the primary one".to_string()));
        }
        for config in self.enabled() {
            let monitor = monitors.iter().find(|m| m.name == config.name).ok_or_else(|| ScreenError::UnknownMonitor(config.name.clone()))?;
            let mode = monitor
                .mode(config.mode)
                .ok_or_else(|| ScreenError::Invalid(format!("{} has no {}x{} resolution", config.name, config.mode.0, config.mode.1)))?;
            if let Some(rate) = config.rate {
                if !mode.rates.iter().any(|r| same_rate(*r, rate)) {
                    return Err(ScreenError::Invalid(format!("{} has no {rate:.2} Hz rate at {}x{}", config.name, config.mode.0, config.mode.1)));
                }
            }
            if config.scale <= 0.0 {
                return Err(ScreenError::Invalid(format!("{} has a scale of {}", config.name, config.scale)));
            }
        }
        Ok(())
    }

    /// This layout for the monitors that are connected now: a setting finds
    /// its monitor by output name and EDID id, or by EDID id alone (the
    /// monitor sits on another port). `None` when a monitor of the layout is
    /// missing or the result is not valid.
    pub fn remapped_to(&self, monitors: &[Monitor]) -> Option<Layout> {
        if self.monitors.len() != monitors.len() { return None; }
        let mut taken = vec![false; monitors.len()];
        let mut result = self.clone();
        let find = |taken: &[bool], pick: &dyn Fn(&Monitor) -> bool| monitors.iter().enumerate().find(|(i, m)| !taken[*i] && pick(m)).map(|(i, _)| i);
        for config in &mut result.monitors {
            let index = find(&taken, &|m| m.name == config.name && m.edid_id == config.edid_id).or_else(|| find(&taken, &|m| m.edid_id == config.edid_id))?;
            taken[index] = true;
            config.name = monitors[index].name.clone();
        }
        result.validate(monitors).ok()?;
        Some(result)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    pub use crate::fake::{monitor, two_monitors};
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn config(monitor: &Monitor, x: i32) -> MonitorConfig {
        MonitorConfig { pos: (x, 0), ..MonitorConfig::default_for(monitor).unwrap() }
    }

    #[test]
    fn desktop_size_scales_and_turns() {
        let m = monitor("a", "a", &[(1920, 1080, &[60.0])]);
        let mut c = config(&m, 0);
        assert_eq!(c.desktop_size(), (1920, 1080));
        c.scale = 2.0;
        assert_eq!(c.desktop_size(), (3840, 2160));
        c.rotation = Rotation::Left;
        assert_eq!(c.desktop_size(), (2160, 3840));
    }

    #[test]
    fn validate_accepts_a_good_layout() {
        let ms = two_monitors();
        let layout = Layout { monitors: vec![config(&ms[0], 0), config(&ms[1], 2560)] };
        assert_eq!(layout.validate(&ms), Ok(()));
    }

    #[test]
    fn validate_rejects_everything_off() {
        let ms = two_monitors();
        let mut layout = Layout { monitors: vec![config(&ms[0], 0)] };
        layout.monitors[0].enabled = false;
        assert!(matches!(layout.validate(&ms), Err(ScreenError::Invalid(_))));
    }

    #[test]
    fn validate_rejects_two_primaries() {
        let ms = two_monitors();
        let mut layout = Layout { monitors: vec![config(&ms[0], 0), config(&ms[1], 2560)] };
        layout.monitors[0].primary = true;
        layout.monitors[1].primary = true;
        assert!(matches!(layout.validate(&ms), Err(ScreenError::Invalid(_))));
    }

    #[test]
    fn validate_rejects_a_missing_mode_and_rate() {
        let ms = two_monitors();
        let mut layout = Layout { monitors: vec![config(&ms[0], 0)] };
        layout.monitors[0].mode = (800, 600);
        assert!(matches!(layout.validate(&ms), Err(ScreenError::Invalid(_))));
        layout.monitors[0].mode = (2560, 1600);
        layout.monitors[0].rate = Some(144.0);
        assert!(matches!(layout.validate(&ms), Err(ScreenError::Invalid(_))));
        layout.monitors[0].rate = Some(60.001);
        assert_eq!(layout.validate(&ms), Ok(()));
    }

    #[test]
    fn validate_rejects_an_unknown_monitor() {
        let ms = two_monitors();
        let mut layout = Layout { monitors: vec![config(&ms[0], 0)] };
        layout.monitors[0].name = "DP-9".to_string();
        assert_eq!(layout.validate(&ms), Err(ScreenError::UnknownMonitor("DP-9".to_string())));
    }

    #[test]
    fn mirrored_means_several_monitors_on_the_same_corner() {
        let ms = two_monitors();
        let mut layout = Layout { monitors: vec![config(&ms[0], 0), config(&ms[1], 2560)] };
        assert!(!layout.is_mirrored());
        layout.monitors[1].pos = (0, 0);
        assert!(layout.is_mirrored());
        layout.monitors[1].enabled = false;
        assert!(!layout.is_mirrored());
    }

    #[test]
    fn remap_follows_a_monitor_to_another_port() {
        let ms = two_monitors();
        let layout = Layout { monitors: vec![config(&ms[0], 0), config(&ms[1], 2560)] };
        let mut moved = ms.clone();
        moved[1].name = "DP-2".to_string();
        let remapped = layout.remapped_to(&moved).unwrap();
        assert_eq!(remapped.monitors[1].name, "DP-2");
        assert_eq!(remapped.monitors[1].pos, (2560, 0));
    }

    #[test]
    fn remap_fails_when_a_monitor_is_missing() {
        let ms = two_monitors();
        let layout = Layout { monitors: vec![config(&ms[0], 0), config(&ms[1], 2560)] };
        assert_eq!(layout.remapped_to(&ms[..1]), None);
        let mut other = ms.clone();
        other[1].edid_id = "cccc".to_string();
        assert_eq!(layout.remapped_to(&other), None);
    }

    #[test]
    fn shifted_to_origin_moves_the_monitors_that_are_on() {
        let ms = two_monitors();
        let mut layout = Layout { monitors: vec![config(&ms[0], -1920), config(&ms[1], 640)] };
        layout.monitors[0].pos.1 = -100;
        let shifted = layout.shifted_to_origin();
        assert_eq!(shifted.monitors[0].pos, (0, 0));
        assert_eq!(shifted.monitors[1].pos, (2560, 100));
        layout.monitors[0].enabled = false;
        assert_eq!(layout.shifted_to_origin().monitors[1].pos, (0, 0));
        assert_eq!(layout.shifted_to_origin().monitors[0].pos, (-1920, -100), "a monitor that is off is left alone");
    }

    #[test]
    fn rotation_round_trips_through_xrandr_words() {
        for r in Rotation::ALL {
            assert_eq!(Rotation::from_xrandr(r.xrandr_arg()), Some(r));
        }
        assert_eq!(Rotation::from_xrandr("x"), None);
    }
}
