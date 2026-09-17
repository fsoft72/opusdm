//! Single `getpwuid_r` wrapper shared by `model.rs` (owner name for the
//! listing) and `terminal.rs` (login shell for the terminal drawer), so the
//! unsafe libc buffer dance exists exactly once.

use std::ffi::CStr;

/// The passwd fields this crate needs. Extend here, not with a second
/// `getpwuid_r` call site, if another field is needed later.
pub struct PasswdEntry {
    pub name: String,
    pub shell: String,
}

/// Look up `uid` in the user database. `None` on any failure (unknown uid,
/// non-UTF8 fields, or the call itself failing).
#[cfg(unix)]
pub fn lookup(uid: u32) -> Option<PasswdEntry> {
    let mut buf = vec![0u8; 16384];
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut ptr = std::ptr::null_mut();
    // SAFETY: `buf` outlives the call and is large enough for glibc's NSS
    // backends; `passwd`/`ptr` are valid out-params the call fills or leaves
    // untouched on error.
    let ret = unsafe {
        libc::getpwuid_r(
            uid as libc::uid_t,
            &mut passwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut ptr,
        )
    };
    if ret != 0 || ptr.is_null() {
        return None;
    }
    // SAFETY: a non-null `ptr` from a successful call points at `passwd`,
    // whose `pw_name`/`pw_shell` are NUL-terminated C strings valid until
    // `buf` is dropped.
    let name = unsafe { CStr::from_ptr((*ptr).pw_name) }.to_str().ok()?.to_string();
    let shell = unsafe { CStr::from_ptr((*ptr).pw_shell) }.to_str().ok()?.to_string();
    Some(PasswdEntry { name, shell })
}

#[cfg(not(unix))]
pub fn lookup(_uid: u32) -> Option<PasswdEntry> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_current_uid_succeeds() {
        let uid = unsafe { libc::getuid() };
        let entry = lookup(uid).expect("current uid should resolve");
        assert!(!entry.name.is_empty());
        assert!(!entry.shell.is_empty());
    }

    #[test]
    fn lookup_unknown_uid_returns_none() {
        // uid_t is 32-bit; this one is vanishingly unlikely to exist.
        assert!(lookup(u32::MAX - 1).is_none());
    }
}
