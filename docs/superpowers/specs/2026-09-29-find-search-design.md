# Find Files and Search in Files (F-19, F-20)

Block C of item 5 of the suggested order (see `TODO.md`). Blocks A (F-21) and B
(F-16, F-17) are done. F-20 opens hits in the Viewer through
`ViewerOptions::initial_search`, which block B already provides.

## Goal

Find files by name anywhere under a folder and search inside files for a text,
from one dialog, with results that stream in while the search runs, and jump
from a result to its folder in the Lister or to the file in the Viewer.

## Decisions taken with the user

- **The search runs in a thread of the Lister**, not as a hub job: nothing else
  needs the results, so no new protocol and no hub involvement. Closing the
  dialog or pressing Stop cancels it.
- **One "Find" dialog** with two optional criteria: Name and Contains. With
  Contains empty it is a name search (F-19); with Contains filled it is a
  content search (F-20) over the files whose names pass the Name filter. The
  palette commands "Find Files..." and "Search in Files..." open the same dialog
  with the cursor on Name or on Contains.

## 1. Pure module (`crates/lister/src/find_search.rs`, no GTK)

```rust
pub struct FindQuery {
    pub name: NameFilter,        // NameFilter::new(text, "")
    pub contains: Option<String>,
    pub case_sensitive: bool,
    pub include_hidden: bool,
    pub root: PathBuf,
}
pub struct TextHit { pub line: usize, pub snippet: String, pub count: usize }
pub struct Found { pub path: PathBuf, pub is_dir: bool, pub hit: Option<TextHit> }
pub enum SearchEvent {
    Found(Found),
    Scanned(usize),                                   // entries looked at so far
    Done { scanned: usize, skipped: usize, truncated: bool },
}
pub const MAX_RESULTS: usize = 5000;
pub const SEARCH_FILE_MAX: u64 = 16 * 1024 * 1024;   // bytes read per file
pub fn scan_text(bytes: &[u8], needle: &str, case_sensitive: bool) -> Option<TextHit>;
pub fn run_search(query: &FindQuery, cancel: &AtomicBool, emit: impl FnMut(SearchEvent));
```

- `NameFilter` gives the Name syntax of Show / Hide: lists separated by `,` or
  `;`, `!pattern` excludes, case-insensitive, empty matches everything.
- `scan_text` treats a buffer with a NUL byte in its first 8 KiB as binary and
  returns `None`. It decodes lossily, finds the first occurrence (ASCII-insensitive
  case folding when `case_sensitive` is false, using the same one-char-to-one-char
  fold as the Viewer search) and returns its 1-based line number, that line with
  leading whitespace trimmed and cut to 120 characters (ending in an ellipsis
  character when cut), and the number of occurrences in the buffer. An empty needle
  returns `None`.
- `run_search` walks `root` with `walkdir` without following symbolic links. A
  name that starts with `.` is skipped, with its whole subtree for directories,
  unless `include_hidden`. The root itself is never a result.
  - Name search (`contains` is `None`): every entry whose name passes the filter
    is a `Found`, directories included (`is_dir`), with `hit: None`.
  - Content search: only regular files whose names pass the filter are read
    (first `SEARCH_FILE_MAX` bytes); a `Found` is emitted only when `scan_text`
    hits. Directories are never results.
  - An entry that cannot be read (permission error, vanished file, broken
    walkdir entry) is counted in `skipped` and the search goes on.
  - `Scanned(n)` is emitted every 200 entries. `cancel` is checked before each
    entry; a cancelled search ends with `Done` (so the dialog can show
    "Stopped"). After `MAX_RESULTS` results the search stops with
    `truncated: true`.

## 2. The dialog (`crates/lister/src/find_dialog.rs`)

- Fields: **Name**, **Contains**, **Folder** (prefilled with the Lister's
  directory), check buttons **Case sensitive** and **Include hidden**. Buttons:
  **Search**, **Stop**, **Show in folder**, **View**, **Close**. Enter in a field
  starts the search.
- A search creates a fresh `Arc<AtomicBool>` cancel flag, spawns a thread
  running `run_search` and sends its events through an `async_channel`. A
  future on the GTK main loop drains the channel and appends result rows, so
  the window stays live. A new search, Stop and Close set the flag of the
  running one; results of a cancelled search are dropped (a generation counter
  guards late events).
- Status line: `Searching...  1,234 scanned, 12 found` while running, then
  `Done: 12 found (1,234 scanned, 3 unreadable)`, `Stopped: ...`, or
  `Stopped at 5000 results`. An empty Folder or one that is not a directory is
  reported in the status line without starting a search.
- Result rows: name search shows the path relative to the folder (directories with
  a trailing `/`); content search shows `relative/path:LINE  snippet`, plus
  `(N matches)` when there are several. Rows are appended in the order found.
- Actions on the selected row: **Show in folder** navigates this Lister to the
  containing folder and selects the entry, scrolling it into view (for a
  directory result, its parent and that directory). **View** opens the file in the
  Viewer in SmartRead mode, with `initial_search` set to the Contains text when
  there is one; it is disabled for directories. Double click does **View** when
  Contains is filled and **Show in folder** otherwise. The Viewer's own search
  ignores case, so with "Case sensitive" it may highlight more than the list
  reported.

## 3. Lister wiring

- `Ui::show_in_folder(path: &Path)`: navigate to `path.parent()` and, when that
  listing is applied, find the row named like `path` and call `jump_to_row`. If
  the entry is hidden by the dot-file toggle or a Show / Hide pattern, the
  Lister reports "not shown in the list (hidden)" and stays in the folder.
- Palette, category View: "Find Files..." and "Search in Files..." (new
  `CommandId::FindFiles`, `CommandId::SearchInFiles`), no default shortcuts.

## Edge cases

- Cancel during a large walk: the thread stops at the next entry and the dialog
  ends in "Stopped".
- A folder full of unreadable subtrees: they count as `skipped` and never abort
  the search.
- A file that grows or vanishes during the search is read as far as possible or
  counted as skipped.
- A very long line: the snippet is cut, the Viewer shows the full text.
- Results already shown stay when a second search is started until its first
  result arrives... they are cleared at the moment the new search starts.
- Files beyond `SEARCH_FILE_MAX` are searched only in their first 16 MiB.

## Testing

Pure unit tests:

- `scan_text`: case-insensitive and case-sensitive, 1-based line number, CRLF
  files, occurrence count, indentation trimmed, long-line snippet cut with the
  ellipsis, a NUL in the head means binary, empty needle, no hit, non-ASCII text.
- `run_search` on a temporary tree: name search finds files and directories with
  glob and `!` exclusions; hidden entries are skipped and included with the flag,
  including inside hidden directories; symbolic links are not followed (a link
  loop does not hang); content search reports only files that contain the text and
  applies the Name filter; a binary file is not reported; a directory made
  unreadable with `chmod 000` counts as skipped (skip this test when running as
  root); the cancel flag set before the start yields a `Done` without results;
  the result cap sets `truncated` (a small cap is passed to an internal
  `run_search_capped`).

Acceptance (from FEATURES.md): find `*.log` under a folder and jump to a hit with
it selected; find "TODO" in `crates/` and open a hit scrolled to the match.

## Out of scope

Regular expressions, replace, searching inside archives, search history, opening
on the exact line of a later hit (the Viewer opens on the first match), result
export, and making `SEARCH_FILE_MAX` / `MAX_RESULTS` settings (added to
`NEXT-PHASE.md`).
