//! The data the panel needs about one sound device.

/// Which side of the sound card a device is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// A sink: speakers, headphones, HDMI.
    Output,
    /// A source that is not a monitor: a microphone, a line in.
    Input,
}

/// One output or input device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    /// Index in the sound server; only unique within a `Kind`.
    pub index: u32,
    /// Stable server name, used to make the device the default.
    pub name: String,
    /// Human readable name.
    pub description: String,
    pub kind: Kind,
    /// Average over the channels. Can be above 100 when another tool raised it.
    pub volume_percent: u32,
    pub muted: bool,
    pub is_default: bool,
}

impl AudioDevice {
    /// Identity of the device: the index alone repeats between outputs and inputs.
    pub fn key(&self) -> (Kind, u32) {
        (self.kind, self.index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_tells_an_output_from_an_input_with_the_same_index() {
        let mk = |kind| AudioDevice {
            index: 3,
            name: "n".into(),
            description: "d".into(),
            kind,
            volume_percent: 50,
            muted: false,
            is_default: false,
        };
        assert_ne!(mk(Kind::Output).key(), mk(Kind::Input).key());
    }
}
