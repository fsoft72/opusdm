//! Errors of the screen manager.

use thiserror::Error;

/// Anything that can go wrong reading, checking or applying a screen layout.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ScreenError {
    /// A required command line tool is not installed.
    #[error("{tool} is not installed (package: {package})")]
    MissingTool { tool: String, package: String },
    /// A tool ran and exited with an error.
    #[error("`{command}` failed: {stderr}")]
    CommandFailed { command: String, stderr: String },
    /// A layout cannot be applied as it is.
    #[error("invalid layout: {0}")]
    Invalid(String),
    /// No monitor has this name.
    #[error("unknown monitor {0}")]
    UnknownMonitor(String),
    /// The monitors share no resolution, so they cannot show the same picture.
    #[error("the monitors have no resolution in common, so they cannot be duplicated")]
    NoCommonMode,
    /// A settings file could not be written.
    #[error("cannot write {path}: {reason}")]
    Write { path: String, reason: String },
    /// The session cannot be configured (no X display, Wayland).
    #[error("{0}")]
    Session(String),
}
