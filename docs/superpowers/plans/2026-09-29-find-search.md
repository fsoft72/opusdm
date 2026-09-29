# Find Files and Search in Files Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One "Find" dialog that searches a folder tree by name (F-19) and by file content (F-20), streaming results while it runs, with jumps to the folder in the Lister or to the file in the Viewer.

**Architecture:** A pure module `find_search.rs` holds the query, the text scan, the walker and the label / status helpers, all tested on temporary trees. `find_dialog.rs` is a thin GTK dialog: a worker thread runs the search and streams `SearchEvent`s over an `async_channel` to a future on the main loop. The Viewer (block B) opens content hits through `ViewerOptions::initial_search`.

**Tech Stack:** Rust workspace, GTK4, `walkdir` (already a dependency of `opusdm-lister`), `async-channel`.

**Spec:** `docs/superpowers/specs/2026-09-29-find-search-design.md`

## Global Constraints

- Code and comments in English. Never use the long dash character; use `-`.
- Constants in UPPERCASE with `_`. Every function and method gets a doc comment (`///` for public ones).
- Exit conditions at the top of a function, one-liners where possible.
- Text files end with an empty line.
- Commit after each task with a meaningful message, only the files you changed. Do NOT push. Do NOT add AI attribution lines. Never use `--no-verify`. Do not disable tests.
- Limits are constants: `MAX_RESULTS` = 5000, `SEARCH_FILE_MAX` = 16 MiB per file (making them settings is in `NEXT-PHASE.md`, not here).
- The search runs in a thread of the Lister, never as a hub job; no protocol change.
- Symbolic links are never followed; names starting with `.` are skipped (with their subtree) unless "Include hidden" is on; the search root itself is never a result.
- Starting a new search or closing the dialog cancels the running one; late events of a cancelled search never reach the screen.

## Review Focus

1. Cancellation: a flag set before the start or during the walk ends the search with a `Done` and no results after the cancel (Task 2 tests).
2. An unreadable directory is counted as skipped and does not abort the search (Task 2 test, skipped when running as root).
3. Symbolic links are not followed: a link loop must not hang, a link to a directory is not descended into (Task 2 tests).
4. A binary file (NUL in its first 8 KiB) is never reported by a content search; CRLF files give the right line number and no `\r` in the snippet (Task 1 tests).
5. A late event of a cancelled search must not appear in a newer search's list, and the closed dialog must not stay alive because of its own handlers (Task 3, generation counter and weak references, reviewed by reading).

---

## File Structure

| File | Responsibility |
| :--- | :--- |
| `crates/lister/src/find_search.rs` (new) | `FindQuery`, `TextHit`, `Found`, `SearchEvent`, `scan_text`, `run_search`, `result_label`, `status_text` |
| `crates/lister/src/find_dialog.rs` (new) | the GTK dialog, `Ui::show_in_folder` |
| `crates/lister/src/advanced_select.rs` | `add_field` becomes `pub(crate)` for reuse |
| `crates/lister/src/keys.rs`, `palette.rs`, `main.rs` | commands and module registration |

---

### Task 1: Text scan, labels and status text (`find_search.rs`, part 1)

**Files:**
- Create: `crates/lister/src/find_search.rs`
- Modify: `crates/lister/src/main.rs` (`mod find_search;`)

**Interfaces:**
- Produces: `TextHit { line: usize, snippet: String, count: usize }`, `Found { path: PathBuf, is_dir: bool, hit: Option<TextHit> }`, `scan_text(bytes: &[u8], needle: &str, case_sensitive: bool) -> Option<TextHit>`, `result_label(root: &Path, found: &Found) -> String`, `enum RunState { Running, Done, Cancelled }`, `status_text(state: RunState, scanned: usize, found: usize, skipped: usize, truncated: bool) -> String`, `group_thousands(n: usize) -> String`.

- [ ] **Step 1: Write the tests first**

Create `crates/lister/src/find_search.rs` with the header and the test module only:

```rust
//! Find Files / Search in Files (F-19, F-20): the search itself, kept free of
//! GTK so it can be tested on temporary trees.

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
```

Add `mod find_search;` to `main.rs` (alphabetical, near `mod filter;`). Run `cargo test -p opusdm-lister find_search 2>&1 | grep -E "^error" | sort | uniq -c | head -4`.
Expected: compile errors (`scan_text`, `Found`, ... not found).

- [ ] **Step 2: Implement**

Above the tests in `find_search.rs`:

```rust
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
```

- [ ] **Step 3: Run**

Run: `cargo test -p opusdm-lister find_search`
Expected: 15 tests PASS. Mutation check: in `count_in` change `i += needle.len();` (after a match) to `i += 1;`; a test with a repeated needle such as `aa`... use the existing `every_occurrence_in_the_file_is_counted` only if it overlaps, so instead mutate `scan_text` `bytes.len().min(BINARY_SNIFF)` to `bytes.len()`: `a_nul_byte_after_the_head_does_not_make_the_file_binary` must FAIL; revert.

- [ ] **Step 4: Commit**

```bash
git add crates/lister/src/find_search.rs crates/lister/src/main.rs
git commit -m "feat(lister): text scan, result labels and status text for Find"
```

---

### Task 2: The walker (`run_search`)

**Files:**
- Modify: `crates/lister/src/find_search.rs`

**Interfaces:**
- Consumes: `NameFilter` (`opusdm_ipc`), `scan_text`, `Found`, `TextHit` (Task 1).
- Produces: `FindQuery { name: NameFilter, contains: Option<String>, case_sensitive: bool, include_hidden: bool, root: PathBuf }`, `enum SearchEvent { Found(Found), Scanned(usize), Done { scanned: usize, skipped: usize, truncated: bool } }`, `MAX_RESULTS`, `SEARCH_FILE_MAX`, `run_search(query: &FindQuery, cancel: &AtomicBool, emit: impl FnMut(SearchEvent))`.

- [ ] **Step 1: Write the tests first**

Append inside the `tests` module of `find_search.rs`:

```rust
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
        assert_eq!(names, vec!["a/one.log", "b/three.log", "bin.log", "logs.d"].iter().filter(|n| **n != "logs.d").map(|s| s.to_string()).collect::<Vec<_>>());
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
```

Run `cargo test -p opusdm-lister find_search 2>&1 | grep -E "^error" | sort | uniq -c | head -4` -> Expected: compile errors (`FindQuery`, `run_search`, ... missing).

- [ ] **Step 2: Implement**

Add near the top of `find_search.rs` (after the existing `use std::path::{Path, PathBuf};`):

```rust
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
        if scanned % PROGRESS_EVERY == 0 {
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
```

- [ ] **Step 3: Run**

Run: `cargo test -p opusdm-lister find_search`
Expected: all PASS (about 29). If `a_name_search_finds_files_and_directories_by_glob` looks odd, note its first assertion filters `logs.d` out of the expected list on purpose: `*.log` does not match `logs.d`. If `done_reports_how_many_entries_were_looked_at` disagrees, count the visible entries of `tree()` again (7) rather than changing the walker.
Mutation checks: (a) remove the `walker.skip_current_dir();` line: `hidden_entries_are_skipped_unless_asked_for` must still pass but scanning `.hidden/secret.log` would be counted, so `done_reports_how_many_entries_were_looked_at` must FAIL (scanned 8 or the hidden log fails the name test); revert. (b) change `follow_links(false)` to `true`: `a_symlink_loop_does_not_hang...` must FAIL or error; revert.

- [ ] **Step 4: Commit**

```bash
git add crates/lister/src/find_search.rs
git commit -m "feat(lister): the Find/Search walker with cancel, cap and skipped counts"
```

---

### Task 3: The dialog, Show in folder and the commands

**Files:**
- Create: `crates/lister/src/find_dialog.rs`
- Modify: `crates/lister/src/advanced_select.rs` (`add_field` -> `pub(crate)`), `crates/lister/src/main.rs`, `crates/lister/src/keys.rs`, `crates/lister/src/palette.rs`

**Interfaces:**
- Consumes: Tasks 1-2; `viewer::open`, `ViewerOptions`; `Ui::navigate`, `Ui::jump_to_row`, `Ui::row_at`; `opusdm_ui::DialogShell`.
- Produces: `find_dialog::open(ui: &Rc<Ui>, focus_contains: bool)`, `Ui::show_in_folder(&Rc<Self>, &Path)`, `CommandId::{FindFiles, SearchInFiles}`.

GTK code: the logic is tested in Tasks 1-2; verification here is the build plus the manual check.

- [ ] **Step 1: The dialog**

In `advanced_select.rs` change `fn add_field(` to `pub(crate) fn add_field(`.

Create `crates/lister/src/find_dialog.rs`:

```rust
//! The "Find" dialog (F-19, F-20): name and content search with streaming
//! results. The search runs on a thread; this file only shows what it sends.

use crate::find_search::{result_label, run_search, status_text, FindQuery, Found, RunState, SearchEvent};
use crate::viewer::ViewerOptions;
use crate::viewer_mode::Mode;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::NameFilter;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// One open Find dialog.
struct Dialog {
    ui: Rc<Ui>,
    window: gtk::Window,
    name: gtk::Entry,
    contains: gtk::Entry,
    folder: gtk::Entry,
    case_sensitive: gtk::CheckButton,
    include_hidden: gtk::CheckButton,
    list: gtk::ListBox,
    status: gtk::Label,
    /// Results of the current search, in the order of the list rows.
    results: RefCell<Vec<Found>>,
    /// The folder the current search started from.
    root: RefCell<PathBuf>,
    /// The Contains text of the current search, if it is a content search.
    searched_text: RefCell<Option<String>>,
    /// Cancel flag of the running (or last) search.
    cancel: RefCell<Arc<AtomicBool>>,
    /// Bumped by every search; events of an older one are ignored.
    generation: Cell<u64>,
    scanned: Cell<usize>,
}

/// Open the Find dialog. `focus_contains` puts the cursor on the Contains
/// field ("Search in Files...") instead of Name ("Find Files...").
pub fn open(ui: &Rc<Ui>, focus_contains: bool) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Find").size(720, 560);
    let content = shell.content().clone();
    let name = crate::advanced_select::add_field(&content, "Name", "*.log, *.rs, !test*");
    let contains = crate::advanced_select::add_field(&content, "Contains", "text to look for inside files");
    let folder = crate::advanced_select::add_field(&content, "Folder", "");
    folder.set_text(&ui.cwd_path().to_string_lossy());
    let case_sensitive = gtk::CheckButton::with_label("Case sensitive");
    let include_hidden = gtk::CheckButton::with_label("Include hidden files and folders");
    content.append(&case_sensitive);
    content.append(&include_hidden);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    content.append(&status);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
    content.append(&scroller);

    let search_btn = shell.button("Search");
    let stop_btn = shell.button("Stop");
    let show_btn = shell.button("Show in folder");
    let view_btn = shell.button("View");
    shell.close_button("Close");
    let window = shell.window().clone();

    let dialog = Rc::new(Dialog {
        ui: ui.clone(),
        window: window.clone(),
        name: name.clone(),
        contains: contains.clone(),
        folder: folder.clone(),
        case_sensitive,
        include_hidden,
        list: list.clone(),
        status,
        results: RefCell::new(Vec::new()),
        root: RefCell::new(PathBuf::new()),
        searched_text: RefCell::new(None),
        cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
        generation: Cell::new(0),
        scanned: Cell::new(0),
    });

    // Handlers on widgets the Dialog also owns hold only a weak reference, so
    // the Dialog can be freed once its window is gone.
    let weak = |dialog: &Rc<Dialog>| Rc::downgrade(dialog);
    for entry in [&name, &contains, &folder] {
        let d = weak(&dialog);
        entry.connect_activate(move |_| {
            if let Some(d) = d.upgrade() {
                d.start();
            }
        });
    }
    {
        let d = weak(&dialog);
        search_btn.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.start();
            }
        });
    }
    {
        let d = weak(&dialog);
        stop_btn.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.stop();
            }
        });
    }
    {
        let d = weak(&dialog);
        show_btn.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.show_selected();
            }
        });
    }
    {
        let d = weak(&dialog);
        view_btn.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.view_selected();
            }
        });
    }
    {
        let d = weak(&dialog);
        list.connect_row_activated(move |_, row| {
            let Some(d) = d.upgrade() else { return };
            if d.searched_text.borrow().is_some() {
                d.view_index(row.index() as usize);
            } else {
                d.show_index(row.index() as usize);
            }
        });
    }
    {
        let d = weak(&dialog);
        window.connect_close_request(move |_| {
            if let Some(d) = d.upgrade() {
                d.stop();
            }
            glib::Propagation::Proceed
        });
    }
    shell.present();
    if focus_contains {
        contains.grab_focus();
    } else {
        name.grab_focus();
    }
}

impl Dialog {
    /// Stop the running search, if any.
    fn stop(&self) {
        self.cancel.borrow().store(true, Ordering::Relaxed);
    }

    /// Start a search from the fields, replacing the one that is running.
    fn start(self: &Rc<Self>) {
        self.stop();
        let root = PathBuf::from(self.folder.text().trim());
        if !root.is_dir() {
            self.status.set_text(&format!("Folder not found: {}", root.display()));
            return;
        }
        let contains = self.contains.text().to_string();
        let contains = (!contains.is_empty()).then_some(contains);
        let query = FindQuery {
            name: NameFilter::new(self.name.text().as_str(), ""),
            contains: contains.clone(),
            case_sensitive: self.case_sensitive.is_active(),
            include_hidden: self.include_hidden.is_active(),
            root: root.clone(),
        };

        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        self.results.borrow_mut().clear();
        *self.root.borrow_mut() = root;
        *self.searched_text.borrow_mut() = contains;
        self.scanned.set(0);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.status.set_text(&status_text(RunState::Running, 0, 0, 0, false));

        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.borrow_mut() = cancel.clone();
        let (tx, rx) = async_channel::unbounded::<SearchEvent>();
        let thread_cancel = cancel.clone();
        std::thread::spawn(move || {
            run_search(&query, &thread_cancel, |event| {
                let _ = tx.send_blocking(event);
            });
        });

        let this = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                if this.generation.get() != generation {
                    break;
                }
                this.handle(event, &cancel);
            }
        });
    }

    /// Show one event of the running search.
    fn handle(&self, event: SearchEvent, cancel: &AtomicBool) {
        match event {
            SearchEvent::Found(found) => {
                let label = gtk::Label::new(Some(&result_label(&self.root.borrow(), &found)));
                label.set_xalign(0.0);
                label.set_selectable(false);
                self.list.append(&label);
                self.results.borrow_mut().push(found);
                self.status.set_text(&status_text(RunState::Running, self.scanned.get(), self.results.borrow().len(), 0, false));
            }
            SearchEvent::Scanned(n) => {
                self.scanned.set(n);
                self.status.set_text(&status_text(RunState::Running, n, self.results.borrow().len(), 0, false));
            }
            SearchEvent::Done { scanned, skipped, truncated } => {
                let state = if cancel.load(Ordering::Relaxed) { RunState::Cancelled } else { RunState::Done };
                self.status.set_text(&status_text(state, scanned, self.results.borrow().len(), skipped, truncated));
            }
        }
    }

    /// The result behind the selected row.
    fn selected(&self) -> Option<usize> {
        self.list.selected_row().map(|r| r.index() as usize)
    }

    fn show_selected(&self) {
        match self.selected() {
            Some(i) => self.show_index(i),
            None => self.status.set_text("Select a result first"),
        }
    }

    fn view_selected(&self) {
        match self.selected() {
            Some(i) => self.view_index(i),
            None => self.status.set_text("Select a result first"),
        }
    }

    /// Navigate the Lister to the folder holding result `index` and select it.
    fn show_index(&self, index: usize) {
        let Some(found) = self.results.borrow().get(index).cloned() else { return };
        self.ui.show_in_folder(&found.path);
    }

    /// Open result `index` in the Viewer, on the first match of the text.
    fn view_index(&self, index: usize) {
        let Some(found) = self.results.borrow().get(index).cloned() else { return };
        if found.is_dir {
            self.status.set_text("A folder cannot be viewed; use Show in folder");
            return;
        }
        let opts = ViewerOptions { initial_search: self.searched_text.borrow().clone() };
        crate::viewer::open(&self.ui, vec![found.path], Mode::Smart, false, opts);
    }
}

impl Ui {
    /// Go to the folder that holds `path` and select and show its entry.
    pub fn show_in_folder(self: &Rc<Self>, path: &Path) {
        let Some(parent) = path.parent() else { return };
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
        let this = self.clone();
        self.navigate(parent, true, move || {
            for i in 0..this.view.selection.n_items() {
                if this.row_at(i).is_some_and(|row| row.name == name) {
                    this.jump_to_row(i);
                    return;
                }
            }
            crate::actions::flash(&this, "not shown in the list (hidden)");
        });
    }
}
```

- [ ] **Step 2: Commands**

`main.rs`: `mod find_dialog;`. `keys.rs` `CommandId`:

```rust
    /// Open the Find dialog on the Name field.
    FindFiles,
    /// Open the Find dialog on the Contains field.
    SearchInFiles,
```

`palette.rs` `builtin_commands()` (category "View", no shortcut):

```rust
        CommandItem { id: CommandId::FindFiles, title: Cow::Borrowed("Find Files..."), category: Cow::Borrowed("View"), shortcut: None },
        CommandItem { id: CommandId::SearchInFiles, title: Cow::Borrowed("Search in Files..."), category: Cow::Borrowed("View"), shortcut: None },
```

and in `execute_command`:

```rust
        CommandId::FindFiles => crate::find_dialog::open(ui, false),
        CommandId::SearchInFiles => crate::find_dialog::open(ui, true),
```

- [ ] **Step 3: Build and test**

Run: `cargo build -p opusdm-lister 2>&1 | grep -E "^(error|warning)" -A9 | head -40; cargo test --workspace 2>&1 | grep -E "^test result|FAILED|^error"`
Expected: builds clean, all tests PASS. If a gtk4-rs method name differs (for example `ListBox::selected_row`), use the equivalent from the crate version in `Cargo.lock`.

- [ ] **Step 4: Manual check (scratch config, a temp tree with a few `.log` files and a `TODO` in some `.rs` files)**

`XDG_CONFIG_HOME=$(mktemp -d)`: palette -> "Find Files...", Name `*.log`, Folder the tree, Search: rows appear as they are found and the status ends `Done: N found (...)`. Select a row, "Show in folder": the Lister goes to that folder with the file selected and scrolled into view. "Search in Files...", Contains `TODO`, Name `*.rs`: rows read `path:LINE  text`; double click opens the Viewer scrolled to the match with the search bar showing `1/N`. Stop during a big search shows `Stopped: ...`; closing the dialog mid-search stops it; starting a second search clears the list and never shows rows of the first.

- [ ] **Step 5: Commit**

```bash
git add crates/lister/src/find_dialog.rs crates/lister/src/advanced_select.rs crates/lister/src/main.rs crates/lister/src/keys.rs crates/lister/src/palette.rs
git commit -m "feat(lister): Find dialog for Find Files and Search in Files"
```

---

### Task 4: Documentation, tracking and final verification

**Files:**
- Modify: `docs/usage.md`, `CHANGES.md`, `TODO.md`

- [ ] **Step 1: usage.md**

Before `### 3.11 Theme Presets & File Icons` add:

```markdown
### 3.10g Find Files and Search in Files
**Find Files...** and **Search in Files...** (command palette) open the same **Find** dialog; they only differ in where the cursor starts.
- **Name**: file name patterns, the same as Show / Hide (`*.log, *.rs`, `!test*` excludes). Empty matches every name.
- **Contains**: text to look for inside files. Empty means a search by name only (folders that match the name are listed too); filled, only files whose name matches and whose content holds the text are listed, as `path:LINE  text`, with the number of matches when there are several.
- **Folder** starts as the Lister's folder. Hidden names (starting with `.`) are skipped with everything below them unless **Include hidden** is on, symbolic links are not followed, and binary files are never searched. Text is searched in the first 16 MiB of each file; a search stops at 5000 results.
- Results appear while the search runs; **Stop** or closing the dialog cancels it. The status line ends with how many entries were scanned and how many could not be read.
- **Show in folder** takes the Lister to the folder that holds the selected result and selects it. **View** opens the file in the Viewer, on the first match of the text. Double click does View for content results and Show in folder for name results.
```

- [ ] **Step 2: CHANGES.md**

Read the file first, then append to the `## 2026-09-29` section (never open it for writing before reading it):

```markdown
- Feature (F-19, F-20): Find Files and Search in Files. New pure `find_search.rs`: `FindQuery`, `scan_text` (first hit with 1-based line, trimmed 120-character snippet, occurrence count; NUL in the first 8 KiB means binary; lossy UTF-8), `run_search` (walkdir without following links, hidden names and their subtrees skipped unless asked, cancel flag checked per entry, `Scanned` progress every 200 entries, `MAX_RESULTS` 5000, `SEARCH_FILE_MAX` 16 MiB, unreadable entries counted as skipped), plus `result_label` and `status_text`. `find_dialog.rs` is the single Find dialog (Name, Contains, Folder, Case sensitive, Include hidden) that streams `SearchEvent`s from a worker thread over an `async_channel`; new search, Stop and Close cancel the running one and a generation counter drops late events. "Show in folder" (`Ui::show_in_folder`) and "View" (Viewer on the first match through `ViewerOptions::initial_search`). Palette: "Find Files...", "Search in Files...". The search limits are recorded in `NEXT-PHASE.md`.
```

- [ ] **Step 3: TODO.md**

Read it, then tick F-19 and F-20 (`- [x]`) and item 5 of the suggested order (`5. [x] Search trio (F-19, F-20, F-21) + viewer upgrades (F-16, F-17)`).

- [ ] **Step 4: Final verification**

Run: `cargo test --workspace 2>&1 | grep -E "^test result|FAILED|SIGABRT"` and `cargo clippy -p opusdm-lister 2>&1 | grep -E "find_search|find_dialog" | head`
Expected: all results `ok`, no clippy findings in the new files. Then the acceptance checks: find `*.log` under a folder and jump to a hit with it selected; find "TODO" in `crates/` and open a hit scrolled to the match.

- [ ] **Step 5: Commit**

```bash
git add docs/usage.md CHANGES.md TODO.md
git commit -m "docs: Find Files and Search in Files (F-19, F-20)"
```
