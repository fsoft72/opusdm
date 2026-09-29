//! Find Files / Search in Files (F-19, F-20): the search itself, kept free of
//! GTK so it can be tested on temporary trees.

use std::path::{Path, PathBuf};

/// Bytes at the start of a file looked at for a NUL, which marks it binary.
const BINARY_SNIFF: usize = 8192;

/// Longest snippet shown for a hit, in characters.
const SNIPPET_MAX: usize = 120;

/// The first hit of a content search in one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextHit {
    /// 1-based line of the first occurrence.
    pub line: usize,
    /// That line, trimmed and cut to [`SNIPPET_MAX`] characters.
    pub snippet: String,
    /// Occurrences in the whole scanned text.
    pub count: usize,
}

/// One result of a search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Full path of the file or directory.
    pub path: PathBuf,
    /// True for a directory (name searches only).
    pub is_dir: bool,
    /// The hit, for content searches.
    pub hit: Option<TextHit>,
}

/// Fold a character for case-insensitive comparison, one to one so positions
/// stay aligned.
fn fold(c: char, case_sensitive: bool) -> char {
    if case_sensitive { c } else { c.to_lowercase().next().unwrap_or(c) }
}

/// Non-overlapping occurrences of `needle` in `hay` (both already folded).
fn count_in(hay: &[char], needle: &[char]) -> usize {
    let mut count = 0;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            count += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    count
}

/// Look for `needle` in `bytes`. A buffer with a NUL in its first
/// [`BINARY_SNIFF`] bytes is binary and gives `None`; so does an empty needle
/// or no occurrence. Invalid UTF-8 is decoded lossily.
pub fn scan_text(bytes: &[u8], needle: &str, case_sensitive: bool) -> Option<TextHit> {
    if needle.is_empty() || bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(bytes);
    let needle: Vec<char> = needle.chars().map(|c| fold(c, case_sensitive)).collect();
    let mut first: Option<(usize, &str)> = None;
    let mut count = 0;
    for (idx, line) in text.lines().enumerate() {
        let hay: Vec<char> = line.chars().map(|c| fold(c, case_sensitive)).collect();
        let n = count_in(&hay, &needle);
        if n == 0 {
            continue;
        }
        count += n;
        if first.is_none() {
            first = Some((idx + 1, line));
        }
    }
    let (line_no, line) = first?;
    let trimmed = line.trim_start();
    let mut snippet: String = trimmed.chars().take(SNIPPET_MAX).collect();
    if trimmed.chars().count() > SNIPPET_MAX {
        snippet.push('\u{2026}');
    }
    Some(TextHit { line: line_no, snippet, count })
}

/// The text of one result row: the path below `root` (a directory with a
/// trailing `/`), and for a content hit `:LINE  snippet` plus the match count
/// when there are several.
pub fn result_label(root: &Path, found: &Found) -> String {
    let rel = found.path.strip_prefix(root).unwrap_or(&found.path).display().to_string();
    if found.is_dir {
        return format!("{rel}/");
    }
    match &found.hit {
        None => rel,
        Some(h) if h.count > 1 => format!("{rel}:{}  {}  ({} matches)", h.line, h.snippet, h.count),
        Some(h) => format!("{rel}:{}  {}", h.line, h.snippet),
    }
}

/// How a search ended up, for the status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Still walking.
    Running,
    /// Ran to the end (or to the result cap).
    Done,
    /// The user stopped it.
    Cancelled,
}

/// `1234567` as `1,234,567`.
pub fn group_thousands(n: usize) -> String {
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

/// The status line of the dialog.
pub fn status_text(state: RunState, scanned: usize, found: usize, skipped: usize, truncated: bool) -> String {
    let (scanned_s, found_s) = (group_thousands(scanned), group_thousands(found));
    if truncated {
        return format!("Stopped at {found_s} results");
    }
    match state {
        RunState::Running => format!("Searching...  {scanned_s} scanned, {found_s} found"),
        RunState::Cancelled => format!("Stopped: {found_s} found ({scanned_s} scanned)"),
        RunState::Done if skipped > 0 => {
            format!("Done: {found_s} found ({scanned_s} scanned, {skipped} unreadable)")
        }
        RunState::Done => format!("Done: {found_s} found ({scanned_s} scanned)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn hit(bytes: &[u8], needle: &str) -> Option<TextHit> {
        scan_text(bytes, needle, false)
    }

    #[test]
    fn the_first_hit_gives_a_one_based_line_and_the_line_text() {
        let h = hit(b"alpha\nbeta\nTODO fix this\ngamma\n", "todo").unwrap();
        assert_eq!(h.line, 3);
        assert_eq!(h.snippet, "TODO fix this");
        assert_eq!(h.count, 1);
    }

    #[test]
    fn matching_can_be_case_sensitive() {
        assert!(scan_text(b"todo\n", "TODO", true).is_none());
        assert!(scan_text(b"TODO\n", "TODO", true).is_some());
        assert!(scan_text(b"todo\n", "TODO", false).is_some());
    }

    #[test]
    fn every_occurrence_in_the_file_is_counted() {
        let h = hit(b"todo todo\nnothing\nTODO\n", "todo").unwrap();
        assert_eq!(h.count, 3);
        assert_eq!(h.line, 1);
    }

    #[test]
    fn crlf_files_keep_their_line_numbers_and_lose_the_carriage_return() {
        let h = hit(b"one\r\ntwo\r\nneedle here\r\n", "needle").unwrap();
        assert_eq!(h.line, 3);
        assert!(!h.snippet.contains('\r'), "{:?}", h.snippet);
    }

    #[test]
    fn the_snippet_is_trimmed_and_a_long_line_is_cut_with_an_ellipsis() {
        assert_eq!(hit(b"      indented match\n", "match").unwrap().snippet, "indented match");
        let long = format!("match {}", "x".repeat(300));
        let s = hit(long.as_bytes(), "match").unwrap().snippet;
        assert_eq!(s.chars().count(), 121, "120 characters and the ellipsis");
        assert!(s.ends_with('\u{2026}'));
    }

    #[test]
    fn a_nul_byte_in_the_head_means_binary_and_no_hit() {
        assert!(hit(b"MZ\x00\x01 match here", "match").is_none());
    }

    #[test]
    fn a_nul_byte_after_the_head_does_not_make_the_file_binary() {
        let mut data = vec![b'a'; 9000];
        data.extend_from_slice(b"\x00 match\n");
        assert!(hit(&data, "match").is_some());
    }

    #[test]
    fn an_empty_needle_or_no_occurrence_is_none() {
        assert!(hit(b"abc", "").is_none());
        assert!(hit(b"abc", "zz").is_none());
        assert!(hit(b"", "a").is_none());
    }

    #[test]
    fn non_ascii_text_is_found_and_invalid_utf8_does_not_fail() {
        assert_eq!(hit("caf\u{e9} \u{c9}t\u{e9}\n".as_bytes(), "\u{e9}t\u{e9}").unwrap().count, 1);
        assert!(hit(b"caf\xe9 match\n", "match").is_some());
    }

    fn found(path: &str, is_dir: bool, hit: Option<TextHit>) -> Found {
        Found { path: PathBuf::from(path), is_dir, hit }
    }

    #[test]
    fn a_name_result_shows_the_path_below_the_root_and_marks_directories() {
        let root = Path::new("/r");
        assert_eq!(result_label(root, &found("/r/a/b.log", false, None)), "a/b.log");
        assert_eq!(result_label(root, &found("/r/a", true, None)), "a/");
    }

    #[test]
    fn a_content_result_shows_line_snippet_and_the_match_count() {
        let root = Path::new("/r");
        let one = TextHit { line: 7, snippet: "TODO x".into(), count: 1 };
        let many = TextHit { line: 2, snippet: "todo".into(), count: 4 };
        assert_eq!(result_label(root, &found("/r/f.rs", false, Some(one))), "f.rs:7  TODO x");
        assert_eq!(result_label(root, &found("/r/f.rs", false, Some(many))), "f.rs:2  todo  (4 matches)");
    }

    #[test]
    fn a_path_outside_the_root_is_shown_whole() {
        assert_eq!(result_label(Path::new("/r"), &found("/elsewhere/x", false, None)), "/elsewhere/x");
    }

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1234), "1,234");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }

    #[test]
    fn the_status_line_follows_the_state_of_the_search() {
        assert_eq!(status_text(RunState::Running, 1234, 12, 0, false), "Searching...  1,234 scanned, 12 found");
        assert_eq!(status_text(RunState::Done, 1234, 12, 0, false), "Done: 12 found (1,234 scanned)");
        assert_eq!(status_text(RunState::Done, 50, 3, 2, false), "Done: 3 found (50 scanned, 2 unreadable)");
        assert_eq!(status_text(RunState::Cancelled, 80, 1, 0, false), "Stopped: 1 found (80 scanned)");
        assert_eq!(status_text(RunState::Done, 9000, 5000, 0, true), "Stopped at 5,000 results");
    }
}
