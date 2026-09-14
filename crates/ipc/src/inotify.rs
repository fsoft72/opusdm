//! Shared low-level inotify(7) plumbing: fd lifecycle, poll-based readiness
//! and raw event parsing. Used by both `lister`'s directory watcher and
//! `hub`'s desktop watcher so a fix to fd handling or event parsing lands
//! once instead of twice.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// The fixed part of `struct inotify_event`, before the variable-length name.
pub const EVENT_HEADER: usize = std::mem::size_of::<libc::inotify_event>();

/// Owns one inotify file descriptor; closes it on drop.
pub struct InotifyFd(i32);

impl InotifyFd {
    /// Open a new inotify instance with the given `inotify_init1` flags
    /// (e.g. `libc::IN_CLOEXEC`, optionally `| libc::IN_NONBLOCK`).
    pub fn new(flags: libc::c_int) -> io::Result<Self> {
        // SAFETY: flags is a valid combination of IN_* constants; the fd is
        // checked below.
        let fd = unsafe { libc::inotify_init1(flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(fd))
    }

    /// The raw fd, for `poll`/`read`.
    pub fn raw(&self) -> i32 {
        self.0
    }

    /// Add a watch on `path` with `mask`. Returns the watch descriptor.
    pub fn add_watch(&self, path: &Path, mask: u32) -> io::Result<i32> {
        let cpath = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path has an interior NUL"))?;
        // SAFETY: cpath is a valid NUL-terminated string for the call's duration.
        let wd = unsafe { libc::inotify_add_watch(self.0, cpath.as_ptr(), mask) };
        if wd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(wd)
    }

    /// Remove a previously added watch. Harmless if already gone.
    pub fn rm_watch(&self, wd: i32) {
        // SAFETY: wd came from add_watch on this fd, or is already invalid,
        // which the kernel reports harmlessly.
        unsafe { libc::inotify_rm_watch(self.0, wd) };
    }
}

impl Drop for InotifyFd {
    fn drop(&mut self) {
        // SAFETY: fd is ours and not used again.
        unsafe { libc::close(self.0) };
    }
}

/// What a bounded wait on a fd observed.
pub enum Wait {
    /// At least one event is readable.
    Ready,
    /// Nothing happened before the timeout.
    Timeout,
    /// The fd went bad; the caller should stop.
    Failed,
}

/// Block until `fd` is readable or `timeout_ms` elapses.
pub fn wait_readable(fd: i32, timeout_ms: i32) -> Wait {
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: one initialised pollfd is passed with a matching count.
    let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if ret < 0 {
        let err = io::Error::last_os_error();
        return if err.kind() == io::ErrorKind::Interrupted { Wait::Timeout } else { Wait::Failed };
    }
    if ret == 0 {
        return Wait::Timeout;
    }
    if pfd.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
        return Wait::Failed;
    }
    Wait::Ready
}

/// Read one batch of raw bytes into `buf`. `None` means the fd failed;
/// `Some(0)` means nothing was available this time (EAGAIN/EINTR/EOF).
pub fn read_raw(fd: i32, buf: &mut [u8]) -> Option<usize> {
    // SAFETY: buf is a valid writable slice of the length passed.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n < 0 {
        let err = io::Error::last_os_error();
        return match err.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Some(0),
            _ => None,
        };
    }
    Some(n as usize)
}

/// Pull the watch descriptors out of a raw inotify read buffer.
///
/// Events are variable length: a header followed by `len` bytes of name. A
/// short or malformed tail is ignored rather than panicking - the kernel
/// never produces one, but the parse must not run off the end of the buffer.
pub fn parse_wds(buf: &[u8]) -> Vec<i32> {
    let mut wds = Vec::new();
    let mut off = 0usize;
    while off + EVENT_HEADER <= buf.len() {
        // SAFETY: the slice has at least EVENT_HEADER bytes left, and the read
        // is unaligned so the buffer needs no particular alignment.
        let ev: libc::inotify_event =
            unsafe { std::ptr::read_unaligned(buf[off..].as_ptr() as *const libc::inotify_event) };
        let Some(total) = EVENT_HEADER.checked_add(ev.len as usize) else { break };
        if off + total > buf.len() {
            break;
        }
        wds.push(ev.wd);
        off += total;
    }
    wds
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a raw inotify buffer the way the kernel would.
    fn encode(events: &[(i32, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        for (wd, name) in events {
            // A name is NUL-terminated and padded to a multiple of 16.
            let len = if name.is_empty() { 0 } else { name.len().div_ceil(16) * 16 + 16 };
            let ev = libc::inotify_event { wd: *wd, mask: 0, cookie: 0, len: len as u32 };
            let bytes: [u8; EVENT_HEADER] = unsafe { std::mem::transmute(ev) };
            buf.extend_from_slice(&bytes);
            buf.extend(std::iter::repeat_n(0u8, len));
        }
        buf
    }

    #[test]
    fn parses_a_batch_of_variable_length_events() {
        let buf = encode(&[(1, ""), (2, "a.txt"), (7, "a-much-longer-file-name.bin")]);
        assert_eq!(parse_wds(&buf), vec![1, 2, 7]);
    }

    #[test]
    fn a_truncated_tail_is_ignored_not_panicked_on() {
        let mut buf = encode(&[(3, "x"), (4, "y")]);
        buf.truncate(buf.len() - 3);
        // The first event is complete, the second is not.
        assert_eq!(parse_wds(&buf), vec![3]);
        assert_eq!(parse_wds(&[]), Vec::<i32>::new());
        assert_eq!(parse_wds(&[0u8; 4]), Vec::<i32>::new());
    }

    #[test]
    fn reports_a_created_file_via_a_real_fd() {
        let td = tempfile::tempdir().unwrap();
        let inotify = InotifyFd::new(libc::IN_CLOEXEC).unwrap();
        inotify.add_watch(td.path(), libc::IN_CREATE).unwrap();

        std::fs::write(td.path().join("new.txt"), b"hi").unwrap();

        assert!(matches!(wait_readable(inotify.raw(), 5_000), Wait::Ready));
        let mut buf = vec![0u8; 4096];
        let n = read_raw(inotify.raw(), &mut buf).unwrap();
        assert!(n > 0);
        assert!(!parse_wds(&buf[..n]).is_empty());
    }
}
