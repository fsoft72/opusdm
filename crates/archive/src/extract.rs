//! Extraction: one entry to a stream, or a selection / everything to a folder.

use crate::error::{ArchiveError, Result};
use crate::names;
use crate::reader::{EntryKind, Reader};
use crate::writer::DiskWriter;
use std::io::Write;
use std::path::Path;

/// What an extraction did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Extracted {
    /// Entries written (folders are not counted).
    pub written: usize,
    /// Entries left alone because the destination already had them.
    pub skipped: usize,
}

/// Copy the file `name` of `archive` into `out`.
pub fn extract_entry(archive: &Path, name: &str, out: &mut dyn Write) -> Result<()> {
    let wanted = names::normalize(name);
    let mut reader = Reader::open(archive)?;
    while reader.next_header()? {
        if names::normalize(&reader.entry_path()) != wanted {
            continue;
        }
        if reader.entry_kind() != EntryKind::File {
            return Err(ArchiveError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{wanted:?} is not a file"))));
        }
        return _copy_data(&mut reader, out);
    }
    Err(ArchiveError::NotFound(wanted))
}

/// Write the data of the reader's current entry to `out`, filling the holes
/// of sparse entries with zeros.
fn _copy_data(reader: &mut Reader, out: &mut dyn Write) -> Result<()> {
    let mut pos: i64 = 0;
    while let Some((block, offset)) = reader.next_block()? {
        if offset > pos {
            _write_zeros(out, (offset - pos) as u64)?;
        }
        out.write_all(block)?;
        pos = offset + block.len() as i64;
    }
    Ok(())
}

/// Write `count` zero bytes to `out`.
fn _write_zeros(out: &mut dyn Write, mut count: u64) -> Result<()> {
    const ZEROS: [u8; 4096] = [0; 4096];
    while count > 0 {
        let step = count.min(ZEROS.len() as u64) as usize;
        out.write_all(&ZEROS[..step])?;
        count -= step as u64;
    }
    Ok(())
}

/// Extract `only` (names, folders take everything below them) or the whole
/// archive into the existing folder `dest`. Existing files are never
/// overwritten. `cancel` is polled before every entry and every data block.
pub fn extract(archive: &Path, dest: &Path, only: Option<&[String]>, cancel: &dyn Fn() -> bool) -> Result<Extracted> {
    if !dest.is_dir() {
        return Err(ArchiveError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} is not a folder", dest.display()))));
    }
    let selection: Option<Vec<String>> = only.map(|names| names.iter().map(|n| names::normalize(n)).collect());
    if let Some(sel) = &selection {
        if let Some(bad) = sel.iter().find(|n| n.is_empty() || !names::is_safe(n)) {
            return Err(ArchiveError::UnsafePath(bad.clone()));
        }
    }
    let mut reader = Reader::open(archive)?;
    let mut writer = DiskWriter::new()?;
    let mut done = Extracted::default();
    let mut hits = vec![false; selection.as_ref().map_or(0, Vec::len)];
    while reader.next_header()? {
        if cancel() {
            return Err(ArchiveError::Cancelled);
        }
        let name = names::normalize(&reader.entry_path());
        if name.is_empty() {
            continue;
        }
        if let Some(sel) = &selection {
            if !names::is_selected(&name, sel) {
                continue;
            }
            for (hit, wanted) in hits.iter_mut().zip(sel) {
                *hit |= names::is_selected(&name, std::slice::from_ref(wanted));
            }
        }
        let link = reader.entry_hardlink().map(|l| names::normalize(&l));
        if !names::is_safe(&name) || link.as_deref().is_some_and(|l| !names::is_safe(l)) {
            return Err(ArchiveError::UnsafePath(name));
        }
        let is_dir = reader.entry_kind() == EntryKind::Dir;
        if let Ok(existing) = dest.join(&name).symlink_metadata() {
            // A folder already there is kept as it is (its permissions too).
            if !(is_dir && existing.is_dir()) {
                done.skipped += 1;
            }
            continue;
        }
        let entry = writer
            .write_header(&reader, dest, &name, link.as_deref())
            .and_then(|()| _copy_to_disk(&mut reader, &mut writer, cancel))
            .and_then(|()| writer.finish_entry());
        if let Err(e) = entry {
            // The entry did not exist before (checked above), so whatever is
            // there now is a partial write: a later run would skip it as
            // already extracted. Close the writer first so it holds no file.
            drop(writer);
            if !is_dir {
                _remove_partial(&dest.join(&name));
            }
            return Err(e);
        }
        if !is_dir {
            done.written += 1;
        }
    }
    writer.close()?;
    if let (Some(sel), true) = (&selection, hits.contains(&false)) {
        let missing = sel.iter().zip(&hits).find(|(_, hit)| !**hit).map(|(n, _)| n.clone()).unwrap_or_default();
        return Err(ArchiveError::NotFound(missing));
    }
    Ok(done)
}

/// Remove the partly written entry at `path`. A missing entry is fine; any
/// other failure is printed, since the caller already reports the first error.
fn _remove_partial(path: &Path) {
    let Err(e) = std::fs::remove_file(path) else { return };
    if e.kind() == std::io::ErrorKind::NotFound { return; }
    eprintln!("opusdm-archive: cannot remove the partial file {}: {e}", path.display());
}

/// Move the data of the reader's current entry to the disk writer.
fn _copy_to_disk(reader: &mut Reader, writer: &mut DiskWriter, cancel: &dyn Fn() -> bool) -> Result<()> {
    while let Some((block, offset)) = reader.next_block()? {
        if cancel() {
            return Err(ArchiveError::Cancelled);
        }
        writer.write_block(block, offset)?;
    }
    Ok(())
}
