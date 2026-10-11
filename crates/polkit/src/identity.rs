//! The users who may authenticate a request. polkitd offers identities
//! (`unix-user` with a uid, `unix-group` with a gid, the administrators of
//! the system); the dialog needs user names, the current user first.

use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use zbus::zvariant::OwnedValue;

/// `a(sa{sv})`: the identities of `BeginAuthentication`.
pub type Identities = Vec<(String, HashMap<String, OwnedValue>)>;

/// Largest buffer offered to the `get*_r` calls before giving up.
const MAX_BUFFER: usize = 1 << 20;

/// One identity polkitd accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    User(u32),
    Group(u32),
}

/// What the dialog needs from the account database.
pub trait Accounts {
    /// The name of user `uid`.
    fn user_name(&self, uid: u32) -> Option<String>;
    /// The names of the members of group `gid`.
    fn group_members(&self, gid: u32) -> Vec<String>;
}

/// The identities polkitd sent, the unknown kinds left out.
pub fn parse(identities: &Identities) -> Vec<Identity> {
    identities
        .iter()
        .filter_map(|(kind, details)| match kind.as_str() {
            "unix-user" => id_of(details, "uid").map(Identity::User),
            "unix-group" => id_of(details, "gid").map(Identity::Group),
            _ => None,
        })
        .collect()
}

/// The number under `key`.
fn id_of(details: &HashMap<String, OwnedValue>, key: &str) -> Option<u32> {
    u32::try_from(details.get(key)?.clone()).ok()
}

/// The user names of `identities` (groups expanded to their members), each
/// once, in order, with `current` moved first when it is one of them.
pub fn user_names(identities: &[Identity], accounts: &dyn Accounts, current: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for identity in identities {
        let found = match *identity {
            Identity::User(uid) => accounts.user_name(uid).into_iter().collect(),
            Identity::Group(gid) => accounts.group_members(gid),
        };
        for name in found {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if let Some(at) = current.and_then(|current| names.iter().position(|name| name == current)) {
        let name = names.remove(at);
        names.insert(0, name);
    }
    names
}

/// The system account database (`/etc/passwd`, `/etc/group` or whatever
/// NSS uses).
pub struct SystemAccounts;

impl Accounts for SystemAccounts {
    fn user_name(&self, uid: u32) -> Option<String> {
        with_buffer(|buf| {
            // SAFETY: `passwd` is plain data; getpwuid_r fills it with
            // pointers into `buf`, which outlives the read below.
            let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
            let mut result = std::ptr::null_mut();
            let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut result) };
            if rc != 0 || result.is_null() { return (rc, None); }
            // SAFETY: pw_name is a NUL terminated string inside `buf`.
            (0, Some(unsafe { CStr::from_ptr(pwd.pw_name) }.to_string_lossy().into_owned()))
        })
        .flatten()
    }

    fn group_members(&self, gid: u32) -> Vec<String> {
        with_buffer(|buf| {
            // SAFETY: as above, with getgrgid_r and `group`.
            let mut group: libc::group = unsafe { std::mem::zeroed() };
            let mut result = std::ptr::null_mut();
            let rc = unsafe { libc::getgrgid_r(gid, &mut group, buf.as_mut_ptr().cast(), buf.len(), &mut result) };
            if rc != 0 || result.is_null() { return (rc, None); }
            // SAFETY: gr_mem is a NULL terminated array of NUL terminated
            // strings inside `buf`.
            (0, Some(unsafe { c_string_list(group.gr_mem) }))
        })
        .flatten()
        .unwrap_or_default()
    }
}

/// Run a `get*_r` lookup with a buffer that grows while the call answers
/// `ERANGE`. `lookup` returns the call's error code and its value.
fn with_buffer<T>(mut lookup: impl FnMut(&mut [u8]) -> (i32, Option<T>)) -> Option<Option<T>> {
    let mut buf = vec![0u8; 1024];
    loop {
        match lookup(&mut buf) {
            (libc::ERANGE, _) if buf.len() < MAX_BUFFER => buf.resize(buf.len() * 2, 0),
            (0, value) => return Some(value),
            _ => return None,
        }
    }
}

/// The strings of a NULL terminated C string array.
///
/// # Safety
/// `list` must be null or point to a NULL terminated array of valid C strings.
unsafe fn c_string_list(list: *mut *mut c_char) -> Vec<String> {
    let mut names = Vec::new();
    if list.is_null() { return names; }
    let mut at = list;
    while !(*at).is_null() {
        names.push(CStr::from_ptr(*at).to_string_lossy().into_owned());
        at = at.add(1);
    }
    names
}

/// The name of the user running this program.
pub fn current_user() -> Option<String> {
    // SAFETY: getuid cannot fail.
    SystemAccounts.user_name(unsafe { libc::getuid() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    struct FakeAccounts;

    impl Accounts for FakeAccounts {
        fn user_name(&self, uid: u32) -> Option<String> {
            match uid {
                0 => Some("root".to_string()),
                1000 => Some("alice".to_string()),
                _ => None,
            }
        }

        fn group_members(&self, gid: u32) -> Vec<String> {
            match gid {
                27 => vec!["bob".to_string(), "alice".to_string()],
                _ => Vec::new(),
            }
        }
    }

    fn identity(kind: &str, key: &str, id: u32) -> (String, HashMap<String, OwnedValue>) {
        (kind.to_string(), HashMap::from([(key.to_string(), OwnedValue::try_from(Value::from(id)).unwrap())]))
    }

    #[test]
    fn users_and_groups_are_parsed_and_other_kinds_skipped() {
        let identities = vec![identity("unix-user", "uid", 1000), identity("unix-group", "gid", 27), identity("unix-netgroup", "name", 1)];
        assert_eq!(parse(&identities), [Identity::User(1000), Identity::Group(27)]);
    }

    #[test]
    fn groups_expand_to_their_members_each_named_once() {
        let names = user_names(&[Identity::Group(27), Identity::User(1000), Identity::User(0)], &FakeAccounts, None);
        assert_eq!(names, ["bob", "alice", "root"]);
    }

    #[test]
    fn the_current_user_comes_first() {
        let names = user_names(&[Identity::User(0), Identity::Group(27)], &FakeAccounts, Some("alice"));
        assert_eq!(names, ["alice", "root", "bob"]);
    }

    #[test]
    fn unknown_ids_give_no_names() {
        assert!(user_names(&[Identity::User(4242), Identity::Group(4242)], &FakeAccounts, Some("alice")).is_empty());
    }

    #[test]
    fn root_has_a_name_in_the_system_database() {
        assert_eq!(SystemAccounts.user_name(0).as_deref(), Some("root"));
        assert!(current_user().is_some());
    }
}
