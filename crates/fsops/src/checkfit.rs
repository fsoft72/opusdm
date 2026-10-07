//! CheckFit: will a selection fit in the free space of a destination?

use std::io;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// The answer of a fit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckFit {
    /// Bytes the selection needs.
    pub needed: u64,
    /// Bytes available to unprivileged users at the destination.
    pub free: u64,
    /// Whether everything fits.
    pub fits: bool,
    /// Share of the selection that fits, 0..=100 (100 for an empty selection).
    pub percent: u64,
}

/// Build the answer from the two numbers.
pub fn compute(needed: u64, free: u64) -> CheckFit {
    let percent = if needed == 0 {
        100
    } else {
        (u128::from(free) * 100 / u128::from(needed)).min(100) as u64
    };
    CheckFit { needed, free, fits: needed <= free, percent }
}

/// Total size of the regular files under `srcs`. Symlinks are not followed
/// and count zero.
pub fn needed_bytes(srcs: &[PathBuf]) -> io::Result<u64> {
    let mut total = 0u64;
    for src in srcs {
        for entry in WalkDir::new(src).follow_links(false) {
            let entry = entry.map_err(io::Error::other)?;
            if entry.file_type().is_file() {
                total += entry.metadata().map_err(io::Error::other)?.len();
            }
        }
    }
    Ok(total)
}

/// Bytes available to unprivileged users on the filesystem holding `dir`.
pub fn free_bytes(dir: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    // SAFETY: `statvfs` is plain data, zero is a valid initial value, and the
    // call only writes into the buffer we own.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut st) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st.f_bavail * st.f_frsize)
}

/// Size `srcs` and compare with the free space at `dest_dir`.
pub fn check_fit(srcs: &[PathBuf], dest_dir: &Path) -> io::Result<CheckFit> {
    Ok(compute(needed_bytes(srcs)?, free_bytes(dest_dir)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn compute_reports_the_share_that_fits() {
        assert_eq!(compute(2000, 1000), CheckFit { needed: 2000, free: 1000, fits: false, percent: 50 });
        assert_eq!(compute(100, 1000), CheckFit { needed: 100, free: 1000, fits: true, percent: 100 });
        assert!(compute(1000, 1000).fits);
        assert_eq!(compute(0, 0), CheckFit { needed: 0, free: 0, fits: true, percent: 100 });
        assert_eq!(compute(u64::MAX, 1).percent, 0);
    }

    #[test]
    fn needed_bytes_sums_files_recursively_and_ignores_symlinks() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("r");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a"), vec![0u8; 100]).unwrap();
        fs::write(root.join("sub/b"), vec![0u8; 250]).unwrap();
        std::os::unix::fs::symlink(root.join("a"), root.join("link")).unwrap();
        let single = td.path().join("single");
        fs::write(&single, vec![0u8; 7]).unwrap();

        assert_eq!(needed_bytes(&[root, single]).unwrap(), 357);
    }

    #[test]
    fn free_bytes_reads_the_filesystem_and_fails_for_missing_paths() {
        let td = tempfile::tempdir().unwrap();
        assert!(free_bytes(td.path()).unwrap() > 0);
        assert!(free_bytes(&td.path().join("nope")).is_err());
    }

    #[test]
    fn check_fit_combines_both() {
        let td = tempfile::tempdir().unwrap();
        let f = td.path().join("f");
        fs::write(&f, "hello").unwrap();
        let r = check_fit(&[f], td.path()).unwrap();
        assert_eq!(r.needed, 5);
        assert!(r.fits);
    }
}
