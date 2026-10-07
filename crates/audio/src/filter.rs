//! Which sources are real inputs.

/// Suffix PulseAudio gives to the "listen to an output" source of every sink.
const MONITOR_SUFFIX: &str = ".monitor";

/// Whether a source called `source_name` is an input (a microphone, a line in).
/// Monitors are the listening side of an output and are never listed.
pub fn is_input(source_name: &str) -> bool {
    !source_name.ends_with(MONITOR_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitors_are_dropped() {
        assert!(!is_input("alsa_output.pci-0000_01_00.1.hdmi-stereo.monitor"));
    }

    #[test]
    fn real_inputs_are_kept() {
        assert!(is_input("alsa_input.usb-Blue_Microphones_Yeti_Stereo_Microphone_REV8-00.analog-stereo"));
        assert!(is_input("bluez_input.00_11_22_33_44_55.0"));
    }
}
