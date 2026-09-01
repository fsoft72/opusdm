//! The directory listing model: reading entries and formatting them for the
//! column view. Pure and deterministic; no GTK.

use std::cmp::Ordering;
use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// One row in a Lister's file view.
#[derive(Clone)]
pub struct Row {
    /// File name (no directory part).
    pub name: String,
    /// Whether this entry is a directory (following symlinks).
    pub is_dir: bool,
    /// Whether the entry itself is a symbolic link.
    pub is_symlink: bool,
    /// A symlink whose target does not resolve.
    pub broken_link: bool,
    /// Size in bytes (0 for directories and broken links).
    pub size: u64,
    /// Modification time, whole seconds since the Unix epoch.
    pub modified_unix: i64,
    /// User ID (owner).
    pub uid: u32,
    /// Group ID (owner group).
    pub gid: u32,
    /// File mode (permissions + type bits).
    pub mode: u32,
}

/// Which column an ordering is keyed on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortCol {
    /// By name, case-insensitive.
    Name,
    /// By byte size.
    Size,
    /// By modification time.
    Modified,
}

/// Read `dir` into rows. `.` and `..` are excluded. A symlink's `is_dir`
/// follows its target; `broken_link` is set when the target does not resolve.
pub fn read_dir_rows(dir: &Path) -> io::Result<Vec<Row>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let link_meta = std::fs::symlink_metadata(entry.path())?;
        let is_symlink = link_meta.file_type().is_symlink();
        let target_meta = std::fs::metadata(entry.path()); // follows symlinks
        let (is_dir, size, broken_link) = match &target_meta {
            Ok(m) => (m.is_dir(), if m.is_dir() { 0 } else { m.len() }, false),
            Err(_) => (false, 0, is_symlink),
        };
        let mtime_source = match &target_meta {
            Ok(m) => m,
            Err(_) => &link_meta,
        };
        let modified_unix = mtime_source
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (uid, gid, mode) = metadata_perms(&link_meta);
        rows.push(Row { name, is_dir, is_symlink, broken_link, size, modified_unix, uid, gid, mode });
    }
    Ok(rows)
}

#[cfg(unix)]
fn metadata_perms(m: &std::fs::Metadata) -> (u32, u32, u32) {
    use std::os::unix::fs::MetadataExt;
    (m.uid(), m.gid(), m.mode())
}

#[cfg(not(unix))]
fn metadata_perms(_m: &std::fs::Metadata) -> (u32, u32, u32) {
    (0, 0, 0)
}

/// Human-readable size. Empty string for directories.
pub fn format_size(is_dir: bool, size: u64) -> String {
    if is_dir {
        return String::new();
    }
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    if size < 1024 {
        return format!("{size} B");
    }
    let mut value = size as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1}{}", UNITS[unit])
}

/// Format `unix` seconds as `YYYY-MM-DD HH:MM` at `tz_offset_secs` east of UTC.
pub fn format_mtime(unix: i64, tz_offset_secs: i32) -> String {
    let local = unix + tz_offset_secs as i64;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}

/// Days since 1970-01-01 -> (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + i64::from(m <= 2), m, d)
}

/// Compare two rows: directories first (both directions), then by `col`.
pub fn dir_first_cmp(a: &Row, b: &Row, col: SortCol, desc: bool) -> Ordering {
    if a.is_dir != b.is_dir {
        return if a.is_dir { Ordering::Less } else { Ordering::Greater };
    }
    let base = match col {
        SortCol::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortCol::Size => a.size.cmp(&b.size),
        SortCol::Modified => a.modified_unix.cmp(&b.modified_unix),
    };
    if desc {
        base.reverse()
    } else {
        base
    }
}

/// Human-readable user name or UID fallback.
pub fn format_user(uid: u32) -> String {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        let mut passwd_buf = [0u8; 16384];
        let mut passwd_ptr = std::ptr::null_mut();
        let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
        let uid_ctype = uid as libc::uid_t;
        // SAFETY: buffers are properly sized; function fills or returns error.
        let ret = unsafe { libc::getpwuid_r(uid_ctype, &mut passwd, passwd_buf.as_mut_ptr() as *mut _, passwd_buf.len(), &mut passwd_ptr) };
        if ret == 0 && !passwd_ptr.is_null() {
            if let Ok(name) = unsafe { CStr::from_ptr((*passwd_ptr).pw_name) }.to_str() {
                return name.to_string();
            }
        }
    }
    uid.to_string()
}

/// Human-readable group name or GID fallback.
pub fn format_group(gid: u32) -> String {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        let mut group_buf = [0u8; 16384];
        let mut group_ptr = std::ptr::null_mut();
        let mut group: libc::group = unsafe { std::mem::zeroed() };
        let gid_ctype = gid as libc::gid_t;
        // SAFETY: buffers are properly sized; function fills or returns error.
        let ret = unsafe { libc::getgrgid_r(gid_ctype, &mut group, group_buf.as_mut_ptr() as *mut _, group_buf.len(), &mut group_ptr) };
        if ret == 0 && !group_ptr.is_null() {
            if let Ok(name) = unsafe { CStr::from_ptr((*group_ptr).gr_name) }.to_str() {
                return name.to_string();
            }
        }
    }
    gid.to_string()
}

/// Rwx permission string for access, prefixed with the entry type ('d'
/// directory, 'l' symlink, '-' regular file), e.g. "drwxr-xr--".
pub fn format_access(mode: u32, is_dir: bool, is_symlink: bool) -> String {
    const BITS: [(u32, char); 9] = [
        (0o400, 'r'), (0o200, 'w'), (0o100, 'x'),
        (0o040, 'r'), (0o020, 'w'), (0o010, 'x'),
        (0o004, 'r'), (0o002, 'w'), (0o001, 'x'),
    ];
    let kind = if is_symlink {
        'l'
    } else if is_dir {
        'd'
    } else {
        '-'
    };
    std::iter::once(kind)
        .chain(BITS.iter().map(|&(bit, c)| if mode & bit != 0 { c } else { '-' }))
        .collect()
}

/// Best-effort local timezone offset in seconds east of UTC. Falls back to 0.
pub fn local_tz_offset_secs() -> i32 {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    // SAFETY: `tm` is a valid out-pointer; localtime_r fills it or returns null.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&now, &mut tm).is_null() };
    if ok {
        tm.tm_gmtoff as i32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;
    use std::fs;

    #[test]
    fn format_size_rules() {
        assert_eq!(format_size(true, 4096), "");
        assert_eq!(format_size(false, 0), "0 B");
        assert_eq!(format_size(false, 512), "512 B");
        assert_eq!(format_size(false, 1024), "1.0K");
        assert_eq!(format_size(false, 1536), "1.5K");
        assert_eq!(format_size(false, 3 * 1024 * 1024), "3.0M");
    }

    #[test]
    fn format_mtime_utc_epoch_and_known_value() {
        assert_eq!(format_mtime(0, 0), "1970-01-01 00:00");
        assert_eq!(format_mtime(1_700_000_000, 0), "2023-11-14 22:13");
        assert_eq!(format_mtime(1_700_000_000, 3600), "2023-11-14 23:13");
    }

    fn row(name: &str, is_dir: bool, size: u64, mtime: i64) -> Row {
        Row {
            name: name.into(),
            is_dir,
            is_symlink: false,
            broken_link: false,
            size,
            modified_unix: mtime,
            uid: 0,
            gid: 0,
            mode: 0,
        }
    }

    #[test]
    fn dirs_sort_before_files_even_descending() {
        let d = row("zeta", true, 0, 0);
        let f = row("alpha", false, 0, 0);
        assert_eq!(dir_first_cmp(&d, &f, SortCol::Name, false), Ordering::Less);
        assert_eq!(dir_first_cmp(&d, &f, SortCol::Name, true), Ordering::Less);
        assert_eq!(dir_first_cmp(&f, &d, SortCol::Name, true), Ordering::Greater);
    }

    #[test]
    fn name_sort_is_case_insensitive() {
        let a = row("Apple", false, 0, 0);
        let b = row("banana", false, 0, 0);
        assert_eq!(dir_first_cmp(&a, &b, SortCol::Name, false), Ordering::Less);
    }

    #[test]
    fn size_and_mtime_sort_and_reverse() {
        let small = row("s", false, 10, 100);
        let big = row("b", false, 20, 50);
        assert_eq!(dir_first_cmp(&small, &big, SortCol::Size, false), Ordering::Less);
        assert_eq!(dir_first_cmp(&small, &big, SortCol::Size, true), Ordering::Greater);
        assert_eq!(dir_first_cmp(&small, &big, SortCol::Modified, false), Ordering::Greater);
    }

    #[test]
    fn read_dir_rows_reports_kinds() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        fs::create_dir(base.join("adir")).unwrap();
        fs::write(base.join("afile"), b"12345").unwrap();
        std::os::unix::fs::symlink("afile", base.join("good_link")).unwrap();
        std::os::unix::fs::symlink("missing", base.join("bad_link")).unwrap();

        let mut rows = read_dir_rows(base).unwrap();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        let by = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();

        assert!(by("adir").is_dir);
        assert_eq!(by("afile").size, 5);
        assert!(by("good_link").is_symlink && !by("good_link").is_dir && !by("good_link").broken_link);
        assert!(by("bad_link").is_symlink && by("bad_link").broken_link);
    }
}
