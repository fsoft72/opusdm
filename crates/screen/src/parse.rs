//! Reads the text of `xrandr --query --props` into monitors and the layout
//! that is set now. Pure functions: the tests use captured output.

use crate::{same_rate, Layout, Mode, Monitor, MonitorConfig, Rotation};

/// Hex digits on one line of an EDID dump (16 bytes).
const EDID_LINE_CHARS: usize = 32;
/// The hex digits of the EDID bytes 8..16 (maker, product, serial), used as
/// the monitor id.
const EDID_ID_RANGE: std::ops::Range<usize> = 16..32;

/// What `xrandr --query --props` says.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// The connected monitors.
    pub monitors: Vec<Monitor>,
    /// How each of them is set now (an unused one is `enabled: false`).
    pub layout: Layout,
}

/// The `WxH+X+Y` of a header line.
struct Geometry {
    size: (u32, u32),
    pos: (i32, i32),
}

/// An output while its lines are read.
#[derive(Default)]
struct Output {
    name: String,
    primary: bool,
    geometry: Option<Geometry>,
    rotation: Rotation,
    modes: Vec<Mode>,
    preferred: Option<(u32, u32)>,
    /// The mode and rate marked `*`.
    current: Option<((u32, u32), f64)>,
    edid: String,
}

/// Parse the text of `xrandr --query --props`. Disconnected outputs are left
/// out. Never fails: a line that is not understood is skipped.
pub fn parse_query(text: &str) -> Parsed {
    let mut outputs: Vec<Output> = Vec::new();
    let mut current: Option<Output> = None;
    let mut in_edid = false;
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            outputs.extend(current.take());
            in_edid = false;
            current = parse_header(line);
            continue;
        }
        let Some(output) = current.as_mut() else { continue };
        if line.starts_with("   ") {
            in_edid = false;
            parse_mode_line(line, output);
        } else if line.trim() == "EDID:" {
            in_edid = true;
        } else if in_edid {
            let hex = line.trim();
            if hex.len() == EDID_LINE_CHARS && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                output.edid.push_str(&hex.to_ascii_lowercase());
            } else {
                in_edid = false;
            }
        }
    }
    outputs.extend(current);
    build(outputs)
}

/// The output a header line starts, or `None` for the `Screen 0:` line and for
/// a disconnected output.
fn parse_header(line: &str) -> Option<Output> {
    let mut words = line.split_whitespace();
    let name = words.next()?;
    if words.next()? != "connected" { return None; }
    let mut output = Output { name: name.to_string(), ..Output::default() };
    for word in words {
        if word.starts_with('(') { break; }
        if word == "primary" {
            output.primary = true;
        } else if let Some(geometry) = parse_geometry(word) {
            output.geometry = Some(geometry);
        } else if let Some(rotation) = Rotation::from_xrandr(word) {
            output.rotation = rotation;
        }
    }
    Some(output)
}

/// Read `2560x1440+2560+0`.
fn parse_geometry(word: &str) -> Option<Geometry> {
    let (size, rest) = word.split_once('+')?;
    let (x, y) = rest.split_once('+')?;
    let (w, h) = parse_size(size)?;
    Some(Geometry { size: (w, h), pos: (x.parse().ok()?, y.parse().ok()?) })
}

/// Read `1920x1080` (an interlaced `1920x1080i` is not accepted).
fn parse_size(word: &str) -> Option<(u32, u32)> {
    let (w, h) = word.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Read a line such as `   2560x1600    240.00*+  60.00 +  60.00` into `output`.
fn parse_mode_line(line: &str, output: &mut Output) {
    let mut words = line.split_whitespace();
    let Some(size) = words.next().and_then(parse_size) else { return };
    let mut rates: Vec<f64> = Vec::new();
    let mut preferred = false;
    // The flags sit on the rate word (`60.00*+`) or alone after it (`60.00 +`).
    let mut last: Option<f64> = None;
    for word in words {
        let digits = word.trim_end_matches(['*', '+']);
        let flags = &word[digits.len()..];
        let rate = match digits.parse::<f64>() {
            Ok(rate) => {
                if !rates.iter().any(|r| same_rate(*r, rate)) { rates.push(rate); }
                last = Some(rate);
                rate
            }
            Err(_) if digits.is_empty() => match last {
                Some(rate) => rate,
                None => continue,
            },
            Err(_) => continue,
        };
        if flags.contains('+') { preferred = true; }
        if flags.contains('*') { output.current = Some((size, rate)); }
    }
    if rates.is_empty() { return; }
    if preferred && output.preferred.is_none() { output.preferred = Some(size); }
    match output.modes.iter_mut().find(|m| (m.width, m.height) == size) {
        Some(existing) => existing.rates.extend(rates),
        None => output.modes.push(Mode { width: size.0, height: size.1, rates }),
    }
}

/// The monitors and the layout of the outputs read.
fn build(outputs: Vec<Output>) -> Parsed {
    let mut monitors = Vec::new();
    let mut layout = Layout::default();
    for output in outputs {
        let edid_id = if output.edid.len() >= EDID_ID_RANGE.end { output.edid[EDID_ID_RANGE].to_string() } else { output.name.clone() };
        let monitor = Monitor { name: output.name.clone(), edid_id: edid_id.clone(), modes: output.modes.clone(), preferred: output.preferred };
        layout.monitors.push(config_of(&output, edid_id, &monitor));
        monitors.push(monitor);
    }
    Parsed { monitors, layout }
}

/// How `output` is set now.
fn config_of(output: &Output, edid_id: String, monitor: &Monitor) -> MonitorConfig {
    let Some(geometry) = &output.geometry else {
        return MonitorConfig {
            name: output.name.clone(),
            edid_id,
            enabled: false,
            mode: monitor.default_size().unwrap_or((0, 0)),
            rate: None,
            pos: (0, 0),
            rotation: Rotation::Normal,
            scale: 1.0,
            primary: false,
        };
    };
    // The geometry is the area on the desktop, so it is turned and scaled;
    // the starred mode is the real resolution.
    let (mode, rate) = match output.current {
        Some((mode, rate)) => (mode, Some(rate).filter(|r| *r > 0.0)),
        None if output.rotation.is_sideways() => ((geometry.size.1, geometry.size.0), None),
        None => (geometry.size, None),
    };
    let area_width = if output.rotation.is_sideways() { geometry.size.1 } else { geometry.size.0 };
    let scale = if mode.0 == 0 { 1.0 } else { (area_width as f64 / mode.0 as f64 * 100.0).round() / 100.0 };
    MonitorConfig {
        name: output.name.clone(),
        edid_id,
        enabled: true,
        mode,
        rate,
        pos: geometry.pos,
        rotation: output.rotation,
        scale,
        primary: output.primary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A laptop panel with an EDID, an external monitor on the right (primary,
    /// with an EDID) and two unplugged outputs.
    const DUAL: &str = "\
Screen 0: minimum 320 x 200, current 5120 x 1600, maximum 16384 x 16384
eDP-1 connected 2560x1600+0+0 (normal left inverted right x axis y axis) 366mm x 228mm
\tEDID:
\t\t00ffffffffffff0006afa68400000000
\t\t17200104a52517780308b5ae5242b226
\tvrr_capable: 1
\t\trange: (0, 1)
   2560x1600    240.00*+  60.00 +  60.00
   2560x1440     60.00
   1920x1080     60.00
   1024x768i     60.00
DP-1 disconnected (normal left inverted right x axis y axis)
\tmax bpc: 12
HDMI-1-0 connected primary 2560x1440+2560+0 (normal left inverted right x axis y axis) 597mm x 336mm
\tCTM: \t1.000000 0.000000 0.000000
\t\t0.000000 1.000000 0.000000
\tEDID:
\t\t00ffffffffffff000469a3273c390000
\t\t01170103803c22782aa595aa544fa126
\tBorderDimensions: 4
\t\tsupported: 4
   2560x1440     59.95*+
   1920x1080     60.00    59.94    50.00
";

    #[test]
    fn reads_the_connected_monitors_and_skips_the_others() {
        let parsed = parse_query(DUAL);
        let names: Vec<_> = parsed.monitors.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["eDP-1", "HDMI-1-0"]);
    }

    #[test]
    fn reads_modes_rates_and_the_preferred_size() {
        let parsed = parse_query(DUAL);
        let edp = &parsed.monitors[0];
        assert_eq!(edp.modes[0], Mode { width: 2560, height: 1600, rates: vec![240.0, 60.0] });
        assert_eq!(edp.modes.len(), 3, "the interlaced mode is skipped");
        assert_eq!(edp.preferred, Some((2560, 1600)));
        assert_eq!(parsed.monitors[1].modes[1].rates, vec![60.0, 59.94, 50.0]);
    }

    #[test]
    fn the_edid_gives_a_stable_id_and_a_missing_one_falls_back_to_the_name() {
        let ids: Vec<_> = parse_query(DUAL).monitors.iter().map(|m| m.edid_id.clone()).collect();
        assert_eq!(ids, ["06afa68400000000", "0469a3273c390000"]);
        let none = parse_query("DP-2 connected 1920x1080+0+0 (normal left) 1mm x 1mm\n   1920x1080     60.00*+\n");
        assert_eq!(none.monitors[0].edid_id, "DP-2");
    }

    #[test]
    fn reads_the_layout_that_is_set_now() {
        let layout = parse_query(DUAL).layout;
        let edp = layout.get("eDP-1").unwrap();
        assert!(edp.enabled && !edp.primary);
        assert_eq!((edp.mode, edp.rate, edp.pos), ((2560, 1600), Some(240.0), (0, 0)));
        let hdmi = layout.get("HDMI-1-0").unwrap();
        assert!(hdmi.primary);
        assert_eq!((hdmi.mode, hdmi.rate, hdmi.pos), ((2560, 1440), Some(59.95), (2560, 0)));
        assert_eq!((hdmi.rotation, hdmi.scale), (Rotation::Normal, 1.0));
    }

    #[test]
    fn a_monitor_on_its_side_has_the_real_resolution() {
        let text = "DP-1 connected 1080x1920+0+0 left (normal left inverted right x axis y axis) 1mm x 1mm\n   1920x1080     60.00*+\n";
        let c = &parse_query(text).layout.monitors[0];
        assert_eq!((c.rotation, c.mode, c.scale), (Rotation::Left, (1920, 1080), 1.0));
    }

    #[test]
    fn a_scaled_monitor_gives_the_scale_from_the_area_it_takes() {
        let text = "DP-1 connected 3840x2160+0+0 (normal left inverted right x axis y axis) 1mm x 1mm\n   1920x1080     60.00*+\n";
        assert_eq!(parse_query(text).layout.monitors[0].scale, 2.0);
    }

    #[test]
    fn a_connected_monitor_that_is_off_is_disabled() {
        let text = "HDMI-1 connected (normal left inverted right x axis y axis)\n   1920x1080     60.00 +\n   1280x720      60.00\n";
        let parsed = parse_query(text);
        let c = &parsed.layout.monitors[0];
        assert!(!c.enabled);
        assert_eq!(c.mode, (1920, 1080));
        assert_eq!(parsed.monitors[0].preferred, Some((1920, 1080)));
    }

    #[test]
    fn empty_or_garbage_text_gives_nothing() {
        assert_eq!(parse_query("").monitors, vec![]);
        assert_eq!(parse_query("xrandr: Can't open display\n").monitors, vec![]);
    }
}
