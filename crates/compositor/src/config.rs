//! `compositor.json`: the built-in bindings of the compositor (window menu,
//! quit). Read once at startup, never written: the owner edits it by hand.

use std::path::PathBuf;

use serde::Deserialize;

use crate::state::LOG_TAG;

/// Environment variable that points at another `compositor.json`.
pub const CONFIG_ENV: &str = "OPUSDM_COMPOSITOR_CONFIG";

/// Name of the file in `opusdm_ipc::settings::config_dir()`.
pub const CONFIG_FILE: &str = "compositor.json";

/// Default key of the window menu.
pub const DEFAULT_WINDOW_MENU: &str = "Alt+space";

/// Default key that ends the compositor.
pub const DEFAULT_QUIT: &str = "Ctrl+Alt+BackSpace";

/// The settings of `compositor.json`; a missing key takes its default.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
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
    /// Parses the JSON object `json`; missing keys take their default.
    pub fn parse(json: &str) -> Result<CompositorConfig, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
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
    fn keys_are_read_and_bad_json_is_an_error() {
        let config = CompositorConfig::parse(r#"{"window_menu": "Super+m", "quit": "Ctrl+Alt+q"}"#).unwrap();
        assert_eq!((config.window_menu.as_str(), config.quit.as_str()), ("Super+m", "Ctrl+Alt+q"));
        assert!(CompositorConfig::parse("not json").is_err());
        assert!(CompositorConfig::parse(r#"{"quit_enabled": "yes"}"#).is_err());
    }
}
