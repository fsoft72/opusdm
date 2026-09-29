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

use opusdm_ipc::NameFilter;
use std::fs::File;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

/// Most results a search reports before it stops.
pub const MAX_RESULTS: usize = 5000;

/// Most bytes read from one file in a content search.
pub const SEARCH_FILE_MAX: u64 = 16 * 1024 * 1024;

/// A `Scanned` event is sent every this many entries.
const PROGRESS_EVERY: usize = 200;

/// What to look for.
pub struct FindQuery {
    /// Name patterns (`NameFilter` show-list syntax); empty matches everything.
    pub name: NameFilter,
    /// Text to look for inside files; `None` for a name search.
    pub contains: Option<String>,
    /// Match the text with its case.
    pub case_sensitive: bool,
    /// Also look at names starting with `.`.
    pub include_hidden: bool,
    /// The folder to search below.
    pub root: PathBuf,
}

/// What a running search tells the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchEvent {
    /// One result.
    Found(Found),
    /// Entries looked at so far.
    Scanned(usize),
    /// The search ended (finished, cancelled or capped).
    Done { scanned: usize, skipped: usize, truncated: bool },
}

/// The first `cap` bytes of `path`.
fn read_prefix(path: &Path, cap: u64) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    File::open(path)?.take(cap).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Search `query.root`, sending results to `emit` as they are found, and end
/// with a `Done` event. Blocking: run it on a thread. `cancel` is checked
/// before every entry.
pub fn run_search(query: &FindQuery, cancel: &AtomicBool, emit: impl FnMut(SearchEvent)) {
    run_search_capped(query, cancel, MAX_RESULTS, emit);
}

/// [`run_search`] with an explicit result cap (tests use a small one).
fn run_search_capped(query: &FindQuery, cancel: &AtomicBool, max_results: usize, mut emit: impl FnMut(SearchEvent)) {
    let (mut scanned, mut skipped, mut found, mut truncated) = (0usize, 0usize, 0usize, false);
    let mut walker = walkdir::WalkDir::new(&query.root).follow_links(false).min_depth(1).into_iter();
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(entry) = walker.next() else { break };
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().is_dir();
        if !query.include_hidden && name.starts_with('.') {
            if is_dir {
                walker.skip_current_dir();
            }
            continue;
        }
        scanned += 1;
        if scanned.is_multiple_of(PROGRESS_EVERY) {
            emit(SearchEvent::Scanned(scanned));
        }
        if !query.name.allows(&name) {
            continue;
        }
        let path = entry.path().to_path_buf();
        match &query.contains {
            None => {
                emit(SearchEvent::Found(Found { path, is_dir, hit: None }));
                found += 1;
            }
            Some(needle) => {
                if !entry.file_type().is_file() {
                    continue;
                }
                match read_prefix(&path, SEARCH_FILE_MAX) {
                    Ok(bytes) => {
                        if let Some(hit) = scan_text(&bytes, needle, query.case_sensitive) {
                            emit(SearchEvent::Found(Found { path, is_dir: false, hit: Some(hit) }));
                            found += 1;
                        }
                    }
                    Err(_) => skipped += 1,
                }
            }
        }
        if found >= max_results {
            truncated = true;
            break;
        }
    }
    emit(SearchEvent::Done { scanned, skipped, truncated });
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

    use std::sync::atomic::{AtomicBool, Ordering};

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    /// A small tree: two logs, a text file, a binary log, a hidden directory
    /// with a log, and a directory called like a log.
    fn tree() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let r = td.path();
        write(r, "a/one.log", b"alpha\nTODO fix\n");
        write(r, "a/two.txt", b"nothing here\n");
        write(r, "b/three.log", b"todo lower\r\nsecond\r\n");
        write(r, "bin.log", b"\x00\x01 TODO");
        write(r, ".hidden/secret.log", b"TODO hidden\n");
        std::fs::create_dir_all(r.join("logs.d")).unwrap();
        td
    }

    fn query(root: &Path, name: &str, contains: Option<&str>) -> FindQuery {
        FindQuery {
            name: NameFilter::new(name, ""),
            contains: contains.map(String::from),
            case_sensitive: false,
            include_hidden: false,
            root: root.to_path_buf(),
        }
    }

    /// Run a search to the end and return the results as sorted relative paths
    /// plus the final `Done` numbers.
    fn run(q: &FindQuery) -> (Vec<String>, (usize, usize, bool)) {
        run_with(q, &AtomicBool::new(false), MAX_RESULTS)
    }

    fn run_with(q: &FindQuery, cancel: &AtomicBool, cap: usize) -> (Vec<String>, (usize, usize, bool)) {
        let mut names = Vec::new();
        let mut done = None;
        run_search_capped(q, cancel, cap, |ev| match ev {
            SearchEvent::Found(f) => names.push(f.path.strip_prefix(&q.root).unwrap().display().to_string()),
            SearchEvent::Scanned(_) => {}
            SearchEvent::Done { scanned, skipped, truncated } => done = Some((scanned, skipped, truncated)),
        });
        names.sort();
        (names, done.expect("every search ends with Done"))
    }

    #[test]
    fn a_name_search_finds_files_and_directories_by_glob() {
        let td = tree();
        let (names, _) = run(&query(td.path(), "*.log", None));
        assert_eq!(names, vec!["a/one.log", "b/three.log", "bin.log"], "logs.d does not match *.log");
        let (names, _) = run(&query(td.path(), "logs*", None));
        assert_eq!(names, vec!["logs.d"], "a matching directory is a result");
    }

    #[test]
    fn a_name_search_reports_directories_as_directories() {
        let td = tree();
        let mut dirs = 0;
        run_search(&query(td.path(), "logs*", None), &AtomicBool::new(false), |ev| {
            if let SearchEvent::Found(f) = ev {
                assert!(f.is_dir && f.hit.is_none());
                dirs += 1;
            }
        });
        assert_eq!(dirs, 1);
    }

    #[test]
    fn name_exclusions_work() {
        let td = tree();
        let (names, _) = run(&query(td.path(), "*.log,!three*", None));
        assert_eq!(names, vec!["a/one.log", "bin.log"]);
    }

    #[test]
    fn hidden_entries_are_skipped_unless_asked_for() {
        let td = tree();
        let (names, _) = run(&query(td.path(), "*.log", None));
        assert!(!names.iter().any(|n| n.contains(".hidden")), "{names:?}");
        let mut q = query(td.path(), "*.log", None);
        q.include_hidden = true;
        let (names, _) = run(&q);
        assert!(names.contains(&".hidden/secret.log".to_string()), "{names:?}");
    }

    #[test]
    fn the_root_itself_is_never_a_result_even_when_hidden() {
        let td = tempfile::tempdir().unwrap();
        let hidden_root = td.path().join(".cfg");
        write(&hidden_root, "x.txt", b"hello\n");
        let (names, _) = run(&query(&hidden_root, "", None));
        assert_eq!(names, vec!["x.txt"]);
    }

    #[test]
    fn a_content_search_reports_only_files_with_the_text() {
        let td = tree();
        let (names, _) = run(&query(td.path(), "", Some("todo")));
        assert_eq!(names, vec!["a/one.log", "b/three.log"], "binary and hidden files are not reported, nor directories");
    }

    #[test]
    fn a_content_search_applies_the_name_filter() {
        let td = tree();
        let (names, _) = run(&query(td.path(), "*.txt", Some("todo")));
        assert!(names.is_empty());
        let (names, _) = run(&query(td.path(), "one*", Some("todo")));
        assert_eq!(names, vec!["a/one.log"]);
    }

    #[test]
    fn a_content_hit_carries_its_line_and_snippet() {
        let td = tree();
        let mut hits = Vec::new();
        run_search(&query(td.path(), "one*", Some("todo")), &AtomicBool::new(false), |ev| {
            if let SearchEvent::Found(f) = ev {
                hits.push(f.hit.unwrap());
            }
        });
        assert_eq!(hits, vec![TextHit { line: 2, snippet: "TODO fix".into(), count: 1 }]);
    }

    #[test]
    fn a_flag_set_before_the_start_ends_the_search_with_no_results() {
        let td = tree();
        let cancel = AtomicBool::new(true);
        let (names, (scanned, _, truncated)) = run_with(&query(td.path(), "", None), &cancel, MAX_RESULTS);
        assert!(names.is_empty());
        assert_eq!(scanned, 0);
        assert!(!truncated);
    }

    #[test]
    fn cancelling_from_inside_the_event_callback_stops_the_walk() {
        let td = tree();
        let cancel = AtomicBool::new(false);
        let mut found = 0;
        run_search_capped(&query(td.path(), "", None), &cancel, MAX_RESULTS, |ev| {
            if matches!(ev, SearchEvent::Found(_)) {
                found += 1;
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(found, 1, "no result after the flag was raised");
    }

    #[test]
    fn the_result_cap_stops_the_search_and_says_so() {
        let td = tree();
        let (names, (_, _, truncated)) = run_with(&query(td.path(), "*.log", None), &AtomicBool::new(false), 2);
        assert_eq!(names.len(), 2);
        assert!(truncated);
    }

    #[test]
    fn a_symlink_loop_does_not_hang_and_links_are_not_followed() {
        let td = tree();
        std::os::unix::fs::symlink(td.path(), td.path().join("a/loop")).unwrap();
        std::os::unix::fs::symlink(td.path().join("a"), td.path().join("linked_a")).unwrap();
        let (names, _) = run(&query(td.path(), "*.log", None));
        assert_eq!(names, vec!["a/one.log", "b/three.log", "bin.log"], "no copy through linked_a or loop");
        let (names, _) = run(&query(td.path(), "", Some("alpha")));
        assert_eq!(names, vec!["a/one.log"]);
    }

    #[test]
    fn an_unreadable_directory_is_counted_and_does_not_abort_the_search() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads everything, the test would prove nothing
        }
        let td = tree();
        let locked = td.path().join("locked");
        write(td.path(), "locked/inside.log", b"TODO\n");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (names, (_, skipped, _)) = run(&query(td.path(), "*.log", None));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(names.contains(&"a/one.log".to_string()), "the rest is still searched");
        assert!(skipped >= 1, "the locked directory is counted");
    }

    #[test]
    fn done_reports_how_many_entries_were_looked_at() {
        let td = tree();
        let (_, (scanned, skipped, truncated)) = run(&query(td.path(), "", None));
        // a, a/one.log, a/two.txt, b, b/three.log, bin.log, logs.d (hidden skipped)
        assert_eq!(scanned, 7);
        assert_eq!(skipped, 0);
        assert!(!truncated);
    }
}
