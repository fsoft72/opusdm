# Lister Format and Show/Hide patterns (F-22, F-23)

Block B of item 4 of the suggested order (see `TODO.md`). Block A (F-29, F-32)
is done; block C (F-24) follows.

## Goal

Let the user choose, per directory, which columns a Lister shows and in what
order, how it is sorted, how directories are separated from files, and which
file names are listed (persistent show/hide patterns). Formats are saved in
`~/.config/opusdm/formats.json` and shared live by every Lister.

## Decisions taken with the user

- Scope: everything in F-22 except the Comment column (needs F-11, not built).
  Includes the free-space gauge.
- Resolution runs on every navigation: saved entry for the directory, else the
  nearest ancestor entry with `inherit`, else the saved default, else the
  built-in format. Entering `/tmp` applies its format, `/var` falls back to the
  default.
- **Use** applies a format to the current directory only; it is dropped as soon
  as the Lister navigates elsewhere. **Save** has three variants: this
  directory, this directory and its subdirectories (`inherit`), and as default.
- The format stores the sort column: `sort` + `reverse` + `separate`. Header
  clicks still change the order live, but only Save keeps it.
- "Set Show Pattern..." / "Set Hide Pattern..." edit the format currently in
  effect and save it where it came from; when it came from the built-in format
  they create a "this directory" entry.
- The hub owns `formats.json` (same model as bindings), not each Lister.
- Column reordering uses up/down buttons, not drag and drop.

## 1. Data model (`crates/ipc/src/format.rs`, pure, no GTK)

```rust
pub enum FormatColumn { Name, Size, Date, User, Group, Access }
pub enum Separate { DirsFirst, FilesFirst, Mixed }

pub struct ListerFormat {
    pub columns: Vec<FormatColumn>, // order = display order
    pub sort: FormatColumn,
    pub reverse: bool,              // descending
    pub separate: Separate,
    pub show_glob: String,
    pub hide_glob: String,
    pub gauge: bool,
}

pub struct FormatEntry { pub path: PathBuf, pub inherit: bool, pub format: ListerFormat }
pub struct FormatStore { pub default: Option<ListerFormat>, pub entries: Vec<FormatEntry> }
```

- `ListerFormat::builtin()`: the six columns, sort Name ascending, DirsFirst,
  empty globs, no gauge. Every struct uses `#[serde(default)]` so an old or
  hand-edited file still loads.
- `sanitize`: drops duplicate columns, guarantees `Name` is present (appended
  if missing), makes `sort` fall back to `Name` when it is not a shown column.
  Also applied to the store on load and on `SetFormats`.
- `FormatStore::resolve(dir) -> ListerFormat`: exact entry; else the deepest
  ancestor entry with `inherit`; else `default`; else `builtin()`. Virtual
  archive paths skip the path steps and use the default.
- `FormatStore::upsert(path, inherit, format)`, `remove(path)`, `set_default`.

### Patterns (`crates/ipc/src/glob.rs`)

`NameFilter::new(show, hide)`. Each string is a list separated by `,` or `;`
using the existing case-insensitive `*`/`?` matcher. A pattern starting with
`!` excludes: `*.rs,!test*` matches Rust files except `test*`. A name is listed
when (`show` has no positive pattern or matches) and `hide` does not match.
Patterns apply to **files only**; directories are always listed. The dot-file
toggle (`show_hidden`) stays independent. An empty filter lists everything.

## 2. Hub

- `Welcome` gains `formats: FormatStore` (`#[serde(default)]`).
- `ToHub::SetFormats(FormatStore)`: sanitize, store, persist to
  `formats.json` on the existing persist thread, broadcast
  `ToLister::FormatsUpdate(FormatStore)` to all Listers.
- Load at start with the shared `load_json`; a malformed file falls back to an
  empty store and is logged.

## 3. Lister

- `build.rs` still creates the six columns once. `Ui::apply_format(&ListerFormat)`
  sets each column visible or hidden, re-inserts them in the format's order
  (`remove_column` + `insert_column`), then calls `sort_by_column` with the
  format's `sort`/`reverse`. The model sorter reads `separate` from a
  `Cell<Separate>` on the view and passes it to `dir_first_cmp`.
- `dir_first_cmp(a, b, col, desc, separate)`: DirsFirst / FilesFirst keep the
  current fixed leading block, Mixed skips the directory split. `desc` reverses
  the key within a block.
- `Ui` gains `format: RefCell<ListerFormat>` (effective) and
  `temp_format: RefCell<Option<(PathBuf, ListerFormat)>>` (from **Use**).
- `navigate_to`: after `cwd` is set, drop `temp_format` if its path differs from
  the new directory, resolve the effective format (temp, else store), apply it,
  and build a `NameFilter` applied in the row loop next to `show_hidden`.
  Totals and the status bar therefore exclude filtered files.
- `FormatsUpdate`: replace the store, re-resolve the current directory and, if
  the effective format changed, apply it and reload.
- Header clicks are unchanged; the current header state is what **Save** stores.

## 4. Editor (`crates/lister/src/format_editor.rs`) and commands

Palette entry "Lister Format...". The dialog shows:

- the six columns with a checkbox each and up/down buttons (Name cannot be
  unchecked);
- a sort-column dropdown and a "Reverse" checkbox;
- a separation dropdown (Directories first / Files first / Mixed);
- Show and Hide text entries;
- a "Free space gauge" checkbox.

Buttons: **Use**, **Save (this directory)**, **Save (with subdirectories)**,
**Save as default**, **Remove saved format** (deletes this directory's entry),
**Reset** (fills the dialog from the built-in format), **Cancel**. Save sends
`SetFormats` with the edited store; the resulting `FormatsUpdate` applies it.

Palette entries "Set Show Pattern..." and "Set Hide Pattern...": a one-line
prompt; on OK the effective format is copied with the new pattern and saved
where it came from (matching entry keeps its `inherit`; a default stays the
default; the built-in format creates a "this directory" entry).

## 5. Gauge

A small `gtk::LevelBar` plus tooltip ("X free of Y") in the top status bar,
shown only when the effective format has `gauge`. The value is used/total of the
filesystem holding `cwd`. `opusdm_fsops::checkfit` already has a `statvfs`
helper for free bytes; add the total there. Refreshed on every navigation and
reload. Not shown for virtual archive paths.

## Edge cases

- A saved `sort` column that the format hides: `sanitize` resets it to `Name`.
- Two entries for the same path: `upsert` replaces; on load the last wins.
- A directory that no longer exists keeps its entry (harmless); Remove is only
  offered for the current directory's own entry.
- Invalid or empty patterns are simply "no filter"; `!` alone is ignored.
- Format changes never touch the icon view; it lists the same rows and applies
  the same show/hide filter, because the filter runs while building rows.

## Testing

Pure unit tests: `resolve` (exact, inherit, nested inherit, default, built-in,
non-inherit ancestor ignored), `sanitize`, serde with missing fields,
`NameFilter` (show only, hide only, both, negation, case, directories exempt at
the call site), `dir_first_cmp` for the three separations with and without
`desc`. Hub test for the `SetFormats` roundtrip and persistence. GTK code is
kept thin; column order/visibility mapping is a pure function returning the
wanted order so it can be tested without a display.

Acceptance (from FEATURES.md): save a format for `/tmp` without User/Group;
entering `/tmp` applies it, `/var` keeps the default. Hide `*.o` persists across
reload and directory re-entry.

## Out of scope

Comment column (F-11), drag and drop reordering, per-lister formats that ignore
the store, icon-mode formats (F-28), status-bar text (done in F-32).
