//! Pure parsers and conversions for the output of the tools, kept apart from
//! the code that runs them so they can be tested with fixed text.

/// The `(current, max)` raw values from `ddcutil getvcp 10 --brief`, whose
/// line looks like `VCP 10 C 45 100`.
pub fn parse_ddc_brief(output: &str) -> Option<(u32, u32)> {
    let line = output.lines().find(|l| l.trim_start().starts_with("VCP"))?;
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 5 || tokens[2] != "C" {
        return None;
    }
    let current = tokens[3].parse().ok()?;
    let max: u32 = tokens[4].parse().ok()?;
    if max == 0 {
        return None;
    }
    Some((current, max))
}

/// The I2C bus number of the first usable monitor in `ddcutil detect
/// --brief`: the `I2C bus: /dev/i2c-N` line of the first `Display N` block
/// (an `Invalid display` block is skipped).
pub fn parse_ddc_detect_bus(output: &str) -> Option<u32> {
    let mut in_display = false;
    for line in output.lines() {
        if !line.starts_with(char::is_whitespace) {
            in_display = line.starts_with("Display ");
            continue;
        }
        if !in_display { continue; }
        if let Some(device) = line.trim().strip_prefix("I2C bus:") {
            return device.trim().strip_prefix("/dev/i2c-")?.parse().ok();
        }
    }
    None
}

/// The name of the first connected output in `xrandr --query`.
pub fn parse_xrandr_connected(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        let name = words.next()?;
        (words.next() == Some("connected")).then(|| name.to_string())
    })
}

/// The brightness (percent) of `output` in `xrandr --verbose`, from the
/// `Brightness: 1.0` line inside the block of that output.
pub fn parse_xrandr_brightness(verbose: &str, output: &str) -> Option<u8> {
    let mut in_block = false;
    for line in verbose.lines() {
        if !line.starts_with(char::is_whitespace) {
            in_block = line.split_whitespace().next() == Some(output);
            continue;
        }
        if !in_block {
            continue;
        }
        if let Some(value) = line.trim().strip_prefix("Brightness:") {
            let factor: f64 = value.trim().parse().ok()?;
            return Some((factor * 100.0).round().clamp(0.0, 100.0) as u8);
        }
    }
    None
}

/// Scale a percent to a raw value in `0..=max`, rounding to the nearest.
pub fn percent_to_raw(percent: u8, max: u32) -> u32 {
    (percent as u32 * max + 50) / 100
}

/// Scale a raw value in `0..=max` to a percent, rounding to the nearest.
pub fn raw_to_percent(raw: u32, max: u32) -> u8 {
    if max == 0 {
        return 0;
    }
    ((raw.min(max) * 100 + max / 2) / max) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddc_detect_gives_the_bus_of_the_first_display() {
        let out = "Invalid display\n   I2C bus:          /dev/i2c-6\n\nDisplay 1\n   I2C bus:          /dev/i2c-10\n   DRM connector:    card2-DP-2\n\nDisplay 2\n   I2C bus:          /dev/i2c-11\n";
        assert_eq!(parse_ddc_detect_bus(out), Some(10));
    }

    #[test]
    fn ddc_detect_without_a_display_gives_no_bus() {
        assert_eq!(parse_ddc_detect_bus("No displays found.\n"), None);
        assert_eq!(parse_ddc_detect_bus("Invalid display\n   I2C bus:  /dev/i2c-6\n"), None);
        assert_eq!(parse_ddc_detect_bus("Display 1\n   I2C bus:  /dev/i2c-x\n"), None);
    }

    const XRANDR_VERBOSE: &str = "Screen 0: minimum 8 x 8, current 2560 x 1440\n\
DP-0 connected primary 2560x1440+0+0 (0x1c6) normal 697mm x 392mm\n\
\tIdentifier: 0x1b9\n\
\tBrightness: 0.80\n\
\tGamma: 1.0:1.0:1.0\n\
HDMI-0 disconnected (normal left inverted right x axis y axis)\n\
\tBrightness: 0.50\n";

    #[test]
    fn ddc_brief_gives_current_and_max() {
        assert_eq!(parse_ddc_brief("VCP 10 C 45 100\n"), Some((45, 100)));
        assert_eq!(parse_ddc_brief("Display 1\nVCP 10 C 7 20\n"), Some((7, 20)));
    }

    #[test]
    fn ddc_brief_rejects_other_shapes() {
        assert_eq!(parse_ddc_brief(""), None);
        assert_eq!(parse_ddc_brief("VCP 10 ERR\n"), None);
        assert_eq!(parse_ddc_brief("VCP 10 C 45 0\n"), None);
        assert_eq!(parse_ddc_brief("VCP 10 C x 100\n"), None);
    }

    #[test]
    fn connected_output_skips_disconnected_ones() {
        let out = "Screen 0: minimum 8 x 8\nHDMI-0 disconnected\nDP-0 connected primary 2560x1440+0+0\n";
        assert_eq!(parse_xrandr_connected(out).as_deref(), Some("DP-0"));
        assert_eq!(parse_xrandr_connected("HDMI-0 disconnected\n"), None);
    }

    #[test]
    fn xrandr_brightness_reads_only_the_requested_block() {
        assert_eq!(parse_xrandr_brightness(XRANDR_VERBOSE, "DP-0"), Some(80));
        assert_eq!(parse_xrandr_brightness(XRANDR_VERBOSE, "HDMI-0"), Some(50));
        assert_eq!(parse_xrandr_brightness(XRANDR_VERBOSE, "DP-1"), None);
    }

    #[test]
    fn percent_and_raw_round_trip() {
        assert_eq!(percent_to_raw(50, 255), 128);
        assert_eq!(raw_to_percent(128, 255), 50);
        assert_eq!(percent_to_raw(100, 100), 100);
        assert_eq!(raw_to_percent(7, 0), 0);
        assert_eq!(raw_to_percent(300, 255), 100);
    }
}
