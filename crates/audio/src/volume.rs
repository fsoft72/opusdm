//! Conversion between a percentage and a PulseAudio volume value.

/// PulseAudio volume of 100% (0 dB).
pub const NORM: u32 = 65_536;
/// The highest percentage the panel sets.
pub const MAX_PERCENT: u32 = 100;

/// PulseAudio value for `percent`, clamped to `MAX_PERCENT` and rounded.
pub fn to_raw(percent: u32) -> u32 {
    let p = percent.min(MAX_PERCENT) as u64;
    ((p * NORM as u64 + 50) / 100) as u32
}

/// Percentage for a PulseAudio value, rounded. Not clamped: a volume raised
/// above 100% by another tool shows its real value.
pub fn from_raw(raw: u32) -> u32 {
    ((raw as u64 * 100 + NORM as u64 / 2) / NORM as u64) as u32
}

/// The channel volumes `current` scaled so the loudest one becomes `target`,
/// keeping the balance between them (what `pa_cvolume_scale` does). When
/// every channel is silent there is no balance to keep and all get `target`.
pub fn scale_channels(current: &[u32], target: u32) -> Vec<u32> {
    let loudest = current.iter().copied().max().unwrap_or(0);
    if loudest == 0 { return vec![target; current.len()]; }
    current.iter().map(|&c| ((c as u64 * target as u64 + loudest as u64 / 2) / loudest as u64) as u32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_keeps_the_balance_between_channels() {
        // Left at 100%, right at 50%: setting 50% halves both.
        assert_eq!(scale_channels(&[NORM, NORM / 2], NORM / 2), vec![NORM / 2, NORM / 4]);
        // Raising never puts the loudest channel above the target.
        assert_eq!(scale_channels(&[NORM / 4, NORM / 2], NORM), vec![NORM / 2, NORM]);
    }

    #[test]
    fn scaling_silent_or_equal_channels_sets_them_all() {
        assert_eq!(scale_channels(&[0, 0], to_raw(30)), vec![to_raw(30); 2]);
        assert_eq!(scale_channels(&[NORM; 6], to_raw(70)), vec![to_raw(70); 6]);
        assert!(scale_channels(&[], NORM).is_empty());
    }

    #[test]
    fn known_points() {
        assert_eq!(to_raw(0), 0);
        assert_eq!(to_raw(50), 32_768);
        assert_eq!(to_raw(100), NORM);
    }

    #[test]
    fn to_raw_rounds_instead_of_truncating() {
        // 2% is 1310.72: rounding gives 1311, truncating 1310.
        assert_eq!(to_raw(2), 1_311);
    }

    #[test]
    fn to_raw_clamps_at_100() {
        assert_eq!(to_raw(250), NORM);
    }

    #[test]
    fn from_raw_rounds_to_the_nearest_percent() {
        assert_eq!(from_raw(51_773), 79);
        assert_eq!(from_raw(13_763), 21);
        assert_eq!(from_raw(64_225), 98);
        assert_eq!(from_raw(NORM), 100);
    }

    #[test]
    fn from_raw_keeps_an_amplified_volume() {
        assert_eq!(from_raw(2 * NORM), 200);
    }

    #[test]
    fn every_percentage_survives_a_round_trip() {
        for p in 0..=MAX_PERCENT {
            assert_eq!(from_raw(to_raw(p)), p, "percent {p}");
        }
    }
}
