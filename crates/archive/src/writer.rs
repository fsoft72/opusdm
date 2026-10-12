//! Safe wrapper on libarchive's "write to disk" handle. The only place that
//! touches the raw write API.

use crate::error::{ArchiveError, Result};
use crate::reader::Reader;
use libarchive3_sys::ffi;
use std::ffi::{c_void, CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Flags of the disk writer: never climb out of the folder (`..` names,
/// symlinks pointing out), never overwrite, keep permissions and times, and
/// leave the owner alone.
const WRITE_FLAGS: i32 = ffi::ARCHIVE_EXTRACT_SECURE_NODOTDOT
    | ffi::ARCHIVE_EXTRACT_SECURE_SYMLINKS
    | ffi::ARCHIVE_EXTRACT_NO_OVERWRITE
    | ffi::ARCHIVE_EXTRACT_PERM
    | ffi::ARCHIVE_EXTRACT_TIME;

/// Permission bits kept on extracted entries: no setuid, setgid or sticky.
const KEPT_PERM_MASK: u32 = 0o777;

/// An open disk writer.
pub struct DiskWriter {
    raw: *mut ffi::Struct_archive,
}

impl DiskWriter {
    /// A writer with the safe flags of this crate.
    pub fn new() -> Result<Self> {
        // SAFETY: returns an owned handle or null.
        let raw = unsafe { ffi::archive_write_disk_new() };
        if raw.is_null() {
            return Err(ArchiveError::Io(io::Error::new(io::ErrorKind::OutOfMemory, "libarchive is out of memory")));
        }
        let writer = DiskWriter { raw };
        // SAFETY: valid handle.
        unsafe { ffi::archive_write_disk_set_options(raw, WRITE_FLAGS) };
        Ok(writer)
    }

    /// Send the reader's current entry to disk under `dest`, rewriting its
    /// path (and hard link target) to live inside `dest`. `name` and `link`
    /// are the already normalised and checked names. The caller must have
    /// checked that the target does not exist: with `NO_OVERWRITE` libarchive
    /// reports success for an existing file and then rejects its data.
    pub fn write_header(&mut self, reader: &Reader, dest: &Path, name: &str, link: Option<&str>) -> Result<()> {
        let entry = reader.raw_entry();
        let target = _c_path(&dest.join(name))?;
        // SAFETY: `entry` is the reader's current entry; the strings are copied by libarchive.
        unsafe {
            ffi::archive_entry_set_pathname(entry, target.as_ptr());
            if let Some(link) = link {
                let link = _c_path(&dest.join(link))?;
                ffi::archive_entry_set_hardlink(entry, link.as_ptr());
            }
            let perm = ffi::archive_entry_perm(entry) as u32 & KEPT_PERM_MASK;
            ffi::archive_entry_set_perm(entry, perm as _);
        }
        // SAFETY: valid handles.
        let status = unsafe { ffi::archive_write_header(self.raw, entry) };
        if status < ffi::ARCHIVE_WARN {
            return Err(self._error());
        }
        Ok(())
    }

    /// Write one block of data at `offset` of the current entry.
    pub fn write_block(&mut self, data: &[u8], offset: i64) -> Result<()> {
        // SAFETY: valid handle; `data` is a live slice.
        let written = unsafe { ffi::archive_write_data_block(self.raw, data.as_ptr() as *const c_void, data.len(), offset) };
        if written < 0 {
            return Err(self._error());
        }
        Ok(())
    }

    /// Finish the current entry (permissions, times).
    pub fn finish_entry(&mut self) -> Result<()> {
        // SAFETY: valid handle.
        let status = unsafe { ffi::archive_write_finish_entry(self.raw) };
        if status < ffi::ARCHIVE_WARN {
            return Err(self._error());
        }
        Ok(())
    }

    /// Close the writer, which applies the folder permissions and times kept
    /// for the end, and report a failure of that.
    pub fn close(&mut self) -> Result<()> {
        // SAFETY: valid handle.
        let status = unsafe { ffi::archive_write_close(self.raw) };
        if status < ffi::ARCHIVE_WARN {
            return Err(self._error());
        }
        Ok(())
    }

    /// The last failure kind of this handle, from its errno.
    fn _errno(&self) -> io::ErrorKind {
        // SAFETY: valid handle.
        let errno = unsafe { ffi::archive_errno(self.raw) };
        io::Error::from_raw_os_error(errno).kind()
    }

    /// The last failure as an [`ArchiveError::Io`] with libarchive's message.
    fn _error(&self) -> ArchiveError {
        // SAFETY: valid handle; the string is NUL terminated or null.
        let msg = unsafe {
            let ptr = ffi::archive_error_string(self.raw);
            if ptr.is_null() { String::from("unknown libarchive error") } else { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
        };
        ArchiveError::Io(io::Error::new(self._errno(), msg))
    }
}

impl Drop for DiskWriter {
    /// Free the handle.
    fn drop(&mut self) {
        // SAFETY: `raw` is a handle we own and free exactly once.
        unsafe {
            ffi::archive_write_free(self.raw);
        }
    }
}

/// `path` as a C string, or an error when it holds a NUL byte.
fn _c_path(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| ArchiveError::UnsafePath(path.to_string_lossy().into_owned()))
}
