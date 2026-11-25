//! `compositor.json`: the built-in bindings of the compositor (window menu,
//! quit). Read once at startup, never written: the owner edits it by hand.

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

/// The keys `compositor.json` may have.
const KNOWN_KEYS: [&str; 3] = ["window_menu", "quit", "quit_enabled"];

/// The settings of `compositor.json`; a missing key, or a key with a value
/// of the wrong type, takes its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositorConfig {
    /// Key that opens the window menu of the focused window.
    pub window_menu: String,
    /// Key that ends the compositor (and the session).
    pub quit: String,
    /// False turns the quit binding off.
    pub quit_enabled: bool,
}

impl Default for CompositorConfig {
    /// `Alt+space`, `Ctrl+Alt+BackSpace`, quit enabled.
    fn default() -> CompositorConfig {
        CompositorConfig {
            window_menu: DEFAULT_WINDOW_MENU.to_string(),
            quit: DEFAULT_QUIT.to_string(),
            quit_enabled: true,
        }
    }
}

impl CompositorConfig {
    /// Parses the JSON object `json`; missing keys take their default, an
    /// unknown key is logged with `err_log!` and ignored, a key with a value
    /// of the wrong type is logged with `err_log!` (naming the key) and
    /// takes its default, the other keys still apply. Not JSON, or not an
    /// object: `Err`.
    pub fn parse(json: &str) -> Result<CompositorConfig, String> {
        let (config, bad) = CompositorConfig::parse_with_errors(json)?;
        for error in bad {
            opusdm_ipc::err_log!(LOG_TAG, "compositor.json: {error}");
        }
        Ok(config)
    }

    /// The work of `parse`: the config and one message per key whose value
    /// has the wrong type (that key took its default). Logs the unknown
    /// keys. Not JSON, or not an object: `Err`.
    pub fn parse_with_errors(json: &str) -> Result<(CompositorConfig, Vec<String>), String> {
        let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        for key in unknown_keys(&value) {
            opusdm_ipc::err_log!(LOG_TAG, "compositor.json: unknown key '{key}' ignored");
        }
        let Value::Object(object) = value else { return Err("not a JSON object".to_string()) };
        let defaults = CompositorConfig::default();
        let mut bad = Vec::new();
        let config = CompositorConfig {
            window_menu: _key_or_default(&object, "window_menu", defaults.window_menu, &mut bad),
            quit: _key_or_default(&object, "quit", defaults.quit, &mut bad),
            quit_enabled: _key_or_default(&object, "quit_enabled", defaults.quit_enabled, &mut bad),
        };
        Ok((config, bad))
    }

    /// Reads `$OPUSDM_COMPOSITOR_CONFIG`, else `config_dir()/compositor.json`
    /// (read only). A missing file gives the defaults (`debug_log!`), a file
    /// that cannot be read or parsed gives the defaults (`err_log!`).
    pub fn load() -> CompositorConfig {
        let path = config_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                opusdm_ipc::debug_log!(LOG_TAG, "{} not found, default bindings", path.display());
                return CompositorConfig::default();
            }
            Err(e) => {
                opusdm_ipc::err_log!(LOG_TAG, "{} not read, default bindings: {e}", path.display());
                return CompositorConfig::default();
            }
        };
        match CompositorConfig::parse(&text) {
            Ok(config) => {
                opusdm_ipc::debug_log!(LOG_TAG, "bindings from {}: {config:?}", path.display());
                config
            }
            Err(e) => {
                opusdm_ipc::err_log!(LOG_TAG, "{} is not valid, default bindings: {e}", path.display());
                CompositorConfig::default()
            }
        }
    }
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
