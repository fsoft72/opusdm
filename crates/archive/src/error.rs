//! Errors of the archive crate.

use std::io;
use std::path::PathBuf;

/// What can go wrong while reading or extracting an archive.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The file could not be opened, or libarchive did not recognise it.
    #[error("cannot open {}: {msg}", path.display())]
    Open { path: PathBuf, msg: String },
    /// libarchive failed while reading; `0` is its own message.
    #[error("cannot read the archive: {0}")]
    Read(String),
    /// The format needs a feature this build of libarchive lacks.
    #[error("unsupported archive format: {0}")]
    Unsupported(String),
    /// The archive (or one entry) is encrypted; passwords are not supported.
    #[error("the archive is encrypted")]
    Encrypted,
    /// An entry name would land outside the destination folder.
    #[error("unsafe entry name: {0:?}")]
    UnsafePath(String),
    /// A requested entry is not in the archive.
    #[error("not in the archive: {0:?}")]
    NotFound(String),
    /// The cancel callback asked to stop.
    #[error("cancelled")]
    Cancelled,
    /// Writing to the destination failed.
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Result alias of the crate.
pub type Result<T> = std::result::Result<T, ArchiveError>;

impl From<ArchiveError> for io::Error {
    /// Map to the closest `io::ErrorKind`, so callers working with
    /// `io::Result` keep a meaningful kind.
    fn from(e: ArchiveError) -> Self {
        let kind = match &e {
            ArchiveError::Unsupported(_) => io::ErrorKind::Unsupported,
            ArchiveError::UnsafePath(_) => io::ErrorKind::InvalidInput,
            ArchiveError::NotFound(_) => io::ErrorKind::NotFound,
            ArchiveError::Cancelled => io::ErrorKind::Interrupted,
            ArchiveError::Io(inner) => return io::Error::new(inner.kind(), inner.to_string()),
            _ => io::ErrorKind::Other,
        };
        io::Error::new(kind, e.to_string())
    }
}
