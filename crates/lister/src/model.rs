//! The directory listing model: reading entries and formatting them for the
//! column view. Pure and deterministic; no GTK.

use opusdm_ipc::{FormatColumn, Separate};
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
    /// Lowercased `name`, precomputed once so the sort comparator never
    /// allocates or re-lowercases per comparison.
    pub name_lower: String,
    /// Lowercased owner user name, precomputed once (also avoids taking the
    /// `format_user` cache lock inside the comparator).
    pub user_lower: String,
    /// Lowercased owner group name, precomputed once, same reason.
    pub group_lower: String,
}

impl Row {
    /// Build a `Row`, deriving the precomputed sort keys from the given fields.
    pub fn new(
        name: String,
        is_dir: bool,
        is_symlink: bool,
        broken_link: bool,
        size: u64,
        modified_unix: i64,
        uid: u32,
        gid: u32,
        mode: u32,
    ) -> Self {
        let name_lower = name.to_lowercase();
        let user_lower = format_user(uid).to_lowercase();
        let group_lower = format_group(gid).to_lowercase();
        Row {
            name,
            is_dir,
            is_symlink,
            broken_link,
            size,
            modified_unix,
            uid,
            gid,
            mode,
            name_lower,
            user_lower,
            group_lower,
        }
    }
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
    /// By owner user name, case-insensitive.
    User,
    /// By owner group name, case-insensitive.
    Group,
    /// By file mode bits.
    Access,
}

/// Read `dir` into rows. `.` and `..` are excluded. A symlink's `is_dir`
/// follows its target; `broken_link` is set when the target does not resolve.
pub fn read_dir_rows(dir: &Path) -> io::Result<Vec<Row>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        // Per-entry failures (transient races, one unreadable file) skip just
        // that entry instead of aborting an otherwise valid listing.
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let is_symlink = ft.is_symlink();
        let (is_dir, size, broken_link, modified_unix, uid, gid, mode) = if is_symlink {
            // The common case (a live target) needs only one metadata call:
            // size, mtime and permissions all come from the target, same as
            // a plain file. The `lstat` on the link itself is only needed as
            // a fallback when the target doesn't resolve.
            match std::fs::metadata(entry.path()) {
                Ok(m) => {
                    let is_dir = m.is_dir();
                    let size = if is_dir { 0 } else { m.len() };
                    let modified_unix = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let (uid, gid, mode) = metadata_perms(&m);
                    (is_dir, size, false, modified_unix, uid, gid, mode)
                }
                Err(_) => {
                    let link_meta = match std::fs::symlink_metadata(entry.path()) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    let modified_unix = link_meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let (uid, gid, mode) = metadata_perms(&link_meta);
                    (false, 0, true, modified_unix, uid, gid, mode)
                }
            }
        } else {
            // Direct entry (not a symlink) - single metadata call needed!
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let is_dir = meta.is_dir();
            let size = if is_dir { 0 } else { meta.len() };
            let modified_unix = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let (uid, gid, mode) = metadata_perms(&meta);
            (is_dir, size, false, modified_unix, uid, gid, mode)
        };
        rows.push(Row::new(name, is_dir, is_symlink, broken_link, size, modified_unix, uid, gid, mode));
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

/// How the Size column of one Lister is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SizeFormat {
    /// Binary units with one decimal (`1.5K`, `3.0M`). The default.
    #[default]
    Auto,
    /// Decimal (SI, base 1000) units with one decimal (`1.5kB`, `3.0MB`).
    Si,
    /// The exact byte count with thousands separators (`1,234,567 B`).
    Bytes,
    /// Always kilobytes (base 1024), two decimals.
    Kilobytes,
    /// Always megabytes (base 1024), two decimals.
    Megabytes,
    /// Always gigabytes (base 1024), two decimals.
    Gigabytes,
}

impl SizeFormat {
    /// Every format, in the order the requester lists them.
    pub const ALL: [SizeFormat; 6] = [
        SizeFormat::Auto,
        SizeFormat::Si,
        SizeFormat::Bytes,
        SizeFormat::Kilobytes,
        SizeFormat::Megabytes,
        SizeFormat::Gigabytes,
    ];

    /// The label shown in the requester.
    pub fn label(self) -> &'static str {
        match self {
            SizeFormat::Auto => "Auto (1.5K, 3.0M ...)",
            SizeFormat::Si => "Auto, decimal units (1.5kB, 3.0MB ...)",
            SizeFormat::Bytes => "Exact bytes (1,234,567 B)",
            SizeFormat::Kilobytes => "Always KB (1.18 KB)",
            SizeFormat::Megabytes => "Always MB (1.18 MB)",
            SizeFormat::Gigabytes => "Always GB (1.18 GB)",
        }
    }
}

/// Human-readable size. Empty string for directories.
pub fn format_size(is_dir: bool, size: u64) -> String {
    format_size_as(SizeFormat::Auto, is_dir, size)
}

/// `size` written the way `fmt` asks. Empty string for directories.
pub fn format_size_as(fmt: SizeFormat, is_dir: bool, size: u64) -> String {
    if is_dir {
        return String::new();
    }
    match fmt {
        SizeFormat::Auto => scaled(size, 1024.0, &["B", "K", "M", "G", "T", "P"]),
        SizeFormat::Si => scaled(size, 1000.0, &["B", "kB", "MB", "GB", "TB", "PB"]),
        SizeFormat::Bytes => format!("{} B", group_thousands(size)),
        SizeFormat::Kilobytes => format!("{:.2} KB", size as f64 / 1024.0),
        SizeFormat::Megabytes => format!("{:.2} MB", size as f64 / 1024.0 / 1024.0),
        SizeFormat::Gigabytes => format!("{:.2} GB", size as f64 / 1024.0 / 1024.0 / 1024.0),
    }
}

/// `size` divided down by `base` until it fits, one decimal. Under one
/// `base` it is a plain byte count.
fn scaled(size: u64, base: f64, units: &[&str]) -> String {
    if (size as f64) < base {
        return format!("{size} B");
    }
    let mut value = size as f64;
    let mut unit = 0usize;
    while value >= base && unit < units.len() - 1 {
        value /= base;
        unit += 1;
    }
    format!("{value:.1}{}", units[unit])
}

/// `n` with a comma between every group of three digits.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
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

/// Compare two rows: directories and files are kept in separate blocks as
/// `separate` says (or sorted together for `Mixed`), then ordered by `col`.
/// `desc` reverses the key within a block, never the block order.
pub fn dir_first_cmp(a: &Row, b: &Row, col: SortCol, desc: bool, separate: Separate) -> Ordering {
    if a.is_dir != b.is_dir {
        match separate {
            Separate::DirsFirst => return if a.is_dir { Ordering::Less } else { Ordering::Greater },
            Separate::FilesFirst => return if a.is_dir { Ordering::Greater } else { Ordering::Less },
            Separate::Mixed => {}
        }
    }
    let base = match col {
        SortCol::Name => a.name_lower.cmp(&b.name_lower),
        SortCol::Size => a.size.cmp(&b.size),
        SortCol::Modified => a.modified_unix.cmp(&b.modified_unix),
        SortCol::User => a.user_lower.cmp(&b.user_lower),
        SortCol::Group => a.group_lower.cmp(&b.group_lower),
        SortCol::Access => a.mode.cmp(&b.mode),
    };
    if desc {
        base.reverse()
    } else {
        base
    }
}

impl From<FormatColumn> for SortCol {
    /// The ordering key of a format column.
    fn from(column: FormatColumn) -> Self {
        match column {
            FormatColumn::Name => SortCol::Name,
            FormatColumn::Size => SortCol::Size,
            FormatColumn::Date => SortCol::Modified,
            FormatColumn::User => SortCol::User,
            FormatColumn::Group => SortCol::Group,
            FormatColumn::Access => SortCol::Access,
        }
    }
}

use std::collections::HashMap;
use std::sync::Mutex;

static USER_CACHE: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);
static GROUP_CACHE: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

/// The rows of a listing that pass the hidden-file and name filters, with the
/// totals the status bar shows.
#[derive(Default)]
pub struct VisibleRows {
    pub rows: Vec<Row>,
    pub dirs: u32,
    pub files: u32,
    pub bytes: u64,
}

/// Reduce a raw listing to what the Lister shows: drops the `.opusdm` folder,
/// hidden entries unless `show_hidden`, and files rejected by `filter`
/// (directories are never name-filtered). Directories take their remembered
/// size from `dir_size`. Totals count only the kept rows.
pub fn visible_rows(
    rows: Vec<Row>,
    show_hidden: bool,
    filter: &opusdm_ipc::NameFilter,
    dir_size: impl Fn(&str) -> Option<u64>,
) -> VisibleRows {
    let mut out = VisibleRows::default();
    for mut row in rows {
        if row.name == opusdm_iconview::scan::OPUSDM_DIR {
            continue;
        }
        if row.is_dir {
            if let Some(size) = dir_size(&row.name) {
                row.size = size;
            }
        }
        if !show_hidden && row.name.starts_with('.') {
            continue;
        }
        if !row.is_dir && !filter.allows(&row.name) {
            continue;
        }
        if row.is_dir {
            out.dirs += 1;
        } else {
            out.files += 1;
            out.bytes += row.size;
        }
        out.rows.push(row);
    }
    out
}

/// Look `id` up in `cache`, calling `resolve` (and remembering the answer) on a
/// miss. A poisoned lock just skips the cache.
fn cached_name(cache: &Mutex<Option<HashMap<u32, String>>>, id: u32, resolve: fn(u32) -> String) -> String {
    let Ok(mut guard) = cache.lock() else { return resolve(id) };
    let names = guard.get_or_insert_with(HashMap::new);
    if let Some(name) = names.get(&id) {
        return name.clone();
    }
    let resolved = resolve(id);
    names.insert(id, resolved.clone());
    resolved
}

/// Human-readable user name or UID fallback (cached globally).
pub fn format_user(uid: u32) -> String {
    cached_name(&USER_CACHE, uid, resolve_user_nss)
}

fn resolve_user_nss(uid: u32) -> String {
    crate::passwd::lookup(uid).map(|e| e.name).unwrap_or_else(|| uid.to_string())
}

/// Human-readable group name or GID fallback (cached globally).
pub fn format_group(gid: u32) -> String {
    cached_name(&GROUP_CACHE, gid, resolve_group_nss)
}

fn resolve_group_nss(gid: u32) -> String {
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

/// Map a listing `Row` to an `opusdm_iconview::IconEntry`. `dir` is the folder
/// the row lives in, used to build the absolute path.
pub fn row_to_entry(row: &Row, dir: &Path) -> opusdm_iconview::IconEntry {
    use opusdm_iconview::IconKind;
    let kind = if row.broken_link {
        IconKind::Broken
    } else if row.is_symlink {
        IconKind::Symlink
    } else if row.is_dir {
        IconKind::Dir
    } else {
        IconKind::File
    };
    let ext = if row.is_dir { None } else { opusdm_iconview::IconEntry::ext_of(&row.name) };
    opusdm_iconview::IconEntry {
        name: row.name.clone(),
        path: dir.join(&row.name),
        kind,
        mode: row.mode,
        ext,
        modified: row.modified_unix,
        left_out: false,
        group: false,
    }
}

/// Best-effort local timezone offset in seconds east of UTC at the moment
/// `unix` (so a summer file keeps its summer offset in winter). Falls back to 0.
pub fn local_tz_offset_secs_at(unix: i64) -> i32 {
    let at = unix as libc::time_t;
    // SAFETY: `tm` is a valid out-pointer; localtime_r fills it or returns null.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&at, &mut tm).is_null() };
    if ok {
        tm.tm_gmtoff as i32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::{FormatColumn, Separate};
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
    fn size_formats() {
        use SizeFormat::*;
        assert_eq!(format_size_as(Bytes, true, 99), "");
        assert_eq!(format_size_as(Bytes, false, 0), "0 B");
        assert_eq!(format_size_as(Bytes, false, 999), "999 B");
        assert_eq!(format_size_as(Bytes, false, 1_234_567), "1,234,567 B");
        assert_eq!(format_size_as(Si, false, 999), "999 B");
        assert_eq!(format_size_as(Si, false, 1500), "1.5kB");
        assert_eq!(format_size_as(Si, false, 3_000_000), "3.0MB");
        assert_eq!(format_size_as(Kilobytes, false, 0), "0.00 KB");
        assert_eq!(format_size_as(Kilobytes, false, 1536), "1.50 KB");
        assert_eq!(format_size_as(Megabytes, false, 1_234_567), "1.18 MB");
        assert_eq!(format_size_as(Gigabytes, false, 3 * 1024 * 1024 * 1024), "3.00 GB");
        assert_eq!(format_size_as(Auto, false, 1023), "1023 B");
        assert_eq!(format_size_as(Auto, false, 1024), "1.0K");
    }

    #[test]
    fn visible_rows_filters_and_totals_what_is_kept() {
        let rows = vec![
            row("src", true, 0, 0),
            row(".git", true, 0, 0),
            row(opusdm_iconview::scan::OPUSDM_DIR, true, 0, 0),
            row("a.rs", false, 10, 0),
            row("b.txt", false, 5, 0),
            row(".hidden", false, 100, 0),
        ];
        let filter = opusdm_ipc::NameFilter::new("*.rs", "");
        let out = visible_rows(rows.clone(), false, &filter, |name| (name == "src").then_some(4096));
        let names: Vec<_> = out.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["src", "a.rs"], "hidden, .opusdm and non matching files dropped");
        assert_eq!((out.dirs, out.files, out.bytes), (1, 1, 10));
        assert_eq!(out.rows[0].size, 4096, "the remembered directory size is applied");

        let all = visible_rows(rows, true, &opusdm_ipc::NameFilter::new("", ""), |_| None);
        assert_eq!((all.dirs, all.files, all.bytes), (2, 3, 115));
    }

    #[test]
    fn cached_name_resolves_once_per_id() {
        static CACHE: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn resolve(id: u32) -> String {
            CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            format!("name{id}")
        }
        assert_eq!(cached_name(&CACHE, 7, resolve), "name7");
        assert_eq!(cached_name(&CACHE, 7, resolve), "name7");
        assert_eq!(cached_name(&CACHE, 8, resolve), "name8");
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn tz_offset_follows_daylight_saving_of_the_timestamp() {
        extern "C" {
            fn tzset();
        }
        static TZ_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = TZ_LOCK.lock().unwrap();
        let old = std::env::var_os("TZ");
        std::env::set_var("TZ", "Europe/Rome");
        // SAFETY: tzset only re-reads the TZ variable.
        unsafe { tzset() };
        let winter = local_tz_offset_secs_at(1_705_312_800); // 2024-01-15
        let summer = local_tz_offset_secs_at(1_721_037_600); // 2024-07-15
        match old {
            Some(v) => std::env::set_var("TZ", v),
            None => std::env::remove_var("TZ"),
        }
        unsafe { tzset() };
        assert_eq!((winter, summer), (3600, 7200));
    }

    #[test]
    fn format_mtime_utc_epoch_and_known_value() {
        assert_eq!(format_mtime(0, 0), "1970-01-01 00:00");
        assert_eq!(format_mtime(1_700_000_000, 0), "2023-11-14 22:13");
        assert_eq!(format_mtime(1_700_000_000, 3600), "2023-11-14 23:13");
    }

    fn row(name: &str, is_dir: bool, size: u64, mtime: i64) -> Row {
        Row::new(name.into(), is_dir, false, false, size, mtime, 0, 0, 0)
    }

    #[test]
    fn row_to_entry_maps_kind_and_path() {
        let f = row("photo.JPG", false, 10, 0);
        let e = row_to_entry(&f, Path::new("/home/x"));
        assert_eq!(e.path, Path::new("/home/x/photo.JPG"));
        assert_eq!(e.ext.as_deref(), Some("jpg"));
        assert_eq!(e.kind, opusdm_iconview::IconKind::File);

        let d = row("src", true, 0, 0);
        assert_eq!(row_to_entry(&d, Path::new("/home/x")).kind, opusdm_iconview::IconKind::Dir);
        assert_eq!(row_to_entry(&d, Path::new("/home/x")).ext, None);
    }

    #[test]
    fn dirs_sort_before_files_even_descending() {
        let d = row("zeta", true, 0, 0);
        let f = row("alpha", false, 0, 0);
        assert_eq!(dir_first_cmp(&d, &f, SortCol::Name, false, Separate::DirsFirst), Ordering::Less);
        assert_eq!(dir_first_cmp(&d, &f, SortCol::Name, true, Separate::DirsFirst), Ordering::Less);
        assert_eq!(dir_first_cmp(&f, &d, SortCol::Name, true, Separate::DirsFirst), Ordering::Greater);
    }

    #[test]
    fn name_sort_is_case_insensitive() {
        let a = row("Apple", false, 0, 0);
        let b = row("banana", false, 0, 0);
        assert_eq!(dir_first_cmp(&a, &b, SortCol::Name, false, Separate::DirsFirst), Ordering::Less);
    }

    #[test]
    fn size_and_mtime_sort_and_reverse() {
        let small = row("s", false, 10, 100);
        let big = row("b", false, 20, 50);
        assert_eq!(dir_first_cmp(&small, &big, SortCol::Size, false, Separate::DirsFirst), Ordering::Less);
        assert_eq!(dir_first_cmp(&small, &big, SortCol::Size, true, Separate::DirsFirst), Ordering::Greater);
        assert_eq!(dir_first_cmp(&small, &big, SortCol::Modified, false, Separate::DirsFirst), Ordering::Greater);
    }

    #[test]
    fn access_sort_uses_mode_bits() {
        let mut ro = row("ro", false, 0, 0);
        ro.mode = 0o100_644;
        let mut rwx = row("rwx", false, 0, 0);
        rwx.mode = 0o100_755;
        assert_eq!(dir_first_cmp(&ro, &rwx, SortCol::Access, false, Separate::DirsFirst), Ordering::Less);
        assert_eq!(dir_first_cmp(&ro, &rwx, SortCol::Access, true, Separate::DirsFirst), Ordering::Greater);
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

    /// A live symlink's size, mtime and mode reflect its target, matching
    /// the target's own row - the single `metadata` (stat) call the fast
    /// path uses gets all three from the same place a plain file's row does.
    #[test]
    fn live_symlink_reports_target_size_and_mode() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        fs::write(base.join("target.txt"), b"12345").unwrap();
        fs::set_permissions(base.join("target.txt"), fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink("target.txt", base.join("link")).unwrap();

        let rows = read_dir_rows(base).unwrap();
        let link = rows.iter().find(|r| r.name == "link").unwrap();
        let target = rows.iter().find(|r| r.name == "target.txt").unwrap();
        assert_eq!(link.size, target.size);
        assert_eq!(link.mode & 0o777, target.mode & 0o777);
        assert!(!link.broken_link);
    }

    /// One unreadable entry must not abort the whole listing: a directory
    /// with read-but-not-execute permission lists names via `read_dir`, but
    /// `stat` on its children fails - those entries are skipped, not fatal.
    /// (Trivially passes when running as root, which bypasses permissions.)
    #[test]
    #[cfg(unix)]
    fn read_dir_rows_skips_entries_whose_metadata_fails() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        let locked = base.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("inner.txt"), b"x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o444)).unwrap();
        let rows = read_dir_rows(&locked);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let rows = rows.expect("one unreadable entry must not abort the listing");
        if !nix_is_root() {
            assert!(rows.is_empty(), "the unreadable entry is skipped, not listed");
        }
    }

    #[cfg(unix)]
    fn nix_is_root() -> bool {
        // No libc dependency in this crate; uid 0 bypasses file permissions.
        std::env::var_os("USER").is_some_and(|u| u == "root")
            || std::process::Command::new("id").arg("-u")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .is_some_and(|s| s.trim() == "0")
    }

    fn sorted(names: &mut Vec<Row>, desc: bool, separate: Separate) -> Vec<String> {
        names.sort_by(|a, b| dir_first_cmp(a, b, SortCol::Name, desc, separate));
        names.iter().map(|r| r.name.clone()).collect()
    }

    fn sample() -> Vec<Row> {
        vec![row("b_file", false, 1, 0), row("a_dir", true, 0, 0), row("a_file", false, 1, 0), row("c_dir", true, 0, 0)]
    }

    #[test]
    fn dirs_first_keeps_a_leading_directory_block_in_both_directions() {
        assert_eq!(sorted(&mut sample(), false, Separate::DirsFirst), ["a_dir", "c_dir", "a_file", "b_file"]);
        assert_eq!(sorted(&mut sample(), true, Separate::DirsFirst), ["c_dir", "a_dir", "b_file", "a_file"]);
    }

    #[test]
    fn files_first_puts_directories_last() {
        assert_eq!(sorted(&mut sample(), false, Separate::FilesFirst), ["a_file", "b_file", "a_dir", "c_dir"]);
        assert_eq!(sorted(&mut sample(), true, Separate::FilesFirst), ["b_file", "a_file", "c_dir", "a_dir"]);
    }

    #[test]
    fn mixed_sorts_directories_and_files_together() {
        assert_eq!(sorted(&mut sample(), false, Separate::Mixed), ["a_dir", "a_file", "b_file", "c_dir"]);
        assert_eq!(sorted(&mut sample(), true, Separate::Mixed), ["c_dir", "b_file", "a_file", "a_dir"]);
    }

    #[test]
    fn format_columns_map_to_sort_columns() {
        assert!(SortCol::from(FormatColumn::Date) == SortCol::Modified);
        assert!(SortCol::from(FormatColumn::Access) == SortCol::Access);
        assert!(SortCol::from(FormatColumn::Name) == SortCol::Name);
    }
}
