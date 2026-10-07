//! Errors of the brightness control.

use thiserror::Error;

/// Anything that can go wrong reading or setting the brightness.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum BrightnessError {
    /// A required command line tool is not installed.
    #[error("{tool} is not installed (package: {package})")]
    MissingTool { tool: String, package: String },
    /// A tool ran and exited with an error.
    #[error("`{command}` failed: {stderr}")]
    CommandFailed { command: String, stderr: String },
    /// A file of the backlight could not be read or written.
    #[error("cannot access {path}: {reason}")]
    Io { path: String, reason: String },
    /// The output of a tool is not in the expected shape.
    #[error("unexpected output of {tool}: {detail}")]
    Parse { tool: String, detail: String },
    /// No method can change the brightness (the reasons are in the text).
    #[error("{0}")]
    Unavailable(String),
}
