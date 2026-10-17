//! `text/uri-list` encode/decode for drag-and-drop of file selections.

use gtk4 as gtk;
use gtk::prelude::*;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// Live Shift-key state, queried directly from the seat rather than trusted
/// off the drop event's modifier bits. During a cross-process drag the
/// source process holds the pointer grab, so the target's own synthesized
/// drop event can carry a stale/empty modifier mask - a direct seat query is
/// unaffected by which process owns the grab.
pub fn shift_held(widget: &impl IsA<gtk::Widget>) -> bool {
    let seat = widget.display().default_seat();
    let pointer_state = seat
        .as_ref()
        .and_then(|s| s.pointer())
        .map(|d| d.modifier_state());
    let keyboard_state = seat
        .as_ref()
        .and_then(|s| s.keyboard())
        .map(|d| d.modifier_state());
    crate::dbg_log!("shift_held: pointer_state={pointer_state:?} keyboard_state={keyboard_state:?}");
    let has_shift = |m: gtk::gdk::ModifierType| m.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    pointer_state.is_some_and(has_shift) || keyboard_state.is_some_and(has_shift)
}

/// Build a `text/uri-list` payload (CRLF-separated, trailing CRLF).
pub fn paths_to_uri_list(paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for p in paths {
        out.push_str(&file_uri(p));
        out.push_str("\r\n");
    }
    out
}

/// The canonical `file://` URI for `path`: each raw byte percent-encoded
/// except the RFC 3986 unreserved set, with `/` kept. Works on the raw bytes,
/// so a non-UTF-8 name survives (and matches the freedesktop thumbnail key).
pub fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Parse a `text/uri-list`: skip `#` comments and non-`file:` URIs, decode `%`.
///
/// This is the one decoder for every `file://` list (drag-and-drop and the
/// clipboard of the Lister and the desktop). An optional `localhost` (or this
/// machine's) host is stripped; URIs naming another host are skipped, since
/// their path does not exist here. Decoded bytes are kept as they are, so a
/// non-UTF-8 name (`%E9`) maps back to the same file.
pub fn uris_to_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        .filter_map(local_path_part)
        .map(|path| PathBuf::from(OsString::from_vec(percent_decode(path.as_bytes()))))
        .collect()
}

/// The path part of a `file://` URI body (`host/path` or `/path`), or `None`
/// when the host is not this machine.
fn local_path_part(rest: &str) -> Option<&str> {
    if rest.starts_with('/') {
        return Some(rest);
    }
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    let local = host.eq_ignore_ascii_case("localhost") || host == gtk::glib::host_name().as_str();
    local.then_some(&rest[slash..])
}

/// Decode `%XX` escapes into raw bytes. A `%` not followed by two hex digits
/// is kept literally. Works on bytes only, so it never splits a char.
fn percent_decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escape = (bytes[i] == b'%').then(|| bytes.get(i + 1..i + 3)).flatten();
        if let Some(&[hi, lo]) = escape {
            if let (Some(hi), Some(lo)) = (hex_value(hi), hex_value(lo)) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// The value of one ASCII hex digit.
fn hex_value(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|v| v as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_simple_paths() {
        let paths = vec![PathBuf::from("/home/x/a.txt"), PathBuf::from("/tmp/b c.txt")];
        let list = paths_to_uri_list(&paths);
        assert!(list.contains("file:///home/x/a.txt"));
        assert!(list.contains("file:///tmp/b%20c.txt"));
        assert!(list.ends_with("\r\n"));
        assert_eq!(uris_to_paths(&list), paths);
    }

    #[test]
    fn ignores_comments_and_non_file_uris() {
        let text = "# comment\r\nhttp://example.com/x\r\nfile:///ok\r\n";
        assert_eq!(uris_to_paths(text), vec![PathBuf::from("/ok")]);
    }

    #[test]
    fn broken_escape_before_a_multibyte_char_does_not_panic() {
        // `%1` followed by the two-byte `é`: slicing the &str by byte index
        // used to cut into the char and panic.
        assert_eq!(uris_to_paths("file:///tmp/a%1é\r\n"), vec![PathBuf::from("/tmp/a%1é")]);
        assert_eq!(uris_to_paths("file:///tmp/%é\r\n"), vec![PathBuf::from("/tmp/%é")]);
        assert_eq!(uris_to_paths("file:///tmp/x%\r\n"), vec![PathBuf::from("/tmp/x%")]);
    }

    #[test]
    fn non_utf8_bytes_are_kept_as_raw_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let got = uris_to_paths("file:///tmp/caf%E9.txt\r\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].as_os_str().as_bytes(), b"/tmp/caf\xe9.txt");
    }

    #[test]
    fn non_utf8_names_round_trip_through_the_uri_list() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let p = PathBuf::from(OsStr::from_bytes(b"/tmp/caf\xe9 #1.txt"));
        assert_eq!(file_uri(&p), "file:///tmp/caf%E9%20%231.txt");
        assert_eq!(uris_to_paths(&paths_to_uri_list(std::slice::from_ref(&p))), vec![p]);
    }

    #[test]
    fn localhost_host_is_stripped_and_remote_hosts_are_skipped() {
        assert_eq!(uris_to_paths("file://localhost/etc/hosts\r\n"), vec![PathBuf::from("/etc/hosts")]);
        assert!(uris_to_paths("file://otherbox.example/etc/hosts\r\n").is_empty());
    }
}
