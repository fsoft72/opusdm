//! The config files the hub reads once at startup (`settings.json`,
//! `desktop.json`, `functions.json`) and hands to the server, the GTK side
//! and the startup code, so the three never read them separately. After
//! startup the server owns them and tells the GTK side of every change.

use opusdm_ipc::function::Function;
use opusdm_ipc::{DesktopSettings, Settings};

/// What the hub starts with.
#[derive(Clone, Debug)]
pub struct StartupConfig {
    /// `settings.json`.
    pub settings: Settings,
    /// `desktop.json`, sanitized.
    pub desktop: DesktopSettings,
    /// `functions.json`.
    pub functions: Vec<Function>,
}

impl StartupConfig {
    /// Read the three files from the standard paths. A missing or broken
    /// file gives its defaults (and is logged by the loader).
    pub fn load() -> StartupConfig {
        StartupConfig {
            settings: crate::settings::load_settings(),
            desktop: crate::desktop_settings::load(),
            functions: opusdm_ipc::function::load_functions(),
        }
    }
}
