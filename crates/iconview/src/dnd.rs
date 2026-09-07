//! `text/uri-list` encode/decode for drag-and-drop of file selections.

use std::path::PathBuf;

/// Build a `text/uri-list` payload (CRLF-separated, trailing CRLF).
pub fn paths_to_uri_list(paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for p in paths {
        out.push_str("file://");
        out.push_str(&percent_encode_path(&p.to_string_lossy()));
        out.push_str("\r\n");
    }
    out
}

/// Parse a `text/uri-list`: skip `#` comments and non-`file:` URIs, decode `%`.
pub fn uris_to_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        .map(|rest| PathBuf::from(percent_decode(rest)))
        .collect()
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
}
