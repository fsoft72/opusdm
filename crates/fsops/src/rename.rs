//! Rename that never replaces an existing entry.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Rename `from` to `to`, failing with `ErrorKind::AlreadyExists` when `to`
/// exists (`std::fs::rename` would silently replace it).
///
/// Uses `renameat2(RENAME_NOREPLACE)`, which is atomic. On kernels or
/// filesystems without support it falls back to an existence check followed by
/// a plain rename (racy, but never worse than `std::fs::rename`). Renaming a
/// path to itself is a no-op.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    if from == to {
        return Ok(());
    }
    let c_from = CString::new(from.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let c_to = CString::new(to.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            c_from.as_ptr(),
            libc::AT_FDCWD,
            c_to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if ret == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP) => {
            if to.symlink_metadata().is_ok() {
                return Err(io::Error::from(io::ErrorKind::AlreadyExists));
            }
            std::fs::rename(from, to)
        }
        _ => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// An existing target is refused and both files survive untouched.
    #[test]
    fn refuses_to_replace_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let err = rename_noreplace(&a, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&a).unwrap(), b"a");
        assert_eq!(fs::read(&b).unwrap(), b"b");
    }

    /// A free target works like a normal rename.
    #[test]
    fn renames_to_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let (a, c) = (dir.path().join("a.txt"), dir.path().join("c.txt"));
        fs::write(&a, b"a").unwrap();
        rename_noreplace(&a, &c).unwrap();
        assert!(!a.exists());
        assert_eq!(fs::read(&c).unwrap(), b"a");
    }
}
