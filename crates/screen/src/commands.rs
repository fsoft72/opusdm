//! The `xrandr` command line that applies a layout. A pure function: the
//! backend runs the result.

use crate::{Layout, MonitorConfig};

/// The program every layout is applied with.
pub const XRANDR: &str = "xrandr";

/// The arguments of one `xrandr` call (program name first) that sets every
/// monitor of `layout`. One call means one atomic change.
pub fn xrandr_args(layout: &Layout) -> Vec<String> {
    let mut args = vec![XRANDR.to_string()];
    for config in &layout.monitors {
        args.push("--output".to_string());
        args.push(config.name.clone());
        if config.enabled {
            args.extend(enabled_args(config));
        } else {
            args.push("--off".to_string());
        }
    }
    if !layout.enabled().any(|m| m.primary) {
        args.push("--noprimary".to_string());
    }
    args
}

/// The arguments that turn one monitor on.
fn enabled_args(config: &MonitorConfig) -> Vec<String> {
    let mut args = vec!["--mode".to_string(), format!("{}x{}", config.mode.0, config.mode.1)];
    if let Some(rate) = config.rate {
        args.push("--rate".to_string());
        args.push(format!("{rate:.2}"));
    }
    args.push("--pos".to_string());
    args.push(format!("{}x{}", config.pos.0, config.pos.1));
    args.push("--rotate".to_string());
    args.push(config.rotation.xrandr_arg().to_string());
    args.push("--scale".to_string());
    args.push(format!("{:.2}x{:.2}", config.scale, config.scale));
    if config.primary {
        args.push("--primary".to_string());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_support::two_monitors;
    use crate::Rotation;

    fn layout() -> Layout {
        let ms = two_monitors();
        let mut a = MonitorConfig::default_for(&ms[0]).unwrap();
        a.primary = true;
        let mut b = MonitorConfig::default_for(&ms[1]).unwrap();
        b.pos = (2560, 0);
        Layout { monitors: vec![a, b] }
    }

    #[test]
    fn one_call_sets_every_output() {
        let args = xrandr_args(&layout());
        let line = args.join(" ");
        assert_eq!(
            line,
            "xrandr --output eDP-1 --mode 2560x1600 --pos 0x0 --rotate normal --scale 1.00x1.00 --primary \
             --output HDMI-1 --mode 2560x1440 --pos 2560x0 --rotate normal --scale 1.00x1.00"
        );
    }

    #[test]
    fn rate_rotation_and_scale_are_passed() {
        let mut l = layout();
        l.monitors[1].rate = Some(59.94);
        l.monitors[1].rotation = Rotation::Left;
        l.monitors[1].scale = 1.5;
        let line = xrandr_args(&l).join(" ");
        assert!(line.contains("--mode 2560x1440 --rate 59.94 --pos 2560x0 --rotate left --scale 1.50x1.50"), "{line}");
    }

    #[test]
    fn a_monitor_that_is_off_only_gets_off() {
        let mut l = layout();
        l.monitors[1].enabled = false;
        let line = xrandr_args(&l).join(" ");
        assert!(line.ends_with("--output HDMI-1 --off"), "{line}");
    }

    #[test]
    fn without_a_primary_monitor_the_primary_is_cleared() {
        let mut l = layout();
        l.monitors[0].primary = false;
        assert_eq!(xrandr_args(&l).last().map(String::as_str), Some("--noprimary"));
        l.monitors[0].primary = true;
        assert!(!xrandr_args(&l).contains(&"--noprimary".to_string()));
    }
}
