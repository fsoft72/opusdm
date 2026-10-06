//! Errors of the input configuration.

use thiserror::Error;

/// Anything that can go wrong reading or applying input settings.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum InputError {
    /// A required command line tool is not installed.
    #[error("{tool} is not installed (package: {package})")]
    MissingTool { tool: String, package: String },
    /// A tool ran and exited with an error.
    #[error("`{command}` failed: {stderr}")]
    CommandFailed { command: String, stderr: String },
    /// The settings file could not be written.
    #[error("cannot write {path}: {reason}")]
    Write { path: String, reason: String },
    /// The session cannot be configured (no X display, Wayland).
    #[error("{0}")]
    Session(String),
}
