//! The Wayland side of the screen manager: `wlr-randr` (the
//! `wlr-output-management` protocol of labwc, sway and other wlroots
//! compositors). It reads `wlr-randr --json` into the same model the
//! `xrandr` parser fills, and sets a whole layout with one call.
//!
//! Two differences from X: there is no primary monitor, and the Wayland
//! scale is the other way round from `xrandr --scale` (2 makes things
//! larger), so the model's scale is its inverse; the size a monitor takes
//! on the desktop comes out the same.

use serde::Deserialize;

use crate::model::{same_rate, Layout, Mode, Monitor, MonitorConfig, Rotation};
use crate::parse::Parsed;
use crate::ScreenError;

/// The program run by this backend.
pub const WLR_RANDR: &str = "wlr-randr";

/// One output of `wlr-randr --json`.
#[derive(Debug, Deserialize)]
struct JsonOutput {
    name: String,
    make: Option<String>,
    model: Option<String>,
    serial: Option<String>,
    enabled: bool,
    #[serde(default)]
    modes: Vec<JsonMode>,
    position: Option<JsonPosition>,
    transform: Option<String>,
    scale: Option<f64>,
}

/// One mode of a `JsonOutput`.
#[derive(Debug, Deserialize)]
struct JsonMode {
    width: u32,
    height: u32,
    refresh: f64,
    #[serde(default)]
    preferred: bool,
    #[serde(default)]
    current: bool,
}

/// The position of a `JsonOutput`.
#[derive(Debug, Deserialize)]
struct JsonPosition {
    x: i32,
    y: i32,
}

/// The rotation of a Wayland `transform` (counter-clockwise, like `xrandr
/// --rotate left` for `90`). Mirrored transforms count as their rotation.
fn rotation_of(transform: &str) -> Rotation {
    match transform.trim_start_matches("flipped").trim_start_matches('-') {
        "90" => Rotation::Left,
        "180" => Rotation::Inverted,
        "270" => Rotation::Right,
        _ => Rotation::Normal,
    }
}

/// The Wayland `transform` of a rotation.
fn transform_of(rotation: Rotation) -> &'static str {
    match rotation {
        Rotation::Normal => "normal",
        Rotation::Left => "90",
        Rotation::Inverted => "180",
        Rotation::Right => "270",
    }
}

/// A refresh rate rounded to what the window shows and compares (3 decimals).
fn round_rate(rate: f64) -> f64 {
    (rate * 1000.0).round() / 1000.0
}

/// The monitors and their settings in `wlr-randr --json` output.
pub fn parse_json(text: &str) -> Result<Parsed, ScreenError> {
    let outputs: Vec<JsonOutput> = serde_json::from_str(text).map_err(|e| ScreenError::Session(format!("cannot read the wlr-randr output: {e}")))?;
    let mut parsed = Parsed { monitors: Vec::new(), layout: Layout::default() };
    for output in outputs {
        let ids: Vec<&str> = [&output.make, &output.model, &output.serial].into_iter().flatten().map(String::as_str).filter(|s| !s.is_empty()).collect();
        let edid_id = if ids.is_empty() { output.name.clone() } else { ids.join(" ") };
        let mut modes: Vec<Mode> = Vec::new();
        for m in &output.modes {
            let rate = round_rate(m.refresh);
            match modes.iter_mut().find(|known| (known.width, known.height) == (m.width, m.height)) {
                Some(known) => {
                    if rate > 0.0 && !known.rates.iter().any(|r| same_rate(*r, rate)) { known.rates.push(rate); }
                }
                None => modes.push(Mode { width: m.width, height: m.height, rates: if rate > 0.0 { vec![rate] } else { Vec::new() } }),
            }
        }
        for mode in &mut modes {
            mode.rates.sort_by(|a, b| b.total_cmp(a));
        }
        modes.sort_by(|a, b| (b.width * b.height).cmp(&(a.width * a.height)));
        let preferred = output.modes.iter().find(|m| m.preferred).map(|m| (m.width, m.height));
        let current = output.modes.iter().find(|m| m.current);
        let monitor = Monitor { name: output.name.clone(), edid_id: edid_id.clone(), modes, preferred };
        let mode = current.map(|m| (m.width, m.height)).or_else(|| monitor.default_size()).unwrap_or((0, 0));
        let wl_scale = output.scale.filter(|s| *s > 0.0).unwrap_or(1.0);
        parsed.layout.monitors.push(MonitorConfig {
            name: output.name,
            edid_id,
            enabled: output.enabled && current.is_some(),
            mode,
            rate: current.map(|m| round_rate(m.refresh)).filter(|r| *r > 0.0),
            pos: output.position.map(|p| (p.x, p.y)).unwrap_or((0, 0)),
            rotation: output.transform.as_deref().map(rotation_of).unwrap_or_default(),
            scale: 1.0 / wl_scale,
            primary: false,
        });
        parsed.monitors.push(monitor);
    }
    Ok(parsed)
}

/// The one `wlr-randr` call that sets `layout`.
pub fn wlr_randr_args(layout: &Layout) -> Vec<String> {
    let mut args = vec![WLR_RANDR.to_string()];
    for m in &layout.monitors {
        args.extend(["--output".to_string(), m.name.clone()]);
        if !m.enabled {
            args.push("--off".to_string());
            continue;
        }
        let mode = match m.rate {
            Some(rate) => format!("{}x{}@{rate}Hz", m.mode.0, m.mode.1),
            None => format!("{}x{}", m.mode.0, m.mode.1),
        };
        let scale = if m.scale > 0.0 { 1.0 / m.scale } else { 1.0 };
        args.extend([
            "--on".to_string(),
            "--mode".to_string(),
            mode,
            "--pos".to_string(),
            format!("{},{}", m.pos.0, m.pos.1),
            "--transform".to_string(),
            transform_of(m.rotation).to_string(),
            "--scale".to_string(),
            format!("{scale}"),
        ]);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_OUTPUTS: &str = r#"[
      { "name": "eDP-1", "make": "BOE", "model": "0x0BCA", "serial": null, "enabled": true,
        "modes": [
          { "width": 1920, "height": 1080, "refresh": 60.002998, "preferred": true, "current": true },
          { "width": 1920, "height": 1080, "refresh": 48.0, "preferred": false, "current": false },
          { "width": 1280, "height": 720, "refresh": 60.0, "preferred": false, "current": false }
        ],
        "position": { "x": 0, "y": 0 }, "transform": "normal", "scale": 1.0 },
      { "name": "HDMI-A-1", "make": null, "model": null, "serial": null, "enabled": true,
        "modes": [ { "width": 2560, "height": 1440, "refresh": 59.951, "preferred": true, "current": true } ],
        "position": { "x": 1920, "y": 0 }, "transform": "90", "scale": 2.0 }
    ]"#;

    #[test]
    fn json_becomes_monitors_and_a_layout() {
        let parsed = parse_json(TWO_OUTPUTS).unwrap();
        let edp = &parsed.monitors[0];
        assert_eq!(edp.edid_id, "BOE 0x0BCA");
        assert_eq!(edp.modes[0].rates, vec![60.003, 48.0], "one mode per size, highest rate first");
        assert_eq!(edp.modes[1].width, 1280);
        assert_eq!(edp.preferred, Some((1920, 1080)));
        assert_eq!(parsed.monitors[1].edid_id, "HDMI-A-1", "no make, model or serial: the name");
        let hdmi = parsed.layout.get("HDMI-A-1").unwrap();
        assert_eq!((hdmi.mode, hdmi.pos, hdmi.rotation, hdmi.rate), ((2560, 1440), (1920, 0), Rotation::Left, Some(59.951)));
        assert_eq!(hdmi.scale, 0.5, "Wayland scale 2 is xrandr scale 0.5");
        assert_eq!(hdmi.desktop_size(), (720, 1280));
    }

    #[test]
    fn a_layout_becomes_one_wlr_randr_call() {
        let mut layout = parse_json(TWO_OUTPUTS).unwrap().layout;
        layout.get_mut("eDP-1").unwrap().enabled = false;
        let args = wlr_randr_args(&layout);
        assert_eq!(args[..4], ["wlr-randr", "--output", "eDP-1", "--off"]);
        assert_eq!(
            args[4..],
            ["--output", "HDMI-A-1", "--on", "--mode", "2560x1440@59.951Hz", "--pos", "1920,0", "--transform", "90", "--scale", "2"]
        );
    }

    #[test]
    fn transforms_map_both_ways() {
        for rotation in Rotation::ALL {
            assert_eq!(rotation_of(transform_of(rotation)), rotation);
        }
        assert_eq!(rotation_of("flipped-270"), Rotation::Right);
        assert!(parse_json("not json").is_err());
    }
}
