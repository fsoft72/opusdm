//! Read and extract archives through libarchive, behind a small safe API.
//!
//! Every format libarchive knows is read the same way; the format is detected
//! from the content. This is the only crate of the workspace with `unsafe`.

mod error;
mod extract;
mod names;
mod reader;
mod writer;

pub use error::{ArchiveError, Result};
pub use extract::{extract, extract_entry, Extracted};
pub use names::is_archive_name;
pub use reader::EntryKind;

use reader::Reader;
use std::path::Path;

/// One entry of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Normalised name: no leading `./`, no trailing `/`.
    pub path: String,
    pub kind: EntryKind,
    /// Size in bytes, 0 when the archive does not say.
    pub size: u64,
    /// Permission bits, without the file type.
    pub mode: u32,
    /// Modification time as Unix seconds, when stored.
    pub mtime: Option<i64>,
}

/// Every entry of `archive`, in archive order. The archive root entry (`./`)
/// is left out.
pub fn list(archive: &Path) -> Result<Vec<Entry>> {
    let mut reader = Reader::open(archive)?;
    let mut entries = Vec::new();
    while reader.next_header()? {
        let path = names::normalize(&reader.entry_path());
        if path.is_empty() {
            continue;
        }
        entries.push(Entry {
            path,
            kind: reader.entry_kind(),
            size: reader.entry_size(),
            mode: reader.entry_perm(),
            mtime: reader.entry_mtime(),
        });
    }
    Ok(entries)
}
