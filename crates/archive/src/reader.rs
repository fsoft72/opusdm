//! Safe wrapper on a libarchive read handle. The only place that touches the
//! raw read API; every pointer stays inside this module.

use crate::error::{ArchiveError, Result};
use libarchive3_sys::ffi;
use std::ffi::{c_void, CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Size of the blocks libarchive reads from the file.
const READ_BLOCK: usize = 64 * 1024;

/// What an entry is, from its file type bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// An open archive being read entry by entry.
pub struct Reader {
    raw: *mut ffi::Struct_archive,
    /// The current entry; owned by `raw`, valid until the next header.
    entry: *mut ffi::Struct_archive_entry,
}

impl Reader {
    /// Open `path`, detecting format and compression from the content.
    pub fn open(path: &Path) -> Result<Self> {
        let open_err = |msg: String| ArchiveError::Open { path: path.to_path_buf(), msg };
        let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| open_err("path contains a NUL byte".into()))?;
        // SAFETY: `archive_read_new` returns an owned handle or null.
        let raw = unsafe { ffi::archive_read_new() };
        if raw.is_null() {
            return Err(open_err("libarchive is out of memory".into()));
        }
        let reader = Reader { raw, entry: std::ptr::null_mut() };
        // SAFETY: `raw` is a valid read handle; `c_path` outlives the call.
        let status = unsafe {
            ffi::archive_read_support_filter_all(raw);
            ffi::archive_read_support_format_all(raw);
            ffi::archive_read_open_filename(raw, c_path.as_ptr(), READ_BLOCK)
        };
        if status != ffi::ARCHIVE_OK {
            return Err(open_err(reader._message()));
        }
        Ok(reader)
    }

    /// Move to the next entry. `Ok(false)` at the end of the archive.
    pub fn next_header(&mut self) -> Result<bool> {
        // SAFETY: valid handle; `entry` receives a pointer owned by the handle.
        let status = unsafe { ffi::archive_read_next_header(self.raw, &mut self.entry) };
        match status {
            ffi::ARCHIVE_EOF => {
                self.entry = std::ptr::null_mut();
                Ok(false)
            }
            ffi::ARCHIVE_OK | ffi::ARCHIVE_WARN => Ok(true),
            _ => Err(self._error()),
        }
    }

    /// Read the next block of the current entry's data as `(bytes, offset)`.
    /// `Ok(None)` when the entry is exhausted. The slice is valid until the
    /// next call.
    pub fn next_block(&mut self) -> Result<Option<(&[u8], i64)>> {
        let mut buf: *const c_void = std::ptr::null();
        let mut size: usize = 0;
        let mut offset: i64 = 0;
        // SAFETY: valid handle; the out pointers are live locals.
        let status = unsafe { ffi::archive_read_data_block(self.raw, &mut buf, &mut size, &mut offset) };
        match status {
            ffi::ARCHIVE_EOF => Ok(None),
            ffi::ARCHIVE_OK | ffi::ARCHIVE_WARN if buf.is_null() || size == 0 => Ok(Some((&[], offset))),
            // SAFETY: libarchive guarantees `buf` points to `size` readable bytes until the next read.
            ffi::ARCHIVE_OK | ffi::ARCHIVE_WARN => Ok(Some((unsafe { std::slice::from_raw_parts(buf as *const u8, size) }, offset))),
            _ => Err(self._error()),
        }
    }

    /// Path of the current entry, lossy if it is not UTF-8.
    pub fn entry_path(&self) -> String {
        // SAFETY: `entry` is the current entry; the name is a NUL terminated string or null.
        Self::_text(unsafe { ffi::archive_entry_pathname(self.entry) })
    }

    /// Kind of the current entry.
    pub fn entry_kind(&self) -> EntryKind {
        // SAFETY: `entry` is the current entry.
        let kind = unsafe { ffi::archive_entry_filetype(self.entry) } as u32 & ffi::AE_IFMT;
        match kind {
            ffi::AE_IFREG => EntryKind::File,
            ffi::AE_IFDIR => EntryKind::Dir,
            ffi::AE_IFLNK => EntryKind::Symlink,
            _ => EntryKind::Other,
        }
    }

    /// Size in bytes of the current entry, 0 when unknown.
    pub fn entry_size(&self) -> u64 {
        // SAFETY: `entry` is the current entry.
        unsafe {
            if ffi::archive_entry_size_is_set(self.entry) == 0 {
                return 0;
            }
            ffi::archive_entry_size(self.entry).max(0) as u64
        }
    }

    /// Permission bits (no file type) of the current entry.
    pub fn entry_perm(&self) -> u32 {
        // SAFETY: `entry` is the current entry.
        unsafe { ffi::archive_entry_perm(self.entry) as u32 }
    }

    /// Modification time of the current entry, if the archive stores one.
    pub fn entry_mtime(&self) -> Option<i64> {
        // SAFETY: `entry` is the current entry.
        unsafe { (ffi::archive_entry_mtime_is_set(self.entry) != 0).then(|| ffi::archive_entry_mtime(self.entry) as i64) }
    }

    /// Hard link target of the current entry, if it is a hard link.
    pub fn entry_hardlink(&self) -> Option<String> {
        // SAFETY: `entry` is the current entry.
        let ptr = unsafe { ffi::archive_entry_hardlink(self.entry) };
        (!ptr.is_null()).then(|| Self::_text(ptr))
    }

    /// Bytes read so far from the archive file itself (before any
    /// decompression), for a progress bar against the file size.
    pub fn bytes_read(&self) -> u64 {
        // SAFETY: valid handle; filter -1 is the raw file reader.
        unsafe { ffi::archive_filter_bytes(self.raw, -1).max(0) as u64 }
    }

    /// Raw pointer of the current entry, for the disk writer of this crate.
    pub(crate) fn raw_entry(&self) -> *mut ffi::Struct_archive_entry {
        self.entry
    }

    /// The last libarchive message of this handle.
    fn _message(&self) -> String {
        // SAFETY: valid handle; the string is NUL terminated or null.
        Self::_text(unsafe { ffi::archive_error_string(self.raw) })
    }

    /// The last failure as an [`ArchiveError`], recognising encryption.
    fn _error(&self) -> ArchiveError {
        let msg = self._message();
        let lower = msg.to_lowercase();
        if lower.contains("encrypt") || lower.contains("passphrase") || lower.contains("password") {
            return ArchiveError::Encrypted;
        }
        if lower.contains("unsupported") || lower.contains("not supported") {
            return ArchiveError::Unsupported(msg);
        }
        ArchiveError::Read(if msg.is_empty() { "unknown libarchive error".into() } else { msg })
    }

    /// A C string as text; empty for null.
    fn _text(ptr: *const std::ffi::c_char) -> String {
        if ptr.is_null() {
            return String::new();
        }
        // SAFETY: callers pass NUL terminated strings owned by libarchive.
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }
}

impl Drop for Reader {
    /// Close and free the handle.
    fn drop(&mut self) {
        // SAFETY: `raw` is a handle we own and free exactly once.
        unsafe {
            ffi::archive_read_free(self.raw);
        }
    }
}
