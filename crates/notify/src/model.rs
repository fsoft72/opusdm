//! One notification as the server keeps it, built from the arguments of
//! `Notify`. No GTK here: the picture of `image-data` stays raw bytes until a
//! popup turns it into a texture.

use std::collections::HashMap;
use zbus::zvariant::{OwnedValue, Value};

/// Urgency levels of the specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

impl Urgency {
    /// The level of the `urgency` hint byte; unknown values are normal.
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Urgency::Low,
            2 => Urgency::Critical,
            _ => Urgency::Normal,
        }
    }
}

/// Raw pixels of `image-data`: 8 bits per sample, RGB or RGBA rows of
/// `stride` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub has_alpha: bool,
    pub data: Vec<u8>,
}

/// The picture of a notification, best source first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Image {
    Pixels(Pixels),
    /// An icon name of the theme or a file path (`file://` already removed).
    Named(String),
}

/// One action: its key, sent back in `ActionInvoked`, and its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub key: String,
    pub label: String,
}

/// Key of the action run by clicking the notification itself.
pub const DEFAULT_ACTION: &str = "default";

/// A notification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    /// The body as the application sent it (it may hold markup).
    pub body: String,
    pub actions: Vec<Action>,
    pub urgency: Urgency,
    /// Milliseconds as sent: -1 server default, 0 never.
    pub expire_timeout: i32,
    /// From `image-data` or `image-path`.
    pub image: Option<Image>,
    /// The `desktop-entry` hint: the application's `.desktop` name without
    /// the extension.
    pub desktop_entry: Option<String>,
    pub transient: bool,
    pub resident: bool,
    /// The unique bus name of the sender.
    pub sender: String,
    /// Arrival time, seconds since the epoch.
    pub time: i64,
}

impl Notification {
    /// Build a notification from the arguments of `Notify` (the id is set by
    /// the store). Malformed hints are ignored.
    pub fn from_call(app_name: String, app_icon: String, summary: String, body: String, actions: &[String], hints: &HashMap<String, OwnedValue>, expire_timeout: i32) -> Self {
        Notification {
            id: 0,
            app_name,
            app_icon: strip_file_url(&app_icon),
            summary,
            body,
            actions: pair_actions(actions),
            urgency: hint_byte(hints, "urgency").map(Urgency::from_byte).unwrap_or_default(),
            expire_timeout,
            image: image_from_hints(hints),
            desktop_entry: hint_string(hints, "desktop-entry").filter(|s| !s.is_empty()),
            transient: hint_bool(hints, "transient"),
            resident: hint_bool(hints, "resident"),
            sender: String::new(),
            time: 0,
        }
    }

    /// Give a nameless notification a name, so it can be muted and is
    /// labelled: the name of the sending `process` (Brave sends an empty
    /// app name), else the `desktop-entry` hint.
    pub fn fill_app_name(&mut self, process: Option<&str>) {
        if !self.app_name.trim().is_empty() { return; }
        let process = process.map(str::trim).filter(|p| !p.is_empty()).map(capitalize);
        self.app_name = process.or_else(|| self.desktop_entry.clone()).unwrap_or_default();
    }

    /// The default action, if the application offers one.
    pub fn default_action(&self) -> Option<&Action> {
        self.actions.iter().find(|a| a.key == DEFAULT_ACTION)
    }

    /// The actions shown as buttons: every one but the default.
    pub fn button_actions(&self) -> impl Iterator<Item = &Action> {
        self.actions.iter().filter(|a| a.key != DEFAULT_ACTION)
    }

    /// An icon name or path that stands for the notification without its
    /// pixels: the image path, the app icon, then the desktop entry name.
    pub fn icon_hint(&self) -> String {
        if let Some(Image::Named(name)) = &self.image { return name.clone(); }
        if !self.app_icon.is_empty() { return self.app_icon.clone(); }
        self.desktop_entry.clone().unwrap_or_default()
    }
}

/// `name` with its first letter in upper case (`brave` -> `Brave`).
fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `actions` comes as a flat list `key, label, key, label...`; a key
/// without its label is dropped.
fn pair_actions(actions: &[String]) -> Vec<Action> {
    actions.as_chunks::<2>().0.iter().map(|[key, label]| Action { key: key.clone(), label: label.clone() }).collect()
}

/// `s` without a leading `file://`.
fn strip_file_url(s: &str) -> String {
    s.strip_prefix("file://").unwrap_or(s).to_string()
}

/// The picture: `image-data` (or its older names), then `image-path`, then
/// the deprecated `icon_data`.
fn image_from_hints(hints: &HashMap<String, OwnedValue>) -> Option<Image> {
    let data = ["image-data", "image_data"].iter().find_map(|k| hints.get(*k)).and_then(pixels_from_value);
    if let Some(p) = data { return Some(Image::Pixels(p)); }
    let path = ["image-path", "image_path"].iter().find_map(|k| hint_string(hints, k)).filter(|s| !s.is_empty());
    if let Some(p) = path { return Some(Image::Named(strip_file_url(&p))); }
    hints.get("icon_data").and_then(pixels_from_value).map(Image::Pixels)
}

/// Read the `(iiibiiay)` structure of `image-data`. `None` when it does not
/// describe a whole 8 bit RGB or RGBA picture.
fn pixels_from_value(value: &OwnedValue) -> Option<Pixels> {
    let Value::Structure(s) = &**value else { return None };
    let fields = s.fields();
    if fields.len() != 7 { return None; }
    let int = |i: usize| -> Option<i32> { if let Value::I32(v) = fields[i] { Some(v) } else { None } };
    let (width, height, stride) = (int(0)?, int(1)?, int(2)?);
    let Value::Bool(has_alpha) = fields[3] else { return None };
    let (bits, channels) = (int(4)?, int(5)?);
    let Value::Array(array) = &fields[6] else { return None };
    let data: Vec<u8> = array.iter().map(|v| if let Value::U8(b) = v { Some(*b) } else { None }).collect::<Option<_>>()?;
    let pixels = Pixels { width, height, stride, has_alpha, data };
    pixels_valid(&pixels, bits, channels).then_some(pixels)
}

/// Whether the declared geometry fits the bytes.
fn pixels_valid(p: &Pixels, bits: i32, channels: i32) -> bool {
    if bits != 8 || p.width <= 0 || p.height <= 0 { return false; }
    if channels != if p.has_alpha { 4 } else { 3 } { return false; }
    let row = i64::from(p.width) * i64::from(channels);
    if i64::from(p.stride) < row { return false; }
    let needed = i64::from(p.stride) * i64::from(p.height - 1) + row;
    p.data.len() as i64 >= needed
}

/// A byte hint (some senders use other integer types).
fn hint_byte(hints: &HashMap<String, OwnedValue>, key: &str) -> Option<u8> {
    match &**hints.get(key)? {
        Value::U8(v) => Some(*v),
        Value::I32(v) => u8::try_from(*v).ok(),
        Value::U32(v) => u8::try_from(*v).ok(),
        Value::I16(v) => u8::try_from(*v).ok(),
        Value::U16(v) => u8::try_from(*v).ok(),
        Value::I64(v) => u8::try_from(*v).ok(),
        _ => None,
    }
}

/// A string hint.
fn hint_string(hints: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    match &**hints.get(key)? {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// A boolean hint; also accepts an integer (some senders use a byte).
fn hint_bool(hints: &HashMap<String, OwnedValue>, key: &str) -> bool {
    match hints.get(key).map(|v| &**v) {
        Some(Value::Bool(b)) => *b,
        Some(Value::U8(v)) => *v != 0,
        Some(Value::I32(v)) => *v != 0,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::StructureBuilder;

    fn owned(v: Value<'_>) -> OwnedValue {
        v.try_to_owned().unwrap()
    }

    fn image_value(w: i32, h: i32, stride: i32, alpha: bool, bits: i32, channels: i32, data: Vec<u8>) -> OwnedValue {
        let s = StructureBuilder::new()
            .add_field(w)
            .add_field(h)
            .add_field(stride)
            .add_field(alpha)
            .add_field(bits)
            .add_field(channels)
            .add_field(data)
            .build()
            .unwrap();
        owned(Value::Structure(s))
    }

    fn call(hints: HashMap<String, OwnedValue>) -> Notification {
        Notification::from_call("App".into(), "".into(), "S".into(), "B".into(), &[], &hints, -1)
    }

    #[test]
    fn hints_are_read() {
        let mut hints = HashMap::new();
        hints.insert("urgency".to_string(), owned(Value::U8(2)));
        hints.insert("desktop-entry".to_string(), owned(Value::from("brave-browser")));
        hints.insert("transient".to_string(), owned(Value::Bool(true)));
        hints.insert("resident".to_string(), owned(Value::U8(1)));
        let n = call(hints);
        assert_eq!(n.urgency, Urgency::Critical);
        assert_eq!(n.desktop_entry.as_deref(), Some("brave-browser"));
        assert!(n.transient && n.resident);
    }

    #[test]
    fn missing_or_odd_hints_take_defaults() {
        let mut hints = HashMap::new();
        hints.insert("urgency".to_string(), owned(Value::from("high")));
        let n = call(hints);
        assert_eq!(n.urgency, Urgency::Normal);
        assert!(!n.transient && n.image.is_none() && n.desktop_entry.is_none());
    }

    #[test]
    fn actions_come_in_pairs() {
        let list: Vec<String> = ["default", "Open", "reply", "Reply", "orphan"].iter().map(|s| s.to_string()).collect();
        let n = Notification::from_call("A".into(), "".into(), "".into(), "".into(), &list, &HashMap::new(), -1);
        assert_eq!(n.actions.len(), 2);
        assert_eq!(n.default_action().map(|a| a.label.as_str()), Some("Open"));
        assert_eq!(n.button_actions().map(|a| a.key.as_str()).collect::<Vec<_>>(), vec!["reply"]);
    }

    #[test]
    fn image_data_is_read_when_whole() {
        let mut hints = HashMap::new();
        // 2x2 RGB, stride 8 (2 bytes of padding per row): 8 + 6 bytes needed.
        hints.insert("image-data".to_string(), image_value(2, 2, 8, false, 8, 3, vec![1; 14]));
        let Some(Image::Pixels(p)) = call(hints).image else { panic!("no pixels") };
        assert_eq!((p.width, p.height, p.stride, p.has_alpha, p.data.len()), (2, 2, 8, false, 14));
    }

    #[test]
    fn bad_image_data_is_ignored() {
        let cases = [
            image_value(2, 2, 8, false, 8, 3, vec![1; 13]), // short buffer
            image_value(2, 2, 4, false, 8, 3, vec![1; 16]), // stride below a row
            image_value(2, 2, 8, true, 8, 3, vec![1; 16]),  // alpha with 3 channels
            image_value(2, 2, 8, false, 16, 3, vec![1; 32]), // 16 bits
            image_value(0, 2, 8, false, 8, 3, vec![1; 16]), // empty
        ];
        for value in cases {
            let mut hints = HashMap::new();
            hints.insert("image-data".to_string(), value);
            assert_eq!(call(hints).image, None);
        }
    }

    #[test]
    fn image_path_and_file_urls() {
        let mut hints = HashMap::new();
        hints.insert("image-path".to_string(), owned(Value::from("file:///tmp/pic.png")));
        let n = Notification::from_call("A".into(), "file:///usr/share/a.svg".into(), "".into(), "".into(), &[], &hints, -1);
        assert_eq!(n.image, Some(Image::Named("/tmp/pic.png".into())));
        assert_eq!(n.app_icon, "/usr/share/a.svg");
        assert_eq!(n.icon_hint(), "/tmp/pic.png");
    }

    #[test]
    fn a_nameless_notification_takes_the_process_name() {
        let mut hints = HashMap::new();
        hints.insert("desktop-entry".to_string(), owned(Value::from("org.chromium.Chromium")));
        let mut n = Notification::from_call("".into(), "".into(), "".into(), "".into(), &[], &hints, -1);
        n.fill_app_name(Some("brave\n"));
        assert_eq!(n.app_name, "Brave");
        let mut n = Notification::from_call(" ".into(), "".into(), "".into(), "".into(), &[], &hints, -1);
        n.fill_app_name(None);
        assert_eq!(n.app_name, "org.chromium.Chromium");
        let mut named = call(HashMap::new());
        named.fill_app_name(Some("other"));
        assert_eq!(named.app_name, "App");
    }

    #[test]
    fn icon_hint_falls_back_to_the_desktop_entry() {
        let mut hints = HashMap::new();
        hints.insert("desktop-entry".to_string(), owned(Value::from("org.x.App")));
        assert_eq!(call(hints).icon_hint(), "org.x.App");
    }
}
