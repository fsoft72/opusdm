//! What the panel and the Bluetooth tool show: the adapter, the devices it
//! knows and the rfkill switch. Plain data, compared to send only changes.

use crate::rfkill::Rfkill;

/// The Bluetooth adapter (`org.bluez.Adapter1`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Adapter {
    pub path: String,
    pub address: String,
    /// The name other devices see.
    pub alias: String,
    pub powered: bool,
    pub discoverable: bool,
    pub pairable: bool,
    /// A scan is running.
    pub discovering: bool,
}

/// What a device is, from the BlueZ `Icon` property.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceKind {
    Headset,
    Audio,
    Keyboard,
    Mouse,
    Phone,
    Computer,
    Gamepad,
    #[default]
    Other,
}

impl DeviceKind {
    /// The kind of a BlueZ icon name (`audio-headset`, `input-mouse`, ...).
    pub fn from_icon(icon: &str) -> Self {
        match icon {
            "audio-headset" | "audio-headphones" => Self::Headset,
            "audio-card" | "audio-speakers" | "multimedia-player" => Self::Audio,
            "input-keyboard" => Self::Keyboard,
            "input-mouse" | "input-tablet" => Self::Mouse,
            "phone" => Self::Phone,
            "computer" => Self::Computer,
            "input-gaming" => Self::Gamepad,
            _ => Self::Other,
        }
    }

    /// Icon of the theme for this kind.
    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Headset => "audio-headphones",
            Self::Audio => "audio-speakers",
            Self::Keyboard => "input-keyboard",
            Self::Mouse => "input-mouse",
            Self::Phone => "phone",
            Self::Computer => "computer",
            Self::Gamepad => "input-gaming",
            Self::Other => "bluetooth",
        }
    }

    /// A word for the details page.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Headset => "Headset",
            Self::Audio => "Audio device",
            Self::Keyboard => "Keyboard",
            Self::Mouse => "Mouse",
            Self::Phone => "Phone",
            Self::Computer => "Computer",
            Self::Gamepad => "Game controller",
            Self::Other => "Device",
        }
    }
}

/// A device the adapter knows (`org.bluez.Device1`). The signal strength is
/// left out on purpose: during a scan it changes all the time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub path: String,
    pub address: String,
    /// Alias, else Name, else the address.
    pub name: String,
    pub kind: DeviceKind,
    pub paired: bool,
    pub trusted: bool,
    pub blocked: bool,
    pub connected: bool,
    /// Battery percentage, when the device reports it (`org.bluez.Battery1`).
    pub battery: Option<u8>,
}

/// Everything about Bluetooth at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BtState {
    /// `None` when there is no adapter.
    pub adapter: Option<Adapter>,
    /// The devices of the adapter, in path order.
    pub devices: Vec<Device>,
    pub rfkill: Rfkill,
}

impl BtState {
    /// Whether the adapter is there and powered.
    pub fn is_on(&self) -> bool {
        self.adapter.as_ref().is_some_and(|a| a.powered)
    }

    /// The paired devices, connected first, then by name.
    pub fn paired(&self) -> Vec<&Device> {
        let mut out: Vec<&Device> = self.devices.iter().filter(|d| d.paired).collect();
        out.sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        out
    }

    /// The devices seen by a scan and not paired, by name.
    pub fn found(&self) -> Vec<&Device> {
        let mut out: Vec<&Device> = self.devices.iter().filter(|d| !d.paired).collect();
        out.sort_by_key(|d| d.name.to_lowercase());
        out
    }

    /// The device at `path`.
    pub fn device(&self, path: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.path == path)
    }

    /// The name of the device at `path`, or the path when it is not known.
    pub fn device_name(&self, path: &str) -> String {
        self.device(path).map(|d| d.name.clone()).unwrap_or_else(|| path.to_string())
    }
}

/// A device for tests.
pub fn test_device(path: &str, name: &str, paired: bool, connected: bool) -> Device {
    Device { path: path.to_string(), address: "00:11:22:33:44:55".to_string(), name: name.to_string(), paired, connected, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_from_bluez_icon() {
        assert_eq!(DeviceKind::from_icon("audio-headset"), DeviceKind::Headset);
        assert_eq!(DeviceKind::from_icon("input-keyboard"), DeviceKind::Keyboard);
        assert_eq!(DeviceKind::from_icon("input-mouse"), DeviceKind::Mouse);
        assert_eq!(DeviceKind::from_icon("phone"), DeviceKind::Phone);
        assert_eq!(DeviceKind::from_icon("x"), DeviceKind::Other);
    }

    #[test]
    fn paired_lists_connected_first() {
        let s = BtState {
            devices: vec![
                test_device("/a", "zeta", true, false),
                test_device("/b", "Alpha", true, false),
                test_device("/c", "mouse", true, true),
                test_device("/d", "new", false, false),
            ],
            ..Default::default()
        };
        let names: Vec<&str> = s.paired().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["mouse", "Alpha", "zeta"]);
        assert_eq!(s.found().len(), 1);
        assert_eq!(s.device_name("/c"), "mouse");
        assert_eq!(s.device_name("/x"), "/x");
    }
}
