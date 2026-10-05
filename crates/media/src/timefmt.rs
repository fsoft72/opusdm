//! Time text and the mapping between the seek slider and nanoseconds.

/// Nanoseconds per second.
const NS_PER_SEC: u64 = 1_000_000_000;
/// Upper end of the seek slider; its resolution is a thousandth of the length.
pub const SLIDER_MAX: f64 = 1000.0;

/// `mm:ss`, or `h:mm:ss` from one hour on.
pub fn format_time(ns: u64) -> String {
    let total = ns / NS_PER_SEC;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

/// Slider value (0..=`SLIDER_MAX`) for a position; 0 when the length is unknown.
pub fn ns_to_slider(position: u64, duration: u64) -> f64 {
    if duration == 0 { return 0.0; }
    (position as f64 / duration as f64 * SLIDER_MAX).clamp(0.0, SLIDER_MAX)
}

/// Position for a slider value; 0 when the length is unknown.
pub fn slider_to_ns(value: f64, duration: u64) -> u64 {
    if duration == 0 { return 0; }
    (value.clamp(0.0, SLIDER_MAX) / SLIDER_MAX * duration as f64) as u64
}

/// `position` moved by `delta_secs` seconds, kept inside `0..=duration`.
pub fn offset(position: u64, delta_secs: i64, duration: u64) -> u64 {
    let moved = position as i128 + delta_secs as i128 * NS_PER_SEC as i128;
    moved.clamp(0, duration as i128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_minutes_and_hours() {
        assert_eq!(format_time(0), "00:00");
        assert_eq!(format_time(65 * NS_PER_SEC), "01:05");
        assert_eq!(format_time(3599 * NS_PER_SEC), "59:59");
        assert_eq!(format_time(3723 * NS_PER_SEC), "1:02:03");
    }

    #[test]
    fn slider_round_trip() {
        let d = 200 * NS_PER_SEC;
        assert_eq!(ns_to_slider(d / 2, d), 500.0);
        assert_eq!(slider_to_ns(500.0, d), d / 2);
        assert_eq!(slider_to_ns(2000.0, d), d);
        assert_eq!(ns_to_slider(d * 2, d), SLIDER_MAX);
    }

    #[test]
    fn unknown_length_maps_to_zero() {
        assert_eq!(ns_to_slider(5, 0), 0.0);
        assert_eq!(slider_to_ns(500.0, 0), 0);
    }

    #[test]
    fn offset_is_clamped() {
        let d = 100 * NS_PER_SEC;
        assert_eq!(offset(50 * NS_PER_SEC, 5, d), 55 * NS_PER_SEC);
        assert_eq!(offset(2 * NS_PER_SEC, -5, d), 0);
        assert_eq!(offset(98 * NS_PER_SEC, 5, d), d);
    }
}
