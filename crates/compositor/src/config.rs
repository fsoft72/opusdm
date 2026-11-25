//! `compositor.json`: the built-in bindings of the compositor (window
//! switcher, close, desktops, focus, window menu, quit), the snap threshold
//! and the title bar layout. Read at startup and on the IPC `reload`, never
//! written: the owner edits it by hand.

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::state::LOG_TAG;

/// Environment variable that points at another `compositor.json`.
pub const CONFIG_ENV: &str = "OPUSDM_COMPOSITOR_CONFIG";

/// Name of the file in `opusdm_ipc::settings::config_dir()`.
pub const CONFIG_FILE: &str = "compositor.json";

/// Default key of the window menu.
pub const DEFAULT_WINDOW_MENU: &str = "Alt+space";

/// Default key that ends the compositor.
pub const DEFAULT_QUIT: &str = "Ctrl+Alt+BackSpace";

/// Every built-in binding (its `compositor.json` key) with its default key
/// (spec, `openbox-rc.xml`), in the order `bindings/list` shows them. An
/// empty key disables the binding.
pub const BUILTIN_DEFAULTS: [(&str, &str); 20] = [
    ("switcher_next", "Alt+Tab"),
    ("switcher_prev", "Alt+Shift+Tab"),
    ("switcher_all", "Ctrl+Alt+Tab"),
    ("close", "Alt+F4"),
    ("desktop_left", "Ctrl+Alt+Left"),
    ("desktop_right", "Ctrl+Alt+Right"),
    ("desktop_1", "Super+F1"),
    ("desktop_2", "Super+F2"),
    ("desktop_3", "Super+F3"),
    ("desktop_4", "Super+F4"),
    ("send_left", "Shift+Alt+Left"),
    ("send_right", "Shift+Alt+Right"),
    ("show_desktop", "Super+d"),
    ("lower", "Alt+Escape"),
    ("focus_left", "Super+Shift+Left"),
    ("focus_right", "Super+Shift+Right"),
    ("focus_up", "Super+Shift+Up"),
    ("focus_down", "Super+Shift+Down"),
    ("window_menu", DEFAULT_WINDOW_MENU),
    ("quit", DEFAULT_QUIT),
];

/// Default snap threshold, in pixels (`labwc-rc.xml` `<range>`).
pub const DEFAULT_SNAP_THRESHOLD: u32 = 10;

/// Accepted range of the snap threshold, in pixels, inclusive.
pub const SNAP_THRESHOLD_RANGE: (u32, u32) = (0, 100);

/// Default title bar layout (labwc syntax: the buttons left of `:` go on
/// the left, the ones right of it on the right).
pub const DEFAULT_TITLE_LAYOUT: &str = "close,iconify,max:";

/// The keys `compositor.json` may have: every built-in binding, then the
/// other settings.
const KNOWN_KEYS: [&str; 23] = [
    "switcher_next",
    "switcher_prev",
    "switcher_all",
    "close",
    "desktop_left",
    "desktop_right",
    "desktop_1",
    "desktop_2",
    "desktop_3",
    "desktop_4",
    "send_left",
    "send_right",
    "show_desktop",
    "lower",
    "focus_left",
    "focus_right",
    "focus_up",
    "focus_down",
    "window_menu",
    "quit",
    "quit_enabled",
    "snap_threshold",
    "title_layout",
];

/// The settings of `compositor.json`; a missing key, or a key with a value
/// of the wrong type, takes its default. Each built-in binding field holds
/// the key text as written (an empty text disables the binding).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositorConfig {
    /// Next window of the switcher (current desktop).
    pub switcher_next: String,
    /// Previous window of the switcher (current desktop).
    pub switcher_prev: String,
    /// Switcher over the windows of every desktop.
    pub switcher_all: String,
    /// Closes the active window.
    pub close: String,
    /// Shows the desktop on the left.
    pub desktop_left: String,
    /// Shows the desktop on the right.
    pub desktop_right: String,
    /// Shows desktop 1.
    pub desktop_1: String,
    /// Shows desktop 2.
    pub desktop_2: String,
    /// Shows desktop 3.
    pub desktop_3: String,
    /// Shows desktop 4.
    pub desktop_4: String,
    /// Sends the active window to the desktop on the left (and follows it).
    pub send_left: String,
    /// Sends the active window to the desktop on the right (and follows it).
    pub send_right: String,
    /// Shows the desktop (minimizes every window, again restores them).
    pub show_desktop: String,
    /// Lowers the active window to the bottom of the stack.
    pub lower: String,
    /// Moves the focus to the window on the left.
    pub focus_left: String,
    /// Moves the focus to the window on the right.
    pub focus_right: String,
    /// Moves the focus to the window above.
    pub focus_up: String,
    /// Moves the focus to the window below.
    pub focus_down: String,
    /// Key that opens the window menu of the focused window.
    pub window_menu: String,
    /// Key that ends the compositor (and the session).
    pub quit: String,
    /// False turns the quit binding off.
    pub quit_enabled: bool,
    /// Distance in pixels from a screen edge where a dragged window snaps,
    /// inside `SNAP_THRESHOLD_RANGE`.
    pub snap_threshold: u32,
    /// Buttons of the title bar, labwc syntax (`close,iconify,max:`).
    pub title_layout: String,
}

impl Default for CompositorConfig {
    /// The keys of `BUILTIN_DEFAULTS`, quit enabled, snap threshold
    /// `DEFAULT_SNAP_THRESHOLD`, title layout `DEFAULT_TITLE_LAYOUT`.
    fn default() -> CompositorConfig {
        let key = |name: &str| builtin_default(name).to_string();
        CompositorConfig {
            switcher_next: key("switcher_next"),
            switcher_prev: key("switcher_prev"),
            switcher_all: key("switcher_all"),
            close: key("close"),
            desktop_left: key("desktop_left"),
            desktop_right: key("desktop_right"),
            desktop_1: key("desktop_1"),
            desktop_2: key("desktop_2"),
            desktop_3: key("desktop_3"),
            desktop_4: key("desktop_4"),
            send_left: key("send_left"),
            send_right: key("send_right"),
            show_desktop: key("show_desktop"),
            lower: key("lower"),
            focus_left: key("focus_left"),
            focus_right: key("focus_right"),
            focus_up: key("focus_up"),
            focus_down: key("focus_down"),
            window_menu: key("window_menu"),
            quit: key("quit"),
            quit_enabled: true,
            snap_threshold: DEFAULT_SNAP_THRESHOLD,
            title_layout: DEFAULT_TITLE_LAYOUT.to_string(),
        }
    }
}

impl CompositorConfig {
    /// Parses the JSON object `json`; missing keys take their default, an
    /// unknown key is logged with `err_log!` and ignored, a key with a value
    /// of the wrong type is logged with `err_log!` (naming the key) and
    /// takes its default, the other keys still apply. Not JSON, or not an
    /// object: `Err`. Only the tests use it; `load_with_errors` keeps the
    /// messages for the IPC `reload` reply.
    #[cfg(test)]
    pub fn parse(json: &str) -> Result<CompositorConfig, String> {
        let (config, bad) = CompositorConfig::parse_with_errors(json)?;
        for error in bad {
            opusdm_ipc::err_log!(LOG_TAG, "compositor.json: {error}");
        }
        Ok(config)
    }

    /// The work of `parse`: the config and one message per problem (not
    /// logged here): an unknown key (ignored), a key whose value has the
    /// wrong type (it took its default), a snap threshold out of range (it
    /// was clamped). Not JSON, or not an object: `Err`.
    pub fn parse_with_errors(json: &str) -> Result<(CompositorConfig, Vec<String>), String> {
        let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut bad: Vec<String> = unknown_keys(&value).iter().map(|key| format!("unknown key '{key}' ignored")).collect();
        let Value::Object(object) = value else { return Err("not a JSON object".to_string()) };
        let d = CompositorConfig::default();
        let o = &object;
        let config = CompositorConfig {
            switcher_next: _key_or_default(o, "switcher_next", d.switcher_next, &mut bad),
            switcher_prev: _key_or_default(o, "switcher_prev", d.switcher_prev, &mut bad),
            switcher_all: _key_or_default(o, "switcher_all", d.switcher_all, &mut bad),
            close: _key_or_default(o, "close", d.close, &mut bad),
            desktop_left: _key_or_default(o, "desktop_left", d.desktop_left, &mut bad),
            desktop_right: _key_or_default(o, "desktop_right", d.desktop_right, &mut bad),
            desktop_1: _key_or_default(o, "desktop_1", d.desktop_1, &mut bad),
            desktop_2: _key_or_default(o, "desktop_2", d.desktop_2, &mut bad),
            desktop_3: _key_or_default(o, "desktop_3", d.desktop_3, &mut bad),
            desktop_4: _key_or_default(o, "desktop_4", d.desktop_4, &mut bad),
            send_left: _key_or_default(o, "send_left", d.send_left, &mut bad),
            send_right: _key_or_default(o, "send_right", d.send_right, &mut bad),
            show_desktop: _key_or_default(o, "show_desktop", d.show_desktop, &mut bad),
            lower: _key_or_default(o, "lower", d.lower, &mut bad),
            focus_left: _key_or_default(o, "focus_left", d.focus_left, &mut bad),
            focus_right: _key_or_default(o, "focus_right", d.focus_right, &mut bad),
            focus_up: _key_or_default(o, "focus_up", d.focus_up, &mut bad),
            focus_down: _key_or_default(o, "focus_down", d.focus_down, &mut bad),
            window_menu: _key_or_default(o, "window_menu", d.window_menu, &mut bad),
            quit: _key_or_default(o, "quit", d.quit, &mut bad),
            quit_enabled: _key_or_default(o, "quit_enabled", d.quit_enabled, &mut bad),
            snap_threshold: _snap_threshold(_key_or_default(o, "snap_threshold", d.snap_threshold, &mut bad), &mut bad),
            title_layout: _key_or_default(o, "title_layout", d.title_layout, &mut bad),
        };
        Ok((config, bad))
    }

    /// Every built-in binding as (name, key text as written), in the order
    /// of `BUILTIN_DEFAULTS`; an empty text is a disabled binding. `quit`
    /// is listed whatever `quit_enabled` says.
    pub fn builtin_keys(&self) -> [(&'static str, &str); 20] {
        [
            ("switcher_next", self.switcher_next.as_str()),
            ("switcher_prev", self.switcher_prev.as_str()),
            ("switcher_all", self.switcher_all.as_str()),
            ("close", self.close.as_str()),
            ("desktop_left", self.desktop_left.as_str()),
            ("desktop_right", self.desktop_right.as_str()),
            ("desktop_1", self.desktop_1.as_str()),
            ("desktop_2", self.desktop_2.as_str()),
            ("desktop_3", self.desktop_3.as_str()),
            ("desktop_4", self.desktop_4.as_str()),
            ("send_left", self.send_left.as_str()),
            ("send_right", self.send_right.as_str()),
            ("show_desktop", self.show_desktop.as_str()),
            ("lower", self.lower.as_str()),
            ("focus_left", self.focus_left.as_str()),
            ("focus_right", self.focus_right.as_str()),
            ("focus_up", self.focus_up.as_str()),
            ("focus_down", self.focus_down.as_str()),
            ("window_menu", self.window_menu.as_str()),
            ("quit", self.quit.as_str()),
        ]
    }

    /// Reads `$OPUSDM_COMPOSITOR_CONFIG`, else `config_dir()/compositor.json`
    /// (read only). A missing file gives the defaults (`debug_log!`), a file
    /// that cannot be read or parsed gives the defaults (`err_log!`).
    pub fn load() -> CompositorConfig {
        CompositorConfig::load_with_errors().0
    }

    /// The work of `load`: the config and its problems, each also logged
    /// with `err_log!` (the IPC `reload` replies with them). A missing file
    /// is not a problem.
    pub fn load_with_errors() -> (CompositorConfig, Vec<String>) {
        let path = config_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                opusdm_ipc::debug_log!(LOG_TAG, "{} not found, default bindings", path.display());
                return (CompositorConfig::default(), Vec::new());
            }
            Err(e) => {
                let error = format!("{} not read, default bindings: {e}", path.display());
                opusdm_ipc::err_log!(LOG_TAG, "{error}");
                return (CompositorConfig::default(), vec![error]);
            }
        };
        match CompositorConfig::parse_with_errors(&text) {
            Ok((config, bad)) => {
                for error in &bad {
                    opusdm_ipc::err_log!(LOG_TAG, "compositor.json: {error}");
                }
                opusdm_ipc::debug_log!(LOG_TAG, "bindings from {}: {config:?}", path.display());
                (config, bad)
            }
            Err(e) => {
                let error = format!("{} is not valid, default bindings: {e}", path.display());
                opusdm_ipc::err_log!(LOG_TAG, "{error}");
                (CompositorConfig::default(), vec![error])
            }
        }
    }
}

/// The default key of the built-in `name` (empty for an unknown name).
pub fn builtin_default(name: &str) -> &'static str {
    BUILTIN_DEFAULTS.iter().find(|(n, _)| *n == name).map_or("", |(_, key)| key)
}

/// `value` clamped to `SNAP_THRESHOLD_RANGE`; a clamped value pushes a
/// message to `bad`.
fn _snap_threshold(value: u32, bad: &mut Vec<String>) -> u32 {
    let (lo, hi) = SNAP_THRESHOLD_RANGE;
    let clamped = value.clamp(lo, hi);
    if clamped != value {
        bad.push(format!("key 'snap_threshold' value {value} is outside {lo}..={hi}, {clamped} used"));
    }
    clamped
}

/// The value of `key` in `object`, `default` when it is missing; a value of
/// the wrong type also gives `default` and pushes a message naming the key
/// to `bad`.
fn _key_or_default<T: DeserializeOwned + std::fmt::Debug>(object: &Map<String, Value>, key: &str, default: T, bad: &mut Vec<String>) -> T {
    let Some(value) = object.get(key) else { return default };
    match serde_json::from_value(value.clone()) {
        Ok(parsed) => parsed,
        Err(e) => {
            bad.push(format!("key '{key}' has a bad value {value} ({e}), default {default:?} used"));
            default
        }
    }
}

/// The keys of the JSON object `value` that `CompositorConfig` does not
/// know (none when `value` is not an object).
pub fn unknown_keys(value: &serde_json::Value) -> Vec<String> {
    let Some(object) = value.as_object() else { return Vec::new() };
    object.keys().filter(|k| !KNOWN_KEYS.contains(&k.as_str())).cloned().collect()
}

/// Path of `compositor.json`: `$OPUSDM_COMPOSITOR_CONFIG` when set and not
/// empty, else `config_dir()/compositor.json`.
pub fn config_path() -> PathBuf {
    match std::env::var_os(CONFIG_ENV) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => opusdm_ipc::settings::config_dir().join(CONFIG_FILE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_object_is_the_defaults() {
        let config = CompositorConfig::parse("{}").unwrap();
        assert_eq!(config, CompositorConfig::default());
        assert_eq!(config.window_menu, "Alt+space");
        assert_eq!(config.quit, "Ctrl+Alt+BackSpace");
        assert!(config.quit_enabled);
    }

    #[test]
    fn an_empty_object_gives_every_stage_3_default() {
        let config = CompositorConfig::parse("{}").unwrap();
        let expected = [
            ("switcher_next", "Alt+Tab"),
            ("switcher_prev", "Alt+Shift+Tab"),
            ("switcher_all", "Ctrl+Alt+Tab"),
            ("close", "Alt+F4"),
            ("desktop_left", "Ctrl+Alt+Left"),
            ("desktop_right", "Ctrl+Alt+Right"),
            ("desktop_1", "Super+F1"),
            ("desktop_2", "Super+F2"),
            ("desktop_3", "Super+F3"),
            ("desktop_4", "Super+F4"),
            ("send_left", "Shift+Alt+Left"),
            ("send_right", "Shift+Alt+Right"),
            ("show_desktop", "Super+d"),
            ("lower", "Alt+Escape"),
            ("focus_left", "Super+Shift+Left"),
            ("focus_right", "Super+Shift+Right"),
            ("focus_up", "Super+Shift+Up"),
            ("focus_down", "Super+Shift+Down"),
            ("window_menu", "Alt+space"),
            ("quit", "Ctrl+Alt+BackSpace"),
        ];
        assert_eq!(config.builtin_keys(), expected);
        assert_eq!(config.close, "Alt+F4");
        assert_eq!(config.focus_down, "Super+Shift+Down");
        assert_eq!(config.snap_threshold, 10);
        assert_eq!(config.title_layout, "close,iconify,max:");
        assert!(config.quit_enabled);
        for (name, _) in expected {
            assert!(KNOWN_KEYS.contains(&name), "{name} is a known key");
        }
        for name in ["quit_enabled", "snap_threshold", "title_layout"] {
            assert!(KNOWN_KEYS.contains(&name), "{name} is a known key");
        }
    }

    #[test]
    fn the_snap_threshold_is_clamped_and_reported() {
        let (config, bad) = CompositorConfig::parse_with_errors(r#"{"snap_threshold": 500}"#).unwrap();
        assert_eq!(config.snap_threshold, 100);
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert!(bad[0].contains("'snap_threshold'"), "{bad:?}");
        assert_eq!(CompositorConfig::parse(r#"{"snap_threshold": 0}"#).unwrap().snap_threshold, 0);
        assert_eq!(CompositorConfig::parse(r#"{"snap_threshold": -4}"#).unwrap().snap_threshold, 10, "not a u32: default");
    }

    #[test]
    fn an_empty_key_disables_its_binding_and_keeps_the_others() {
        let config = CompositorConfig::parse(r#"{"close": "", "title_layout": ":close"}"#).unwrap();
        assert_eq!(config.close, "");
        assert_eq!(config.switcher_next, "Alt+Tab");
        assert_eq!(config.title_layout, ":close");
    }

    #[test]
    fn unknown_keys_are_config_errors() {
        let (config, bad) = CompositorConfig::parse_with_errors(r#"{"closee": "Alt+F4", "lower": "Alt+Escape"}"#).unwrap();
        assert_eq!(config, CompositorConfig::default());
        assert_eq!(bad, ["unknown key 'closee' ignored"]);
    }

    #[test]
    fn quit_can_be_turned_off_and_the_rest_keeps_its_default() {
        let config = CompositorConfig::parse(r#"{"quit_enabled": false}"#).unwrap();
        assert!(!config.quit_enabled);
        assert_eq!(config.quit, "Ctrl+Alt+BackSpace");
        assert_eq!(config.window_menu, "Alt+space");
    }

    #[test]
    fn unknown_keys_are_found_and_the_rest_still_applies() {
        let json = r#"{"quit_enabled": false, "window_menue": "Super+m"}"#;
        assert_eq!(unknown_keys(&serde_json::from_str(json).unwrap()), ["window_menue"]);
        let config = CompositorConfig::parse(json).unwrap();
        assert!(!config.quit_enabled);
        assert_eq!(config.window_menu, "Alt+space");
    }

    #[test]
    fn keys_are_read_and_bad_json_is_an_error() {
        let config = CompositorConfig::parse(r#"{"window_menu": "Super+m", "quit": "Ctrl+Alt+q"}"#).unwrap();
        assert_eq!((config.window_menu.as_str(), config.quit.as_str()), ("Super+m", "Ctrl+Alt+q"));
        assert!(CompositorConfig::parse("not json").is_err());
        assert!(CompositorConfig::parse("[1]").is_err());
    }

    #[test]
    fn a_key_with_the_wrong_type_takes_its_default_and_the_rest_applies() {
        let json = r#"{"quit_enabled": "false", "window_menu": "Super+m", "quit": "Ctrl+Alt+q"}"#;
        let (config, bad) = CompositorConfig::parse_with_errors(json).unwrap();
        assert!(config.quit_enabled, "the bad key takes its default");
        assert_eq!((config.window_menu.as_str(), config.quit.as_str()), ("Super+m", "Ctrl+Alt+q"), "the good keys apply");
        assert_eq!(bad.len(), 1, "one error: {bad:?}");
        assert!(bad[0].contains("'quit_enabled'"), "the error names the key: {bad:?}");
    }

    #[test]
    fn every_bad_key_is_reported_and_falls_back_on_its_own() {
        let json = r#"{"quit_enabled": false, "window_menu": 3, "quit": null}"#;
        let (config, bad) = CompositorConfig::parse_with_errors(json).unwrap();
        assert!(!config.quit_enabled, "the good key applies");
        assert_eq!(config.window_menu, DEFAULT_WINDOW_MENU);
        assert_eq!(config.quit, DEFAULT_QUIT);
        assert_eq!(bad.len(), 2, "{bad:?}");
        assert!(bad.iter().any(|e| e.contains("'window_menu'")) && bad.iter().any(|e| e.contains("'quit'")), "{bad:?}");
        assert_eq!(CompositorConfig::parse(json).unwrap(), config, "parse gives the same config");
    }
}
