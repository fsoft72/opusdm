//! Byte counts written for people. One scaling rule shared by the Lister,
//! the panel and the device popup; each place keeps its own unit style.

/// Binary (IEC) units for disk space: `512 B`, `1.5 KiB`, `5.0 GiB`.
pub const IEC_UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
/// Decimal (SI) units as drives are sold: `32 GB`, `1.5 TB`.
pub const SI_UNITS: &[&str] = &["B", "kB", "MB", "GB", "TB"];

/// `value` divided by `base` until it is below `base` or the last of
/// `unit_count` units is reached. Returns the scaled value and the unit index.
pub fn scale(value: f64, base: f64, unit_count: usize) -> (f64, usize) {
    let mut value = value.max(0.0);
    let mut unit = 0;
    while value >= base && unit + 1 < unit_count {
        value /= base;
        unit += 1;
    }
    (value, unit)
}

/// Binary units with one decimal, plain bytes below 1 KiB (`5.0 GiB`, `512 B`).
pub fn binary_size(bytes: u64) -> String {
    let (value, unit) = scale(bytes as f64, 1024.0, IEC_UNITS.len());
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", IEC_UNITS[unit]) }
}

/// Decimal units, one decimal only when the value is not round (`32 GB`,
/// `1.5 TB`, `0 B`).
pub fn decimal_size(bytes: u64) -> String {
    let (value, unit) = scale(bytes as f64, 1000.0, SI_UNITS.len());
    let rounded = (value * 10.0).round() / 10.0;
    if unit == 0 || rounded.fract() == 0.0 {
        format!("{} {}", rounded as u64, SI_UNITS[unit])
    } else {
        format!("{:.1} {}", rounded, SI_UNITS[unit])
    }
}

/// Compact transfer rate in binary steps (`0B/s`, `2.0K/s`, `3.0M/s`).
pub fn rate(bytes_per_s: f64) -> String {
    const UNITS: [&str; 4] = ["B", "K", "M", "G"];
    let (value, unit) = scale(bytes_per_s, 1024.0, UNITS.len());
    if unit == 0 { format!("{value:.0}B/s") } else { format!("{value:.1}{}/s", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_stops_at_the_last_unit() {
        assert_eq!(scale(512.0, 1024.0, 3), (512.0, 0));
        assert_eq!(scale(2048.0, 1024.0, 3), (2.0, 1));
        assert_eq!(scale(1024.0 * 1024.0 * 1024.0, 1024.0, 2), (1024.0 * 1024.0, 1));
        assert_eq!(scale(-5.0, 1024.0, 3), (0.0, 0), "negative counts as zero");
    }

    #[test]
    fn binary_sizes() {
        assert_eq!(binary_size(512), "512 B");
        assert_eq!(binary_size(1536), "1.5 KiB");
        assert_eq!(binary_size(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn decimal_sizes() {
        assert_eq!(decimal_size(0), "0 B");
        assert_eq!(decimal_size(32_000_000_000), "32 GB");
        assert_eq!(decimal_size(1_500_000_000_000), "1.5 TB");
        assert_eq!(decimal_size(16_000_898_564_096), "16 TB");
    }

    #[test]
    fn rates() {
        assert_eq!(rate(0.0), "0B/s");
        assert_eq!(rate(512.0), "512B/s");
        assert_eq!(rate(2048.0), "2.0K/s");
        assert_eq!(rate(3.0 * 1024.0 * 1024.0), "3.0M/s");
    }
}
