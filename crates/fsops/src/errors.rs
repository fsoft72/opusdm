//! Typed filesystem operations errors.

use std::path::PathBuf;
use thiserror::Error;

/// High-level filesystem operation error.
#[derive(Debug, Error)]
pub enum FsError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Scanning error at {path}: {source}")]
    Scan {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Path '{0}' is outside its source base")]
    PathOutsideBase(PathBuf),

    #[error("Destination '{0}' already exists")]
    AlreadyExists(PathBuf),

    #[error("Operation cancelled")]
    Cancelled,
}

impl FsError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}
