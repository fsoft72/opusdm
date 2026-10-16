//! Free-space gauge in the status bar.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use std::io;
use std::path::Path;

/// (total bytes, bytes free to unprivileged users) of the filesystem holding
/// `dir`.
// The statvfs fields are 32 bits wide on some targets, so the conversions
// are useless only on 64-bit ones.
#[allow(clippy::useless_conversion)]
pub fn usage(dir: &Path) -> io::Result<(u64, u64)> {
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
    let frsize = u64::from(st.f_frsize);
    Ok((u64::from(st.f_blocks) * frsize, u64::from(st.f_bavail) * frsize))
}

/// Share of the filesystem in use, `0.0..=1.0`. An empty filesystem reads as
/// 0 so the bar never divides by zero.
pub fn used_fraction(total: u64, free: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    (total.saturating_sub(free) as f64 / total as f64).clamp(0.0, 1.0)
}

/// Human readable size for the tooltip (binary units).
fn human(bytes: u64) -> String {
    opusdm_ipc::size::binary_size(bytes)
}

impl Ui {
    /// Show or hide the gauge and, when shown, fill it for `dir`. Hidden when
    /// the format does not ask for it, inside an archive, or when the
    /// filesystem cannot be queried. `statvfs` can block on a stalled mount,
    /// so it runs on a thread; a result that arrives after a newer request is
    /// dropped.
    pub fn update_gauge(&self, dir: &Path) {
        let bar = self.status_bar.gauge.clone();
        let generation = self.status_bar.gauge_generation.clone();
        let this_request = generation.get() + 1;
        generation.set(this_request);
        let wanted = self.format_state.current.borrow().gauge && crate::archive::parse_archive_path(dir).is_none() && crate::remote::url_of(dir).is_none();
        if !wanted {
            bar.set_visible(false);
            return;
        }
        let dir = dir.to_path_buf();
        crate::background::run_blocking(move || usage(&dir).ok(), move |result| {
            if generation.get() != this_request {
                return;
            }
            let Some((total, free)) = result else {
                bar.set_visible(false);
                return;
            };
            bar.set_value(used_fraction(total, free));
            bar.set_tooltip_text(Some(&format!("{} free of {}", human(free), human(total))));
            bar.set_visible(true);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_used_share_is_total_minus_free_over_total() {
        assert_eq!(used_fraction(1000, 250), 0.75);
        assert_eq!(used_fraction(1000, 1000), 0.0);
        assert_eq!(used_fraction(1000, 0), 1.0);
    }

    #[test]
    fn an_empty_or_inconsistent_filesystem_never_divides_by_zero_or_overflows() {
        assert_eq!(used_fraction(0, 0), 0.0);
        assert_eq!(used_fraction(100, 500), 0.0, "free above total clamps to empty use");
    }

    #[test]
    fn the_root_filesystem_reports_a_total_at_least_as_big_as_the_free_space() {
        let (total, free) = usage(Path::new("/")).unwrap();
        assert!(total > 0);
        assert!(free <= total);
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        assert!(usage(Path::new("/definitely/not/here")).is_err());
    }

    #[test]
    fn sizes_are_shown_in_binary_units() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KiB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }
}
