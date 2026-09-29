# Viewer: Read / AnsiRead / HexRead / SmartRead and Show (F-16, F-17)

Block B of item 5 of the suggested order (see `TODO.md`). Block A (F-21) is
done; block C (F-19, F-20) follows and needs the search and the "open at a
match" entry point defined here.

## Goal

A viewer window for the files selected in a Lister: text, ANSI art (CP437 and
colour codes) and hex modes, a smart mode that picks one, search inside the
file, paging through several files, and an image slideshow that lets the user
mark pictures for deletion.

## Decisions taken with the user

- **A new Viewer window**, separate from Quick Look. Quick Look stays the light
  preview that follows the selection and closes with Space / F3 / Esc. The
  Viewer reuses the loading and rendering code of `preview.rs`.
- **Fixed read caps with a notice**: text and ANSI 16 MiB, hex 1 MiB, Quick
  Look keeps its 256 KiB `MAX_READ`. Beyond a cap the file is shown truncated and
  the header says "showing the first N". Search covers the loaded part. The caps
  are constants now; making them a setting is recorded in `NEXT-PHASE.md`.
- **The file list is the selected files only**, in every mode including Show:
  directories are skipped; with no selection the file under the cursor is used;
  a single selected image shows just that image.

## 1. Pure modules (`crates/lister/src`, no GTK)

### `ansi.rs`

```rust
pub struct Span { pub start: usize, pub end: usize, pub fg: Option<u8>, pub bg: Option<u8>, pub bold: bool }
pub struct AnsiDoc { pub text: String, pub spans: Vec<Span> }   // offsets in chars of `text`
pub fn decode_cp437(bytes: &[u8]) -> String;
pub fn parse_ansi(bytes: &[u8]) -> AnsiDoc;
pub const PALETTE: [&str; 16];   // "#rrggbb", index 0..7 normal, 8..15 bright
```

- Bytes are decoded with CP437 (a 256 entry table; control codes below 0x20 that
  are not newline, carriage return or escape show their CP437 glyph).
- SGR (`ESC [ ... m`): 0 reset, 1 bold (bold on a normal foreground 0..7 maps to
  the bright colour 8..15), 30-37 / 90-97 foreground, 40-47 / 100-107 background,
  39 / 49 default. Unknown parameters are ignored.
- `ESC [ n C` (cursor forward) inserts `n` spaces (default 1). Every other CSI
  sequence (`ESC [` up to its final byte 0x40..0x7E) is skipped silently.
- CR is dropped, LF starts a new line. `0x1A` (SUB) ends the content: a SAUCE
  record may follow and is not shown.
- Spans cover contiguous text with one style; unstyled text has no span.

### `viewer_search.rs`

```rust
pub fn find_matches(text: &str, query: &str) -> Vec<(usize, usize)>;   // char offsets, case-insensitive, non-overlapping
pub fn step(current: Option<usize>, count: usize, forward: bool) -> Option<usize>;   // wraps around
```

Empty query or empty text has no matches. `step` with no current match starts at
the first (forward) or the last (backward).

### `viewer_mode.rs`

```rust
pub enum Mode { Smart, Text, Ansi, Hex }
pub enum Resolved { Image, Text, Ansi, Hex }
pub fn resolve(mode: Mode, ext: &str, head: &[u8]) -> Resolved;
```

`Text`, `Ansi` and `Hex` resolve to themselves (an image file under `Text` is
still shown as text). `Smart` picks: image extension -> `Image`; `ans`, `diz`,
`asc` or a `ESC [` sequence in the first 4 KiB -> `Ansi`; otherwise the same
text-versus-binary probe Quick Look uses (no NUL byte and valid UTF-8, or a
known text extension) -> `Text`; else `Hex`.

### `slideshow.rs`

`Marks`: an ordered set of paths with `toggle(path) -> bool` (true when now
marked), `is_marked`, `list() -> Vec<PathBuf>` (in marking order), `len`.

## 2. The Viewer window (`viewer.rs`)

- `ViewerWindow::open(ui, files, mode, opts)` where `opts` holds an optional
  initial search string (F-20 will use it to open a file at its first match). The
  file list is built by `viewer_files(selected: Vec<PathBuf>, cursor: Option<PathBuf>)`
  (pure): directories dropped, cursor file when nothing is selected.
- Header: `index/total  name`, the meta text (size, mode, "showing the first N"
  when capped) and a close button. Content: a read-only monospace `TextView`
  (text, ANSI with `TextTag`s from the spans, hex) or a `Picture` (image mode).
- Loading runs on a thread like `PreviewWindow::load_file` (generation counter,
  superseded results dropped). The loader helpers of `preview.rs` that both
  windows need (`detect_file_kind`, `generate_hex_dump`, the capped read) are
  reused; the caps live in constants `VIEWER_TEXT_MAX`, `VIEWER_HEX_MAX`.
- Keys (only when the search entry does not have the focus): `n` / `p` next /
  previous file (also `Right` / `Left` in image mode), `t` `a` `x` `s` switch to
  Text / ANSI / Hex / Smart, `Ctrl+F` opens the search bar, `Esc` closes the
  search bar first and the window second. Mode letters are ignored in Show.
- Search bar (hidden until `Ctrl+F`): an entry, previous / next buttons and a
  `3/17` counter. Typing highlights every match (a `TextTag`) and jumps to the
  first at or after the top of the view; `Enter` / `Shift+Enter` step through the
  matches with wrap-around and scroll to the current one (a stronger tag). The
  search is case-insensitive and works on the loaded text; in hex mode it searches
  the dump text.

## 3. Show: image slideshow (F-17)

- `Show` keeps only the image files of the selection (extension test of
  `detect_file_kind`); the others are skipped; an empty result flashes "no images
  selected" and opens nothing.
- `Left` / `Right` (and `n` / `p`) move through the images. `Delete` or `m`
  toggles the mark of the current picture; the header shows a `MARKED` label and
  `N marked`.
- On close, if any picture is marked, a requester asks "Delete N marked files?"
  (Delete / Keep). Delete submits an `Op::Delete` job through the existing job
  path (`job_submit`), so the recursive-filter setting and conflict handling of
  the job engine are unchanged; Keep does nothing. Nothing is deleted before the
  confirmation.

## 4. Commands (palette, category View)

"Read (SmartRead)", "Read as Text", "Read as ANSI", "Read as Hex", "Show (image
slideshow)". No default shortcuts. Quick Look and its keys are untouched.

## Edge cases

- A file that disappears or cannot be read: the error shows in the content area
  and `n` / `p` keep working.
- A text file that is not valid UTF-8 under `Text`: decoded lossily (replacement
  characters), never an error. Under `Ansi` it is always CP437.
- Truncated files: the header says so; marks and search work on what is shown.
- Closing the Viewer while a load is in flight: the result is dropped.
- Slideshow marks are per Viewer instance; they are gone after the requester.
- A picture that fails to decode shows its name and size instead of the image.

## Testing

Pure unit tests:

- `ansi`: plain text; each SGR class (foreground, background, bright, bold
  mapping, reset, defaults); `ESC[nC` spaces; unknown CSI skipped; CR / LF; a
  truncated escape at the end of input; `0x1A` cutting the content; CP437
  samples (`0xB0..0xB2` shades, `0xC9` box corner, `0xE1` sharp s).
- `viewer_search`: case-insensitive, non-overlapping, char (not byte) offsets on
  non-ASCII text, empty query, `step` wrap forward and backward, no matches.
- `viewer_mode`: each explicit mode resolves to itself; Smart on `.png`, `.ans`,
  ANSI bytes in a `.txt`, plain text, binary; an `ESC[` after the 4 KiB probe does
  not count.
- `slideshow`: toggle twice, order of `list`.
- `viewer_files`: directories dropped, cursor fallback, empty result.

Acceptance (from FEATURES.md): SmartRead on an `.ans` file resolves to ANSI and
its spans carry the colours; a search finds matches and `step` moves between
them; three selected files page through one window; marking 2 of 5 images and
confirming deletes exactly those two.

## Out of scope

SAUCE display, ANSI animation and absolute cursor positioning, printing (F-47),
paged reading of huge files and the configurable caps (`NEXT-PHASE.md`),
opening the Viewer from the context menu, remembering the last mode.
