# IconView Phase 1 (Walking Skeleton) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A reusable icon-grid widget crate (`opusdm-iconview`) with glyph icons, full selection, keyboard nav, rubber-band, and drag-and-drop, mounted in both the Lister (icon view mode) and the hub desktop window.

**Architecture:** New workspace crate `crates/iconview`. The widget is approach "C": a single `gtk::Fixed` inside a `gtk::ScrolledWindow`, holding one leaf widget per entry, with an internal `Arrange` trait that computes pixel positions (`GridArrange` for the Lister, `FreeArrange` for the desktop). `IconField` and `IconItem` are plain `Rc`-wrapped structs (not GObject subclasses) that expose `.widget()` and `connect_*` callback registries; this satisfies the spec's signal semantics with far less boilerplate and matches the project's "boring and obvious" rule. The widget never touches the filesystem or dispatches jobs: it takes `IconEntry` values and invokes consumer callbacks.

**Tech Stack:** Rust 2021, gtk4-rs 0.10 (`v4_10` feature), `libc` (inotify), `tempfile` (dev). No new third-party crates.

**Spec:** `docs/superpowers/specs/2026-09-07-iconview-component-design.md`

## Global Constraints

- Edition `2021`, `rust-version = "1.91"`, both from `workspace.package`.
- `gtk4 = { version = "0.10", features = ["v4_10"] }` from `workspace.dependencies` (use `gtk4.workspace = true`).
- No new third-party dependencies beyond what `workspace.dependencies` already lists.
- Never use the `-` long-dash character in code, comments, or docs; use a plain hyphen.
- All code and comments in English.
- Constants `UPPER_SNAKE_CASE`. Internal/private free functions and methods prefixed `_` only where the surrounding module already does so (this codebase does NOT prefix module-private fns with `_`, so do not add the prefix here; follow existing files like `crates/lister/src/model.rs`).
- Every public function/type gets a `///` doc comment.
- Early-return guard clauses at the top of functions, one-liners where possible (`if x.is_empty() { return; }`).
- Text files end with a trailing newline.
- After each task's final step: `git commit` only the files that task created or modified. No `--no-verify`. No AI attribution lines in commit messages. Do not `git push`.
- `CHANGES.md` at repo root is updated as the last task.

---

## File Structure

**New crate `crates/iconview`:**

| File | Responsibility |
|------|----------------|
| `Cargo.toml` | crate manifest |
| `src/lib.rs` | module declarations + public re-exports |
| `src/types.rs` | `IconKind`, `LayoutMode`, `ThumbSize`, `IconEntry` (plain data, no GTK) |
| `src/glyphs.rs` | Nerd Font glyph lookup by kind/extension (moved from lister, generalized signature) |
| `src/arrange.rs` | `ItemBox`, `Placement`, `Arrange` trait, `GridArrange`, `FreeArrange` (pure, no GTK) |
| `src/scan.rs` | `read_entries(dir, show_hidden)` (no GTK) |
| `src/dnd.rs` | `text/uri-list` encode/decode (moved from lister) |
| `src/item.rs` | `IconItem`: one leaf widget (image + editable label) |
| `src/field.rs` | `IconField`: the container widget, interaction, callbacks |

**Modified in `crates/lister`:**

| File | Change |
|------|--------|
| `Cargo.toml` | add `opusdm-iconview` path dependency |
| `src/glyphs.rs` | shrink to `decorate_name` + `type_indicator` (Row-based), calling `opusdm_iconview::glyphs::glyph_for` |
| `src/dnd.rs` | drop the moved uri-list fns, re-import from `opusdm_iconview::dnd`; add `attach_field` |
| `src/window.rs` | `Ui` gains `icon_field`, `view_mode`; `gtk::Stack` wraps the two views; toggle button; feed `set_entries` on navigate/reload; wire callbacks |
| `src/model.rs` | add `row_to_entry(row: &Row, dir: &Path) -> IconEntry` |

**Modified in `crates/hub`:**

| File | Change |
|------|--------|
| `Cargo.toml` | add `opusdm-iconview` path dependency |
| `src/lib.rs` | `pub mod desktop_files;` |
| `src/desktop_files.rs` | new: `desktop_dir()` resolution + `DesktopWatch` single-dir inotify |
| `src/toolbar.rs` | `build_desktop_area` mounts an `IconField` (Free mode) when a desktop dir exists; wire callbacks |

---

## Task 1: Crate scaffold and shared types

**Files:**
- Create: `crates/iconview/Cargo.toml`
- Create: `crates/iconview/src/lib.rs`
- Create: `crates/iconview/src/types.rs`
- Modify: `Cargo.toml` (workspace `members`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `opusdm_iconview::IconKind` (`enum { Dir, File, Symlink, Broken }`, derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `opusdm_iconview::LayoutMode` (`enum { Grid, Free }`, derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `opusdm_iconview::ThumbSize` (`enum { Normal, Large }`, derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `opusdm_iconview::IconEntry { pub name: String, pub path: PathBuf, pub kind: IconKind, pub mode: u32, pub ext: Option<String> }` (derives `Debug, Clone`)
  - `IconEntry::ext_of(name: &str) -> Option<String>` (associated fn: lowercase final extension, `None` for a leading-dot-only name like `.bashrc`)

- [ ] **Step 1: Add the crate to the workspace**

Edit the repo-root `Cargo.toml` `members` array to:

```toml
members = ["crates/ipc", "crates/fsops", "crates/hub", "crates/lister", "crates/ui", "crates/iconview"]
```

- [ ] **Step 2: Write `crates/iconview/Cargo.toml`**

```toml
[package]
name = "opusdm-iconview"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
gtk4.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 3: Write the failing test in `crates/iconview/src/types.rs`**

```rust
//! Plain data types shared across the crate and its consumers. No GTK.

use std::path::PathBuf;

/// What a directory entry is, for icon and glyph selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Dir,
    File,
    Symlink,
    Broken,
}

/// How `IconField` arranges its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Grid,
    Free,
}

/// Freedesktop thumbnail bucket. Used in phase 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    Normal,
    Large,
}

/// One entry shown by `IconField`. Built by the consumer, never mutated here.
#[derive(Debug, Clone)]
pub struct IconEntry {
    /// File name, no directory part.
    pub name: String,
    /// Absolute path.
    pub path: PathBuf,
    /// Entry kind.
    pub kind: IconKind,
    /// Unix mode bits (for exec / socket / fifo glyphs).
    pub mode: u32,
    /// Lowercase final extension, if any.
    pub ext: Option<String>,
}

impl IconEntry {
    /// The lowercase final extension of `name`, or `None`. A leading dot does
    /// not start an extension, so `.bashrc` has none.
    pub fn ext_of(name: &str) -> Option<String> {
        let stem = name.strip_prefix('.').unwrap_or(name);
        let (_, ext) = stem.rsplit_once('.')?;
        if ext.is_empty() {
            return None;
        }
        Some(ext.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_of_rules() {
        assert_eq!(IconEntry::ext_of("main.RS").as_deref(), Some("rs"));
        assert_eq!(IconEntry::ext_of("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(IconEntry::ext_of(".bashrc"), None);
        assert_eq!(IconEntry::ext_of("Makefile"), None);
        assert_eq!(IconEntry::ext_of("trailing."), None);
    }
}
```

- [ ] **Step 4: Write `crates/iconview/src/lib.rs`**

```rust
//! Reusable icon-grid widget: file and folder icons with selection, keyboard
//! navigation, rubber-band, and drag-and-drop. Used by the Lister (icon view)
//! and the hub desktop window.

pub mod arrange;
pub mod dnd;
pub mod glyphs;
pub mod scan;
pub mod types;

pub use types::{IconEntry, IconKind, LayoutMode, ThumbSize};
```

Note: `arrange`, `dnd`, `glyphs`, `scan` are created in later tasks. For this task, temporarily reduce `lib.rs` to only `pub mod types;` and `pub use types::{...};`, then restore each `pub mod` line in the task that creates it.

- [ ] **Step 5: Run tests**

Run: `cargo test -p opusdm-iconview`
Expected: PASS (1 test, `ext_of_rules`).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/iconview/Cargo.toml crates/iconview/src/lib.rs crates/iconview/src/types.rs
git commit -m "feat(iconview): crate scaffold and shared types"
```

---

## Task 2: Move glyph lookup into the crate

**Files:**
- Create: `crates/iconview/src/glyphs.rs`
- Modify: `crates/iconview/src/lib.rs` (add `pub mod glyphs;`)
- Modify: `crates/lister/src/glyphs.rs` (shrink to Row adapters)
- Modify: `crates/lister/Cargo.toml` (add dependency)

**Interfaces:**
- Consumes: `opusdm_iconview::IconKind`.
- Produces:
  - `opusdm_iconview::glyphs::glyph_for(kind: IconKind, mode: u32, name: &str) -> char`
  - public consts `GLYPH_DIR`, `GLYPH_LINK`, `GLYPH_FILE`, `GLYPH_EXEC`, `GLYPH_SOCKET`, `GLYPH_PIPE`, `GLYPH_BLOCK` (all `char`)

- [ ] **Step 1: Write `crates/iconview/src/glyphs.rs` with tests**

Port the tables and logic from `crates/lister/src/glyphs.rs` (the `S_IF*`, `EXEC_BITS`, `GLYPH_*` consts, `EXT_GLYPHS` table, and `extension_of`). Replace the `glyph_for(row: &Row)` signature:

```rust
//! Name-column / icon decoration: Nerd Font glyphs by entry kind and by file
//! extension. Pure and GTK-free. The glyphs live in the Unicode Private Use
//! Area and only render with a patched "Nerd Font" installed.

use crate::types::IconKind;

const S_IFMT: u32 = 0o170_000;
const S_IFSOCK: u32 = 0o140_000;
const S_IFBLK: u32 = 0o060_000;
const S_IFCHR: u32 = 0o020_000;
const S_IFIFO: u32 = 0o010_000;

/// Any execute bit set (owner, group or other).
const EXEC_BITS: u32 = 0o111;

/// Glyph shown for a directory.
pub const GLYPH_DIR: char = '\u{f07b}';
/// Glyph shown for a symbolic link.
pub const GLYPH_LINK: char = '\u{f481}';
/// Glyph shown for a regular file with no better match.
pub const GLYPH_FILE: char = '\u{f15b}';
/// Glyph shown for an executable regular file.
pub const GLYPH_EXEC: char = '\u{f489}';
/// Glyph shown for a socket.
pub const GLYPH_SOCKET: char = '\u{f1e6}';
/// Glyph shown for a fifo or character device.
pub const GLYPH_PIPE: char = '\u{f120}';
/// Glyph shown for a block device.
pub const GLYPH_BLOCK: char = '\u{f0a0}';

// EXT_GLYPHS: copy the full &[(&str, char)] table verbatim from
// crates/lister/src/glyphs.rs lines 36-98.
const EXT_GLYPHS: &[(&str, char)] = &[
    // ... full table copied verbatim ...
];

/// The lowercase final extension of `name`, or `None`. A leading dot does not
/// start an extension.
fn extension_of(name: &str) -> Option<String> {
    let stem = name.strip_prefix('.').unwrap_or(name);
    let (_, ext) = stem.rsplit_once('.')?;
    if ext.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// The glyph for an entry: kind first (dir, symlink, socket, fifo, block),
/// then the file extension, then executable, then a generic file.
pub fn glyph_for(kind: IconKind, mode: u32, name: &str) -> char {
    if kind == IconKind::Dir {
        return GLYPH_DIR;
    }
    if kind == IconKind::Symlink || kind == IconKind::Broken {
        return GLYPH_LINK;
    }
    match mode & S_IFMT {
        S_IFSOCK => return GLYPH_SOCKET,
        S_IFIFO | S_IFCHR => return GLYPH_PIPE,
        S_IFBLK => return GLYPH_BLOCK,
        _ => {}
    }
    if let Some(ext) = extension_of(name) {
        if let Some((_, g)) = EXT_GLYPHS.iter().find(|(e, _)| *e == ext) {
            return *g;
        }
    }
    if mode & EXEC_BITS != 0 {
        return GLYPH_EXEC;
    }
    GLYPH_FILE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_follow_kind_then_extension() {
        assert_eq!(glyph_for(IconKind::Dir, 0o040_755, "src"), GLYPH_DIR);
        assert_eq!(glyph_for(IconKind::Symlink, 0o120_777, "link"), GLYPH_LINK);
        assert_eq!(glyph_for(IconKind::File, 0o100_644, "main.rs"), '\u{e7a8}');
        assert_eq!(glyph_for(IconKind::File, 0o100_644, "app.py"), '\u{e73c}');
        assert_eq!(glyph_for(IconKind::File, 0o100_644, "clip.mkv"), '\u{f1c8}');
        assert_eq!(glyph_for(IconKind::File, 0o100_644, "notes"), GLYPH_FILE);
        assert_eq!(glyph_for(IconKind::File, 0o100_755, "runme"), GLYPH_EXEC);
    }

    #[test]
    fn special_files_get_their_own_glyph() {
        assert_eq!(glyph_for(IconKind::File, S_IFSOCK | 0o644, "app.sock"), GLYPH_SOCKET);
        assert_eq!(glyph_for(IconKind::File, S_IFIFO | 0o644, "pipe"), GLYPH_PIPE);
        assert_eq!(glyph_for(IconKind::File, S_IFBLK | 0o644, "sda"), GLYPH_BLOCK);
    }
}
```

- [ ] **Step 2: Restore `pub mod glyphs;` in `crates/iconview/src/lib.rs`**

- [ ] **Step 3: Run and verify**

Run: `cargo test -p opusdm-iconview glyphs`
Expected: PASS (2 tests).

- [ ] **Step 4: Add the dependency to `crates/lister/Cargo.toml`**

Under `[dependencies]`:

```toml
opusdm-iconview = { path = "../iconview" }
```

- [ ] **Step 5: Shrink `crates/lister/src/glyphs.rs`**

Delete the `S_IF*`, `EXEC_BITS`, `GLYPH_*`, `EXT_GLYPHS`, `extension_of`, and the old `glyph_for`. Keep `decorate_name` and `type_indicator` (both `&Row`), and rewrite `decorate_name` to call the crate:

```rust
//! Name-column decoration: converts a `Row` to a display string with an
//! optional Nerd Font glyph and `ls -F` type indicator. The glyph lookup
//! itself lives in `opusdm_iconview::glyphs`.

use crate::model::Row;
use opusdm_iconview::glyphs::glyph_for;
use opusdm_iconview::IconKind;

const S_IFMT: u32 = 0o170_000;
const S_IFSOCK: u32 = 0o140_000;
const S_IFIFO: u32 = 0o010_000;
const EXEC_BITS: u32 = 0o111;

/// The `IconKind` for a `Row`, following the same precedence the icon view uses.
fn kind_of(row: &Row) -> IconKind {
    if row.broken_link {
        return IconKind::Broken;
    }
    if row.is_symlink {
        return IconKind::Symlink;
    }
    if row.is_dir {
        return IconKind::Dir;
    }
    IconKind::File
}

/// The `ls -F` style type character, or `None` for a plain file.
pub fn type_indicator(row: &Row) -> Option<char> {
    if row.is_symlink {
        return Some('@');
    }
    if row.is_dir {
        return Some('/');
    }
    match row.mode & S_IFMT {
        S_IFSOCK => return Some('='),
        S_IFIFO => return Some('|'),
        _ => {}
    }
    (row.mode & EXEC_BITS != 0).then_some('*')
}

/// Build the Name-column text: optional glyph, the name, then either the type
/// indicator or the plain symlink marker.
pub fn decorate_name(row: &Row, icons: bool, indicators: bool) -> String {
    let mut out = String::with_capacity(row.name.len() + 16);
    if icons {
        out.push(glyph_for(kind_of(row), row.mode, &row.name));
        out.push(' ');
    }
    out.push_str(&row.name);
    if indicators {
        if let Some(c) = type_indicator(row) {
            out.push(c);
        }
    }
    if row.broken_link {
        out.push_str(" (broken link)");
    } else if row.is_symlink && !indicators {
        out.push_str(" ->");
    }
    out
}
```

Keep the `decorate_name` / `type_indicator` tests that live in that file (the `decorate_name_defaults_match_the_previous_rendering`, `decorate_name_adds_glyph_and_indicator_when_enabled`, `indicators_match_ls_dash_f`, `special_files_get_their_own_glyph_and_indicator` tests). Delete `extension_lookup_ignores_case_and_leading_dot` and `glyphs_follow_kind_then_extension` (moved to the crate). Update the `row()` helper if the deleted tests were its only users; keep it for the remaining tests.

- [ ] **Step 6: Run the full lister test suite**

Run: `cargo test -p opusdm-lister`
Expected: PASS. If `decorate_name_adds_glyph_and_indicator_when_enabled` used `GLYPH_DIR` directly, import it: `use opusdm_iconview::glyphs::GLYPH_DIR;` in that test module.

- [ ] **Step 7: Commit**

```bash
git add crates/iconview/src/glyphs.rs crates/iconview/src/lib.rs crates/lister/Cargo.toml crates/lister/src/glyphs.rs Cargo.lock
git commit -m "refactor(iconview): move glyph lookup into the crate"
```

---

## Task 3: `arrange.rs` core types and `GridArrange`

**Files:**
- Create: `crates/iconview/src/arrange.rs`
- Modify: `crates/iconview/src/lib.rs` (add `pub mod arrange;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `arrange::ItemBox { pub w: f64, pub h: f64 }` (derives `Debug, Clone, Copy`)
  - `arrange::Placement { pub positions: Vec<(f64, f64)>, pub virtual_w: f64, pub virtual_h: f64 }` (derives `Debug, Clone`)
  - `arrange::Arrange` trait: `fn place(&self, items: &[ItemBox], viewport_w: f64) -> Placement;`
  - `arrange::GridArrange { pub gap_x: f64, pub gap_y: f64 }` implementing `Arrange`
  - `GridArrange::new() -> Self` (defaults `gap_x = 12.0`, `gap_y = 12.0`)

- [ ] **Step 1: Write the failing tests**

```rust
//! Position math for `IconField`. Pure and GTK-free.
//!
//! `Arrange::place` maps a list of item sizes plus the viewport width to
//! absolute pixel positions inside the field's `Fixed`, and reports the total
//! virtual size so the surrounding `ScrolledWindow` knows its extent.

/// The measured size of one item.
#[derive(Debug, Clone, Copy)]
pub struct ItemBox {
    pub w: f64,
    pub h: f64,
}

/// Result of an arrangement pass.
#[derive(Debug, Clone)]
pub struct Placement {
    /// One (x, y) per input item, same order.
    pub positions: Vec<(f64, f64)>,
    pub virtual_w: f64,
    pub virtual_h: f64,
}

/// Strategy that positions items.
pub trait Arrange {
    /// Place `items` given a viewport `viewport_w` pixels wide.
    fn place(&self, items: &[ItemBox], viewport_w: f64) -> Placement;
}

/// Left-to-right, top-to-bottom flow into equal cells. Cell size is the max
/// item width/height plus the gap. Used by the Lister icon view.
#[derive(Debug, Clone, Copy)]
pub struct GridArrange {
    pub gap_x: f64,
    pub gap_y: f64,
}

impl GridArrange {
    /// A grid with the default 12px gaps.
    pub fn new() -> Self {
        Self { gap_x: 12.0, gap_y: 12.0 }
    }
}

impl Default for GridArrange {
    fn default() -> Self {
        Self::new()
    }
}

impl Arrange for GridArrange {
    fn place(&self, items: &[ItemBox], viewport_w: f64) -> Placement {
        // implemented in Step 3
        Placement { positions: Vec::new(), virtual_w: 0.0, virtual_h: 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxes(n: usize, w: f64, h: f64) -> Vec<ItemBox> {
        (0..n).map(|_| ItemBox { w, h }).collect()
    }

    #[test]
    fn wraps_at_the_column_count() {
        let g = GridArrange::new();
        // cell_w = 100 + 12 = 112; viewport 400 -> floor(400/112) = 3 columns.
        let p = g.place(&boxes(7, 100.0, 80.0), 400.0);
        assert_eq!(p.positions.len(), 7);
        // Item 0 at (0, 0), item 3 wraps to row 1.
        assert_eq!(p.positions[0], (0.0, 0.0));
        assert_eq!(p.positions[2], (224.0, 0.0));
        assert_eq!(p.positions[3], (0.0, 92.0));
        // 7 items, 3 per row -> 3 rows -> virtual_h = 3 * 92.
        assert_eq!(p.virtual_h, 276.0);
    }

    #[test]
    fn at_least_one_column_when_viewport_is_narrower_than_a_cell() {
        let g = GridArrange::new();
        let p = g.place(&boxes(3, 200.0, 60.0), 50.0);
        assert_eq!(p.positions[0], (0.0, 0.0));
        assert_eq!(p.positions[1], (0.0, 72.0));
        assert_eq!(p.positions[2], (0.0, 144.0));
    }

    #[test]
    fn empty_input_is_a_zero_placement() {
        let g = GridArrange::new();
        let p = g.place(&[], 400.0);
        assert!(p.positions.is_empty());
        assert_eq!(p.virtual_h, 0.0);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-iconview arrange`
Expected: FAIL (`wraps_at_the_column_count`: positions empty).

- [ ] **Step 3: Implement `GridArrange::place`**

```rust
fn place(&self, items: &[ItemBox], viewport_w: f64) -> Placement {
    if items.is_empty() {
        return Placement { positions: Vec::new(), virtual_w: 0.0, virtual_h: 0.0 };
    }
    let cell_w = items.iter().fold(0.0_f64, |m, b| m.max(b.w)) + self.gap_x;
    let cell_h = items.iter().fold(0.0_f64, |m, b| m.max(b.h)) + self.gap_y;
    let cols = ((viewport_w / cell_w).floor() as usize).max(1);
    let mut positions = Vec::with_capacity(items.len());
    for i in 0..items.len() {
        let col = i % cols;
        let row = i / cols;
        positions.push((col as f64 * cell_w, row as f64 * cell_h));
    }
    let rows = items.len().div_ceil(cols);
    Placement {
        positions,
        virtual_w: cols as f64 * cell_w,
        virtual_h: rows as f64 * cell_h,
    }
}
```

- [ ] **Step 4: Restore `pub mod arrange;` in `lib.rs` and run tests**

Run: `cargo test -p opusdm-iconview arrange`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/arrange.rs crates/iconview/src/lib.rs
git commit -m "feat(iconview): Arrange trait and GridArrange"
```

---

## Task 4: `FreeArrange`

**Files:**
- Modify: `crates/iconview/src/arrange.rs`

**Interfaces:**
- Consumes: `arrange::{ItemBox, Placement, Arrange}`.
- Produces:
  - `arrange::FreeArrange { pub known: std::collections::HashMap<usize, (f64, f64)>, pub cell: f64, pub margin: f64 }`
  - `FreeArrange::new() -> Self` (empty `known`, `cell = 96.0`, `margin = 24.0`)
  - `arrange::nearest_in_direction(from: (f64, f64), candidates: &[(usize, (f64, f64))], dx: i32, dy: i32) -> Option<usize>` (free-standing helper for keyboard nav; `dx`/`dy` in `{-1, 0, 1}`)

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn known_positions_are_used_verbatim() {
    let mut known = std::collections::HashMap::new();
    known.insert(0, (240.0, 120.0));
    known.insert(1, (40.0, 40.0));
    let f = FreeArrange { known, cell: 96.0, margin: 24.0 };
    let p = f.place(&boxes(2, 64.0, 80.0), 800.0);
    assert_eq!(p.positions[0], (240.0, 120.0));
    assert_eq!(p.positions[1], (40.0, 40.0));
    // virtual size = bounding box of positions + item extent + margin,
    // floored at the viewport.
    assert!(p.virtual_w >= 800.0);
    assert!(p.virtual_h >= 120.0 + 80.0 + 24.0);
}

#[test]
fn unknown_items_get_non_overlapping_slots() {
    let f = FreeArrange::new();
    let p = f.place(&boxes(3, 64.0, 80.0), 300.0);
    // Column-major flow on a `cell`-sized implicit grid from the top-left.
    assert_eq!(p.positions[0], (0.0, 0.0));
    assert_ne!(p.positions[1], p.positions[0]);
    assert_ne!(p.positions[2], p.positions[1]);
}

#[test]
fn unknown_items_skip_cells_taken_by_known_items() {
    let mut known = std::collections::HashMap::new();
    known.insert(1, (0.0, 0.0)); // item 1 pinned to the first cell
    let f = FreeArrange { known, cell: 96.0, margin: 24.0 };
    let p = f.place(&boxes(2, 64.0, 80.0), 300.0);
    assert_eq!(p.positions[1], (0.0, 0.0));
    assert_ne!(p.positions[0], (0.0, 0.0));
}

#[test]
fn nearest_in_direction_picks_the_closest_to_the_right() {
    let cands = vec![(0, (0.0, 0.0)), (1, (100.0, 0.0)), (2, (100.0, 90.0)), (3, (-50.0, 0.0))];
    assert_eq!(nearest_in_direction((0.0, 0.0), &cands, 1, 0), Some(1));
    assert_eq!(nearest_in_direction((0.0, 0.0), &cands, 0, 1), Some(2));
    assert_eq!(nearest_in_direction((0.0, 0.0), &cands, -1, 0), Some(3));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-iconview arrange`
Expected: FAIL (`FreeArrange` not found).

- [ ] **Step 3: Implement `FreeArrange` and `nearest_in_direction`**

```rust
use std::collections::HashMap;

/// Free placement: items with a known (x, y) sit there; the rest fill an
/// implicit `cell`-sized grid from the top-left, skipping occupied cells.
/// Used by the hub desktop window.
#[derive(Debug, Clone)]
pub struct FreeArrange {
    pub known: HashMap<usize, (f64, f64)>,
    pub cell: f64,
    pub margin: f64,
}

impl FreeArrange {
    /// Empty layout: every item auto-slots, 96px cells, 24px margin.
    pub fn new() -> Self {
        Self { known: HashMap::new(), cell: 96.0, margin: 24.0 }
    }
}

impl Default for FreeArrange {
    fn default() -> Self {
        Self::new()
    }
}

impl Arrange for FreeArrange {
    fn place(&self, items: &[ItemBox], viewport_w: f64) -> Placement {
        let cols = ((viewport_w / self.cell).floor() as i64).max(1);
        // Cells already claimed by known positions (snapped to the grid).
        let mut taken: std::collections::HashSet<(i64, i64)> = self
            .known
            .values()
            .map(|&(x, y)| ((x / self.cell).round() as i64, (y / self.cell).round() as i64))
            .collect();
        let mut next = 0_i64;
        let mut positions = vec![(0.0, 0.0); items.len()];
        for (i, pos) in positions.iter_mut().enumerate() {
            if let Some(&known) = self.known.get(&i) {
                *pos = known;
                continue;
            }
            loop {
                let cell = (next % cols, next / cols);
                next += 1;
                if taken.insert(cell) {
                    *pos = (cell.0 as f64 * self.cell, cell.1 as f64 * self.cell);
                    break;
                }
            }
        }
        let mut max_x = viewport_w;
        let mut max_y = 0.0_f64;
        for (i, &(x, y)) in positions.iter().enumerate() {
            max_x = max_x.max(x + items[i].w + self.margin);
            max_y = max_y.max(y + items[i].h + self.margin);
        }
        Placement { positions, virtual_w: max_x, virtual_h: max_y }
    }
}

/// The candidate index closest to `from` lying in the (`dx`, `dy`) half-plane.
/// `dx`/`dy` are each `-1`, `0`, or `1`. `None` if nothing lies that way.
pub fn nearest_in_direction(
    from: (f64, f64),
    candidates: &[(usize, (f64, f64))],
    dx: i32,
    dy: i32,
) -> Option<usize> {
    candidates
        .iter()
        .filter(|(_, (x, y))| {
            let ok_x = if dx > 0 { *x > from.0 } else if dx < 0 { *x < from.0 } else { true };
            let ok_y = if dy > 0 { *y > from.1 } else if dy < 0 { *y < from.1 } else { true };
            ok_x && ok_y && (*x, *y) != from
        })
        .min_by(|(_, a), (_, b)| {
            let da = (a.0 - from.0).powi(2) + (a.1 - from.1).powi(2);
            let db = (b.0 - from.0).powi(2) + (b.1 - from.1).powi(2);
            da.total_cmp(&db)
        })
        .map(|(i, _)| *i)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p opusdm-iconview arrange`
Expected: PASS (7 tests total in `arrange`).

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/arrange.rs
git commit -m "feat(iconview): FreeArrange and directional nav helper"
```

---

## Task 5: `scan.rs` directory reader

**Files:**
- Create: `crates/iconview/src/scan.rs`
- Modify: `crates/iconview/src/lib.rs` (add `pub mod scan;`)

**Interfaces:**
- Consumes: `opusdm_iconview::{IconEntry, IconKind}`.
- Produces:
  - `scan::LAYOUT_FILE: &str` (`".opusdm-layout"`)
  - `scan::read_entries(dir: &std::path::Path, show_hidden: bool) -> std::io::Result<Vec<IconEntry>>`

- [ ] **Step 1: Write the failing test**

```rust
//! Read a directory into `IconEntry` values. Pure filesystem access, no GTK.

use crate::types::{IconEntry, IconKind};
use std::io;
use std::path::Path;

/// The per-directory layout file, always excluded from the listing.
pub const LAYOUT_FILE: &str = ".opusdm-layout";

/// Read `dir` into entries. `.` and `..` are skipped, so is `LAYOUT_FILE`, and
/// dotfiles unless `show_hidden`. A symlink's kind follows its target; a
/// dangling symlink is `IconKind::Broken`.
pub fn read_entries(dir: &Path, show_hidden: bool) -> io::Result<Vec<IconEntry>> {
    // implemented in Step 3
    let _ = (dir, show_hidden);
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reports_kinds_and_filters() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        fs::create_dir(base.join("adir")).unwrap();
        fs::write(base.join("afile.txt"), b"12345").unwrap();
        fs::write(base.join(".hidden"), b"x").unwrap();
        fs::write(base.join(LAYOUT_FILE), b"{}").unwrap();
        std::os::unix::fs::symlink("afile.txt", base.join("good_link")).unwrap();
        std::os::unix::fs::symlink("nope", base.join("bad_link")).unwrap();

        let mut got = read_entries(base, false).unwrap();
        got.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["adir", "afile.txt", "bad_link", "good_link"]);

        let by = |n: &str| got.iter().find(|e| e.name == n).unwrap().clone();
        assert_eq!(by("adir").kind, IconKind::Dir);
        assert_eq!(by("afile.txt").kind, IconKind::File);
        assert_eq!(by("afile.txt").ext.as_deref(), Some("txt"));
        assert_eq!(by("good_link").kind, IconKind::Symlink);
        assert_eq!(by("bad_link").kind, IconKind::Broken);

        let with_hidden = read_entries(base, true).unwrap();
        assert!(with_hidden.iter().any(|e| e.name == ".hidden"));
        assert!(!with_hidden.iter().any(|e| e.name == LAYOUT_FILE));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-iconview scan`
Expected: FAIL (`assert_eq!(names, ...)` left is empty).

- [ ] **Step 3: Implement `read_entries`**

```rust
pub fn read_entries(dir: &Path, show_hidden: bool) -> io::Result<Vec<IconEntry>> {
    use std::os::unix::fs::MetadataExt;

    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("opusdm-iconview: skipping unreadable entry in {}: {e}", dir.display());
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == LAYOUT_FILE {
            continue;
        }
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let (kind, mode) = if file_type.is_symlink() {
            match std::fs::metadata(entry.path()) {
                Ok(m) if m.is_dir() => (IconKind::Symlink, m.mode()),
                Ok(m) => (IconKind::Symlink, m.mode()),
                Err(_) => (IconKind::Broken, 0),
            }
        } else {
            match entry.metadata() {
                Ok(m) if m.is_dir() => (IconKind::Dir, m.mode()),
                Ok(m) => (IconKind::File, m.mode()),
                Err(_) => continue,
            }
        };
        let ext = if kind == IconKind::Dir { None } else { IconEntry::ext_of(&name) };
        out.push(IconEntry { name, path: entry.path(), kind, mode, ext });
    }
    Ok(out)
}
```

Note: a symlink to a directory keeps `IconKind::Symlink` here (the glyph layer draws the link glyph either way); the two `Ok` arms are intentionally identical for clarity and will diverge in phase 2 if needed.

- [ ] **Step 4: Restore `pub mod scan;` in `lib.rs` and run tests**

Run: `cargo test -p opusdm-iconview scan`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/scan.rs crates/iconview/src/lib.rs
git commit -m "feat(iconview): read_entries directory reader"
```

---

## Task 6: Move the uri-list DnD helpers into the crate

**Files:**
- Create: `crates/iconview/src/dnd.rs`
- Modify: `crates/iconview/src/lib.rs` (add `pub mod dnd;`)
- Modify: `crates/lister/src/dnd.rs` (re-import moved fns)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `dnd::paths_to_uri_list(paths: &[std::path::PathBuf]) -> String`
  - `dnd::uris_to_paths(text: &str) -> Vec<std::path::PathBuf>`

- [ ] **Step 1: Write `crates/iconview/src/dnd.rs`**

Move `paths_to_uri_list`, `uris_to_paths`, `percent_encode_path`, `percent_decode`, and the two `#[cfg(test)]` tests (`roundtrip_simple_paths`, `ignores_comments_and_non_file_uris`) verbatim from `crates/lister/src/dnd.rs`. Header:

```rust
//! `text/uri-list` encode/decode for drag-and-drop of file selections.
```

- [ ] **Step 2: Restore `pub mod dnd;` in `lib.rs`, run tests**

Run: `cargo test -p opusdm-iconview dnd`
Expected: PASS (2 tests).

- [ ] **Step 3: Rewrite the top of `crates/lister/src/dnd.rs`**

Remove the four moved fns and the two moved tests. Replace with a re-import so the rest of the file (the `attach` fn) still compiles:

```rust
//! Drag-and-drop wiring for the Lister views. The `text/uri-list` codec lives
//! in `opusdm_iconview::dnd`; this module wires it to `ToHub::RunJob`.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_iconview::dnd::{paths_to_uri_list, uris_to_paths};
use opusdm_ipc::{Op, ToHub};
use std::rc::Rc;
```

Delete the now-unused `use std::path::PathBuf;` if the remaining code no longer needs it (the `attach` fn uses `ui.cwd_path()` which returns `PathBuf`, so keep it if referenced; otherwise remove). Run `cargo build -p opusdm-lister` and fix any unused-import warning.

- [ ] **Step 4: Run the lister suite**

Run: `cargo test -p opusdm-lister`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/dnd.rs crates/iconview/src/lib.rs crates/lister/src/dnd.rs
git commit -m "refactor(iconview): move uri-list codec into the crate"
```

---

## Task 7: `IconItem` leaf widget

**Files:**
- Create: `crates/iconview/src/item.rs`
- Modify: `crates/iconview/src/lib.rs` (add `pub mod item;` and `pub use item::IconItem;`)

**Interfaces:**
- Consumes: `opusdm_iconview::{IconEntry}`, `opusdm_iconview::glyphs::glyph_for`.
- Produces:
  - `item::IconItem` (plain struct, `#[derive(Clone)]` over `Rc` internals)
  - `IconItem::new(entry: &IconEntry, icon_size: i32) -> IconItem`
  - `IconItem::widget(&self) -> &gtk::Widget`
  - `IconItem::set_selected(&self, on: bool)`
  - `IconItem::set_cursor(&self, on: bool)`
  - `IconItem::set_icon_size(&self, px: i32)`
  - `IconItem::measure(&self) -> (f64, f64)` (natural width, height in px)
  - `IconItem::start_edit(&self)` (swap label to entry, select basename)
  - `IconItem::connect_edit_committed(&self, f: impl Fn(String) + 'static)` (fires on Enter / focus-out with changed text; never on Esc)
  - `IconItem::name(&self) -> String`

- [ ] **Step 1: Write the widget**

```rust
//! One icon-view item: a glyph (phase 2: a thumbnail) above an editable label.

use crate::glyphs::glyph_for;
use crate::types::IconEntry;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

struct Inner {
    root: gtk::Box,
    icon: gtk::Label,      // phase 2 swaps in a gtk::Picture
    name_stack: gtk::Stack,
    name_label: gtk::Label,
    name_entry: gtk::Entry,
    original_name: RefCell<String>,
    on_commit: RefCell<Option<Box<dyn Fn(String)>>>,
}

/// A single item widget. Cheap to clone (shared `Rc`).
#[derive(Clone)]
pub struct IconItem {
    inner: Rc<Inner>,
}

impl IconItem {
    /// Build an item for `entry`, icon box `icon_size` px square.
    pub fn new(entry: &IconEntry, icon_size: i32) -> IconItem {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("icon-item");
        root.set_halign(gtk::Align::Center);

        let icon = gtk::Label::new(Some(&glyph_for(entry.kind, entry.mode, &entry.name).to_string()));
        icon.add_css_class("icon-item-glyph");
        icon.set_width_request(icon_size);
        icon.set_height_request(icon_size);

        let name_label = gtk::Label::new(Some(&entry.name));
        name_label.set_wrap(true);
        name_label.set_justify(gtk::Justification::Center);
        name_label.set_max_width_chars(14);
        name_label.set_lines(2);
        name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let name_entry = gtk::Entry::new();
        name_entry.set_max_width_chars(14);

        let name_stack = gtk::Stack::new();
        name_stack.add_named(&name_label, Some("label"));
        name_stack.add_named(&name_entry, Some("entry"));
        name_stack.set_visible_child_name("label");

        root.append(&icon);
        root.append(&name_stack);

        let inner = Rc::new(Inner {
            root,
            icon,
            name_stack,
            name_label,
            name_entry,
            original_name: RefCell::new(entry.name.clone()),
            on_commit: RefCell::new(None),
        });

        let item = IconItem { inner };
        item.wire_editing();
        item
    }

    fn wire_editing(&self) {
        let this = self.clone();
        self.inner.name_entry.connect_activate(move |e| {
            this.finish_edit(Some(e.text().to_string()));
        });
        let this = self.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |_| {
            if this.inner.name_stack.visible_child_name().as_deref() == Some("entry") {
                let text = this.inner.name_entry.text().to_string();
                this.finish_edit(Some(text));
            }
        });
        self.inner.name_entry.add_controller(focus);
        let this = self.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                this.finish_edit(None);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.inner.name_entry.add_controller(key);
    }

    fn finish_edit(&self, new_text: Option<String>) {
        if self.inner.name_stack.visible_child_name().as_deref() != Some("entry") {
            return;
        }
        self.inner.name_stack.set_visible_child_name("label");
        let Some(text) = new_text else { return; };
        let changed = *self.inner.original_name.borrow() != text;
        if !changed || text.is_empty() {
            return;
        }
        self.inner.name_label.set_text(&text);
        *self.inner.original_name.borrow_mut() = text.clone();
        if let Some(cb) = self.inner.on_commit.borrow().as_ref() {
            cb(text);
        }
    }

    /// The root widget, to add to a parent container.
    pub fn widget(&self) -> &gtk::Widget {
        self.inner.root.upcast_ref()
    }

    /// Toggle the `selected` style class.
    pub fn set_selected(&self, on: bool) {
        toggle_class(&self.inner.root, "selected", on);
    }

    /// Toggle the `cursor` style class.
    pub fn set_cursor(&self, on: bool) {
        toggle_class(&self.inner.root, "cursor", on);
    }

    /// Resize the icon box.
    pub fn set_icon_size(&self, px: i32) {
        self.inner.icon.set_width_request(px);
        self.inner.icon.set_height_request(px);
    }

    /// Natural size in pixels, for the arranger.
    pub fn measure(&self) -> (f64, f64) {
        let (_, nat_w, _, _) = self.inner.root.measure(gtk::Orientation::Horizontal, -1);
        let (_, nat_h, _, _) = self.inner.root.measure(gtk::Orientation::Vertical, nat_w);
        (nat_w as f64, nat_h as f64)
    }

    /// Enter inline editing: show the entry, preselect the basename.
    pub fn start_edit(&self) {
        let name = self.inner.original_name.borrow().clone();
        self.inner.name_entry.set_text(&name);
        let split = name.rfind('.').filter(|&i| i > 0).unwrap_or(name.len());
        self.inner.name_entry.select_region(0, split as i32);
        self.inner.name_stack.set_visible_child_name("entry");
        self.inner.name_entry.grab_focus();
    }

    /// Register the rename callback.
    pub fn connect_edit_committed(&self, f: impl Fn(String) + 'static) {
        *self.inner.on_commit.borrow_mut() = Some(Box::new(f));
    }

    /// Current displayed name.
    pub fn name(&self) -> String {
        self.inner.original_name.borrow().clone()
    }
}

fn toggle_class(w: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        w.add_css_class(class);
    } else {
        w.remove_css_class(class);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::IconKind;
    use std::path::PathBuf;

    fn entry(name: &str) -> IconEntry {
        IconEntry {
            name: name.into(),
            path: PathBuf::from("/tmp").join(name),
            kind: IconKind::File,
            mode: 0o100_644,
            ext: IconEntry::ext_of(name),
        }
    }

    #[test]
    fn selected_class_toggles() {
        if gtk::init().is_err() {
            return;
        }
        let it = IconItem::new(&entry("a.txt"), 64);
        assert!(!it.inner.root.has_css_class("selected"));
        it.set_selected(true);
        assert!(it.inner.root.has_css_class("selected"));
        it.set_selected(false);
        assert!(!it.inner.root.has_css_class("selected"));
    }

    #[test]
    fn edit_commit_fires_only_on_change() {
        if gtk::init().is_err() {
            return;
        }
        let it = IconItem::new(&entry("a.txt"), 64);
        let hits = Rc::new(RefCell::new(Vec::<String>::new()));
        let h = hits.clone();
        it.connect_edit_committed(move |s| h.borrow_mut().push(s));

        it.start_edit();
        it.finish_edit(Some("a.txt".to_string())); // unchanged
        assert!(hits.borrow().is_empty());

        it.start_edit();
        it.finish_edit(Some("b.txt".to_string()));
        assert_eq!(&*hits.borrow(), &["b.txt".to_string()]);

        it.start_edit();
        it.finish_edit(None); // Esc
        assert_eq!(hits.borrow().len(), 1);
    }
}
```

- [ ] **Step 2: Add `pub mod item;` and `pub use item::IconItem;` to `lib.rs`, run tests**

Run: `cargo test -p opusdm-iconview item`
Expected: PASS (2 tests, or skipped if no display).

- [ ] **Step 3: Commit**

```bash
git add crates/iconview/src/item.rs crates/iconview/src/lib.rs
git commit -m "feat(iconview): IconItem leaf widget with inline rename"
```

---

## Task 8: `IconField` skeleton: entries, layout, sizing

**Files:**
- Create: `crates/iconview/src/field.rs`
- Modify: `crates/iconview/src/lib.rs` (`pub mod field;`, `pub use field::IconField;`)

**Interfaces:**
- Consumes: `arrange::{Arrange, GridArrange, FreeArrange, ItemBox}`, `item::IconItem`, `types::{IconEntry, LayoutMode}`.
- Produces:
  - `field::IconField` (plain struct, `#[derive(Clone)]` over `Rc`)
  - `IconField::new() -> IconField` (default `LayoutMode::Grid`, icon size 64)
  - `IconField::widget(&self) -> &gtk::Widget` (the outer `ScrolledWindow`)
  - `IconField::set_entries(&self, entries: &[IconEntry])`
  - `IconField::set_mode(&self, mode: LayoutMode)`
  - `IconField::set_icon_size(&self, px: i32)`
  - `IconField::entry_count(&self) -> usize`
  - internal: `IconField::relayout(&self)`, `IconField::items(&self) -> Vec<IconItem>` (`pub(crate)`)

- [ ] **Step 1: Write the skeleton with a GTK test**

```rust
//! The icon field: a `Fixed` in a `ScrolledWindow`, one `IconItem` per entry,
//! positioned by a pluggable `Arrange`. Interaction is added in later steps.

use crate::arrange::{Arrange, FreeArrange, GridArrange, ItemBox};
use crate::item::IconItem;
use crate::types::{IconEntry, LayoutMode};
use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Inner {
    scroller: gtk::ScrolledWindow,
    fixed: gtk::Fixed,
    items: RefCell<Vec<IconItem>>,
    mode: Cell<LayoutMode>,
    icon_size: Cell<i32>,
    arrange: RefCell<Box<dyn Arrange>>,
    free_positions: RefCell<std::collections::HashMap<usize, (f64, f64)>>,
}

/// The reusable icon view. Cheap to clone (shared `Rc`).
#[derive(Clone)]
pub struct IconField {
    inner: Rc<Inner>,
}

impl IconField {
    /// A new field in grid mode, 64px icons.
    pub fn new() -> IconField {
        let fixed = gtk::Fixed::new();
        let scroller = gtk::ScrolledWindow::builder().child(&fixed).build();
        scroller.set_hexpand(true);
        scroller.set_vexpand(true);

        let inner = Rc::new(Inner {
            scroller,
            fixed,
            items: RefCell::new(Vec::new()),
            mode: Cell::new(LayoutMode::Grid),
            icon_size: Cell::new(64),
            arrange: RefCell::new(Box::new(GridArrange::new())),
            free_positions: RefCell::new(std::collections::HashMap::new()),
        });
        let field = IconField { inner };

        // Re-flow when the viewport width changes.
        let this = field.clone();
        field.inner.scroller.connect_width_request_notify(move |_| this.relayout());
        let this = field.clone();
        let sizer = gtk::EventControllerMotion::new(); // cheap "we're realized" hook
        sizer.connect_enter(move |_, _, _| this.relayout());
        field.inner.scroller.add_controller(sizer);

        field
    }

    /// Outer widget for embedding.
    pub fn widget(&self) -> &gtk::Widget {
        self.inner.scroller.upcast_ref()
    }

    /// Replace all entries. Rebuilds the item widgets and re-lays-out.
    pub fn set_entries(&self, entries: &[IconEntry]) {
        for it in self.inner.items.borrow().iter() {
            self.inner.fixed.remove(it.widget());
        }
        let size = self.inner.icon_size.get();
        let mut items = Vec::with_capacity(entries.len());
        for e in entries {
            let it = IconItem::new(e, size);
            self.inner.fixed.put(it.widget(), 0.0, 0.0);
            items.push(it);
        }
        *self.inner.items.borrow_mut() = items;
        self.inner.free_positions.borrow_mut().clear();
        self.relayout();
    }

    /// Switch arrangement strategy.
    pub fn set_mode(&self, mode: LayoutMode) {
        self.inner.mode.set(mode);
        let boxed: Box<dyn Arrange> = match mode {
            LayoutMode::Grid => Box::new(GridArrange::new()),
            LayoutMode::Free => {
                let mut fa = FreeArrange::new();
                fa.known = self.inner.free_positions.borrow().clone();
                Box::new(fa)
            }
        };
        *self.inner.arrange.borrow_mut() = boxed;
        self.relayout();
    }

    /// Resize every item's icon box.
    pub fn set_icon_size(&self, px: i32) {
        self.inner.icon_size.set(px);
        for it in self.inner.items.borrow().iter() {
            it.set_icon_size(px);
        }
        self.relayout();
    }

    /// Number of entries currently shown.
    pub fn entry_count(&self) -> usize {
        self.inner.items.borrow().len()
    }

    pub(crate) fn items(&self) -> Vec<IconItem> {
        self.inner.items.borrow().clone()
    }

    /// Recompute positions and move every item.
    pub(crate) fn relayout(&self) {
        let items = self.inner.items.borrow();
        if items.is_empty() {
            self.inner.fixed.set_size_request(0, 0);
            return;
        }
        let boxes: Vec<ItemBox> = items
            .iter()
            .map(|it| {
                let (w, h) = it.measure();
                ItemBox { w, h }
            })
            .collect();
        let viewport_w = self.inner.scroller.width().max(1) as f64;
        if matches!(self.inner.mode.get(), LayoutMode::Free) {
            let mut fa = FreeArrange::new();
            fa.known = self.inner.free_positions.borrow().clone();
            *self.inner.arrange.borrow_mut() = Box::new(fa);
        }
        let placement = self.inner.arrange.borrow().place(&boxes, viewport_w);
        for (it, &(x, y)) in items.iter().zip(placement.positions.iter()) {
            self.inner.fixed.move_(it.widget(), x, y);
        }
        self.inner
            .fixed
            .set_size_request(placement.virtual_w as i32, placement.virtual_h as i32);
    }
}

impl Default for IconField {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::IconKind;
    use std::path::PathBuf;

    fn entries(n: usize) -> Vec<IconEntry> {
        (0..n)
            .map(|i| IconEntry {
                name: format!("file{i}.txt"),
                path: PathBuf::from(format!("/tmp/file{i}.txt")),
                kind: IconKind::File,
                mode: 0o100_644,
                ext: Some("txt".into()),
            })
            .collect()
    }

    #[test]
    fn set_entries_creates_one_widget_each() {
        if gtk::init().is_err() {
            return;
        }
        let f = IconField::new();
        f.set_entries(&entries(5));
        assert_eq!(f.entry_count(), 5);
        let mut child_count = 0;
        let mut c = f.inner.fixed.first_child();
        while let Some(w) = c {
            child_count += 1;
            c = w.next_sibling();
        }
        assert_eq!(child_count, 5);

        f.set_entries(&entries(2));
        assert_eq!(f.entry_count(), 2);
    }

    #[test]
    fn mode_and_size_changes_do_not_panic() {
        if gtk::init().is_err() {
            return;
        }
        let f = IconField::new();
        f.set_entries(&entries(3));
        f.set_mode(LayoutMode::Free);
        f.set_icon_size(96);
        f.set_mode(LayoutMode::Grid);
    }
}
```

Note: `connect_width_request_notify` plus a motion-enter hook is a pragmatic "relayout on show/resize" trigger without subclassing. If in practice the grid does not re-flow on window resize during Task 13 integration testing, add a `gtk::DrawingArea`-free size hook by connecting to the `Fixed`'s `map` signal and to the scroller's `notify::default-width` / a `SizeGroup`; document whichever works. Do not block this task on perfect resize behavior.

- [ ] **Step 2: Run tests**

Run: `cargo test -p opusdm-iconview field`
Expected: PASS (2 tests, or skipped without display).

- [ ] **Step 3: Commit**

```bash
git add crates/iconview/src/field.rs crates/iconview/src/lib.rs
git commit -m "feat(iconview): IconField skeleton with entries and layout"
```

---

## Task 9: `IconField` selection, cursor, and click

**Files:**
- Modify: `crates/iconview/src/field.rs`

**Interfaces:**
- Consumes: `IconField` internals from Task 8.
- Produces:
  - `IconField::selected_indices(&self) -> Vec<usize>`
  - `IconField::selected_paths(&self) -> Vec<std::path::PathBuf>`
  - `IconField::connect_selection_changed(&self, f: impl Fn() + 'static)`
  - `IconField::connect_activate(&self, f: impl Fn(usize) + 'static)`
  - `pub(crate)` helpers: `select_exclusive(usize)`, `toggle(usize)`, `select_range_to(usize)`, `select_all()`, `clear_selection()`, `set_cursor(usize)`, `cursor(&self) -> Option<usize>`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
#[test]
fn selection_helpers_and_signal() {
    if gtk::init().is_err() {
        return;
    }
    let f = IconField::new();
    f.set_entries(&entries(5));
    let hits = Rc::new(Cell::new(0));
    let h = hits.clone();
    f.connect_selection_changed(move || h.set(h.get() + 1));

    f.select_exclusive(1);
    assert_eq!(f.selected_indices(), vec![1]);
    f.toggle(3);
    assert_eq!(f.selected_indices(), vec![1, 3]);
    f.select_range_to(4); // anchor is 3 after toggle
    assert_eq!(f.selected_indices(), vec![1, 3, 4]);
    f.select_exclusive(0);
    assert_eq!(f.selected_indices(), vec![0]);
    f.clear_selection();
    assert!(f.selected_indices().is_empty());
    f.select_all();
    assert_eq!(f.selected_indices(), vec![0, 1, 2, 3, 4]);

    assert!(hits.get() >= 6);
}

#[test]
fn selected_paths_track_entries() {
    if gtk::init().is_err() {
        return;
    }
    let f = IconField::new();
    f.set_entries(&entries(3));
    f.select_exclusive(2);
    assert_eq!(f.selected_paths(), vec![PathBuf::from("/tmp/file2.txt")]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-iconview field::tests::selection_helpers_and_signal`
Expected: FAIL (method `connect_selection_changed` not found).

- [ ] **Step 3: Implement**

Extend `Inner` with:

```rust
    entries: RefCell<Vec<IconEntry>>,
    selection: RefCell<std::collections::BTreeSet<usize>>,
    anchor: Cell<Option<usize>>,
    cursor: Cell<Option<usize>>,
    on_selection_changed: RefCell<Vec<Box<dyn Fn()>>>,
    on_activate: RefCell<Vec<Box<dyn Fn(usize)>>>,
```

Initialise them in `new()` (empty). In `set_entries`, also store `*self.inner.entries.borrow_mut() = entries.to_vec();`, clear `selection`, set `anchor`/`cursor` to `None`, and emit selection-changed.

Add the methods:

```rust
impl IconField {
    /// Indices of selected items, ascending.
    pub fn selected_indices(&self) -> Vec<usize> {
        self.inner.selection.borrow().iter().copied().collect()
    }

    /// Absolute paths of selected items.
    pub fn selected_paths(&self) -> Vec<std::path::PathBuf> {
        let entries = self.inner.entries.borrow();
        self.inner
            .selection
            .borrow()
            .iter()
            .filter_map(|&i| entries.get(i).map(|e| e.path.clone()))
            .collect()
    }

    /// Register a selection-changed listener.
    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) {
        self.inner.on_selection_changed.borrow_mut().push(Box::new(f));
    }

    /// Register an activate (open) listener.
    pub fn connect_activate(&self, f: impl Fn(usize) + 'static) {
        self.inner.on_activate.borrow_mut().push(Box::new(f));
    }

    fn emit_selection_changed(&self) {
        self.sync_selection_classes();
        for cb in self.inner.on_selection_changed.borrow().iter() {
            cb();
        }
    }

    fn emit_activate(&self, i: usize) {
        for cb in self.inner.on_activate.borrow().iter() {
            cb(i);
        }
    }

    fn sync_selection_classes(&self) {
        let sel = self.inner.selection.borrow();
        let cur = self.inner.cursor.get();
        for (i, it) in self.inner.items.borrow().iter().enumerate() {
            it.set_selected(sel.contains(&i));
            it.set_cursor(cur == Some(i));
        }
    }

    pub(crate) fn select_exclusive(&self, i: usize) {
        let mut sel = self.inner.selection.borrow_mut();
        sel.clear();
        sel.insert(i);
        drop(sel);
        self.inner.anchor.set(Some(i));
        self.inner.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(crate) fn toggle(&self, i: usize) {
        {
            let mut sel = self.inner.selection.borrow_mut();
            if !sel.remove(&i) {
                sel.insert(i);
            }
        }
        self.inner.anchor.set(Some(i));
        self.inner.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(crate) fn select_range_to(&self, i: usize) {
        let a = self.inner.anchor.get().unwrap_or(i);
        let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
        {
            let mut sel = self.inner.selection.borrow_mut();
            sel.clear();
            for k in lo..=hi {
                sel.insert(k);
            }
        }
        self.inner.cursor.set(Some(i));
        self.emit_selection_changed();
    }

    pub(crate) fn select_all(&self) {
        let n = self.inner.items.borrow().len();
        {
            let mut sel = self.inner.selection.borrow_mut();
            sel.clear();
            for k in 0..n {
                sel.insert(k);
            }
        }
        self.emit_selection_changed();
    }

    pub(crate) fn clear_selection(&self) {
        self.inner.selection.borrow_mut().clear();
        self.emit_selection_changed();
    }

    pub(crate) fn set_cursor(&self, i: usize) {
        self.inner.cursor.set(Some(i));
        self.sync_selection_classes();
    }

    pub(crate) fn cursor(&self) -> Option<usize> {
        self.inner.cursor.get()
    }
}
```

Wire a `gtk::GestureClick` in `new()` on `self.inner.fixed`:

```rust
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        let this = field.clone();
        click.connect_pressed(move |g, n_press, x, y| {
            let hit = this.item_at(x, y);
            let state = g.current_event_state();
            let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            match hit {
                Some(i) if n_press >= 2 => this.emit_activate(i),
                Some(i) if ctrl => this.toggle(i),
                Some(i) if shift => this.select_range_to(i),
                Some(i) => this.select_exclusive(i),
                None if !ctrl && !shift => this.clear_selection(),
                None => {}
            }
        });
        field.inner.fixed.add_controller(click);
```

Add the hit-test helper (`Fixed` child positions come from the last placement; use widget bounds):

```rust
    fn item_at(&self, x: f64, y: f64) -> Option<usize> {
        let items = self.inner.items.borrow();
        for (i, it) in items.iter().enumerate() {
            let w = it.widget();
            if let Some(bounds) = w.compute_bounds(&self.inner.fixed) {
                if bounds.contains_point(&gtk::graphene::Point::new(x as f32, y as f32)) {
                    return Some(i);
                }
            }
        }
        None
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p opusdm-iconview field`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/field.rs
git commit -m "feat(iconview): IconField selection, cursor, and click handling"
```

---

## Task 10: `IconField` rubber-band and keyboard navigation

**Files:**
- Modify: `crates/iconview/src/field.rs`

**Interfaces:**
- Consumes: Task 9 internals, `arrange::nearest_in_direction`.
- Produces:
  - `pub(crate)` helper `IconField::rubber_band_to(&self, rect: (f64, f64, f64, f64), additive: bool)` (rect = x, y, w, h)
  - `pub(crate)` helper `IconField::move_cursor(&self, dx: i32, dy: i32, extend: bool, move_only: bool)`
  - key handling installed on the `Fixed` (Ctrl+A, Esc, Enter, Space, arrows, F2)

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn rubber_band_selects_intersecting_items() {
    if gtk::init().is_err() {
        return;
    }
    let f = IconField::new();
    f.set_entries(&entries(6));
    f.inner.scroller.set_size_request(400, 400);
    f.relayout();
    // A rectangle over the whole field selects everything.
    f.rubber_band_to((0.0, 0.0, 10_000.0, 10_000.0), false);
    assert_eq!(f.selected_indices().len(), 6);
    // A zero-size rectangle far away selects nothing.
    f.rubber_band_to((99_999.0, 99_999.0, 1.0, 1.0), false);
    assert!(f.selected_indices().is_empty());
}

#[test]
fn move_cursor_wraps_by_columns_in_grid() {
    if gtk::init().is_err() {
        return;
    }
    let f = IconField::new();
    f.set_entries(&entries(6));
    f.inner.scroller.set_size_request(400, 400);
    f.relayout();
    f.set_cursor(0);
    f.move_cursor(1, 0, false, true); // right
    assert_eq!(f.cursor(), Some(1));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-iconview field::tests::rubber_band_selects_intersecting_items`
Expected: FAIL (method not found).

- [ ] **Step 3: Implement**

```rust
impl IconField {
    /// Select every item whose bounds intersect `rect`. When `additive`, keep
    /// the prior selection; otherwise replace it.
    pub(crate) fn rubber_band_to(&self, rect: (f64, f64, f64, f64), additive: bool) {
        let (rx, ry, rw, rh) = rect;
        let mut sel = if additive {
            self.inner.selection.borrow().clone()
        } else {
            std::collections::BTreeSet::new()
        };
        for (i, it) in self.inner.items.borrow().iter().enumerate() {
            if let Some(b) = it.widget().compute_bounds(&self.inner.fixed) {
                let ix = b.x() as f64;
                let iy = b.y() as f64;
                let iw = b.width() as f64;
                let ih = b.height() as f64;
                let hit = ix < rx + rw && ix + iw > rx && iy < ry + rh && iy + ih > ry;
                if hit {
                    sel.insert(i);
                }
            }
        }
        *self.inner.selection.borrow_mut() = sel;
        self.emit_selection_changed();
    }

    /// Move the keyboard cursor by (`dx`, `dy`) in {-1,0,1}. `extend` grows the
    /// selection from the anchor; `move_only` leaves the selection untouched.
    pub(crate) fn move_cursor(&self, dx: i32, dy: i32, extend: bool, move_only: bool) {
        let n = self.inner.items.borrow().len();
        if n == 0 {
            return;
        }
        let cur = self.inner.cursor.get().unwrap_or(0);
        let next = if matches!(self.inner.mode.get(), LayoutMode::Grid) {
            self.grid_step(cur, dx, dy, n)
        } else {
            self.free_step(cur, dx, dy)
        };
        let Some(next) = next else { return; };
        if extend {
            self.select_range_to(next);
        } else if move_only {
            self.set_cursor(next);
        } else {
            self.select_exclusive(next);
        }
    }

    fn grid_step(&self, cur: usize, dx: i32, dy: i32, n: usize) -> Option<usize> {
        let cols = self.current_columns().max(1);
        let mut idx = cur as i64;
        idx += dx as i64;
        idx += dy as i64 * cols as i64;
        if idx < 0 || idx as usize >= n {
            return None;
        }
        Some(idx as usize)
    }

    fn free_step(&self, cur: usize, dx: i32, dy: i32) -> Option<usize> {
        let items = self.inner.items.borrow();
        let from = items.get(cur)?.widget().compute_bounds(&self.inner.fixed)?;
        let from = (from.x() as f64, from.y() as f64);
        let cands: Vec<(usize, (f64, f64))> = items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let b = it.widget().compute_bounds(&self.inner.fixed)?;
                Some((i, (b.x() as f64, b.y() as f64)))
            })
            .collect();
        crate::arrange::nearest_in_direction(from, &cands, dx, dy)
    }

    fn current_columns(&self) -> usize {
        let items = self.inner.items.borrow();
        if items.len() < 2 {
            return 1;
        }
        let y0 = items[0]
            .widget()
            .compute_bounds(&self.inner.fixed)
            .map(|b| b.y())
            .unwrap_or(0.0);
        let mut cols = 1;
        for it in items.iter().skip(1) {
            let y = it.widget().compute_bounds(&self.inner.fixed).map(|b| b.y()).unwrap_or(0.0);
            if (y - y0).abs() < 1.0 {
                cols += 1;
            } else {
                break;
            }
        }
        cols
    }
}
```

Add a `gtk::GestureDrag` on `self.inner.fixed` for the rubber-band (starts only when `item_at` at the press point is `None`), drawing the marquee via a `RefCell<Option<(f64,f64,f64,f64)>>` field and a `self.inner.fixed.set_draw_func`-equivalent. `Fixed` has no draw hook, so overlay a transparent `gtk::DrawingArea` sized to the field as the marquee layer, or connect a `gtk::GestureDrag` and on `drag-update` call `rubber_band_to`, skipping the visual marquee in phase 1 if it proves fiddly (document the decision). The selection math is the tested part; the marquee rectangle is cosmetic.

Add a `gtk::EventControllerKey` on `self.inner.fixed`:

```rust
        let key = gtk::EventControllerKey::new();
        let this = field.clone();
        key.connect_key_pressed(move |_, keyval, _, state| {
            let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            use gtk::gdk::Key;
            match keyval {
                Key::a | Key::A if ctrl => { this.select_all(); glib::Propagation::Stop }
                Key::Escape => { this.clear_selection(); glib::Propagation::Stop }
                Key::Return | Key::KP_Enter => {
                    if let Some(i) = this.cursor() { this.emit_activate(i); }
                    glib::Propagation::Stop
                }
                Key::space => {
                    if let Some(i) = this.cursor() { this.toggle(i); }
                    glib::Propagation::Stop
                }
                Key::F2 => {
                    if let Some(i) = this.cursor() {
                        if let Some(it) = this.items().get(i) { it.start_edit(); }
                    }
                    glib::Propagation::Stop
                }
                Key::Left  => { this.move_cursor(-1, 0, shift, ctrl); glib::Propagation::Stop }
                Key::Right => { this.move_cursor( 1, 0, shift, ctrl); glib::Propagation::Stop }
                Key::Up    => { this.move_cursor( 0, -1, shift, ctrl); glib::Propagation::Stop }
                Key::Down  => { this.move_cursor( 0, 1, shift, ctrl); glib::Propagation::Stop }
                _ => glib::Propagation::Proceed,
            }
        });
        field.inner.fixed.add_controller(key);
        field.inner.fixed.set_focusable(true);
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p opusdm-iconview field`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/field.rs
git commit -m "feat(iconview): IconField rubber-band and keyboard navigation"
```

---

## Task 11: `IconField` drag-and-drop, reposition, context menu, rename, zoom

**Files:**
- Modify: `crates/iconview/src/field.rs`

**Interfaces:**
- Consumes: Task 10 internals, `dnd::{paths_to_uri_list, uris_to_paths}`.
- Produces:
  - `IconField::connect_drop_uris(&self, f: impl Fn(Vec<std::path::PathBuf>, i32, bool) + 'static)` (paths, target index or `-1`, shift held)
  - `IconField::connect_context_menu(&self, f: impl Fn(i32, f64, f64) + 'static)` (index or `-1`, x, y)
  - `IconField::connect_rename_requested(&self, f: impl Fn(usize, String) + 'static)`
  - `IconField::connect_zoom_requested(&self, f: impl Fn(i32) + 'static)`
  - `IconField::connect_move_icons(&self, f: impl Fn() + 'static)`
  - `IconField::free_positions(&self) -> std::collections::HashMap<usize, (f64, f64)>`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn drop_uris_callback_gets_parsed_paths() {
    if gtk::init().is_err() {
        return;
    }
    let f = IconField::new();
    f.set_entries(&entries(2));
    let got = Rc::new(RefCell::new(None::<(Vec<PathBuf>, i32, bool)>));
    let g = got.clone();
    f.connect_drop_uris(move |paths, target, shift| *g.borrow_mut() = Some((paths, target, shift)));

    f.handle_drop_text("file:///tmp/a.txt\r\nfile:///tmp/b%20c.txt\r\n", -1, true);
    let (paths, target, shift) = got.borrow().clone().unwrap();
    assert_eq!(paths, vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b c.txt")]);
    assert_eq!(target, -1);
    assert!(shift);
}

#[test]
fn reposition_updates_free_positions_and_fires_move_icons() {
    if gtk::init().is_err() {
        return;
    }
    let f = IconField::new();
    f.set_entries(&entries(3));
    f.set_mode(LayoutMode::Free);
    let fired = Rc::new(Cell::new(0));
    let fr = fired.clone();
    f.connect_move_icons(move || fr.set(fr.get() + 1));

    f.apply_reposition(&[0, 2], 25.0, 40.0);
    let pos = f.free_positions();
    assert!(pos.contains_key(&0));
    assert!(pos.contains_key(&2));
    assert!(!pos.contains_key(&1));
    assert_eq!(fired.get(), 1);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-iconview field::tests::drop_uris_callback_gets_parsed_paths`
Expected: FAIL (method `connect_drop_uris` not found).

- [ ] **Step 3: Implement**

Extend `Inner` with callback vectors: `on_drop_uris`, `on_context_menu`, `on_rename_requested`, `on_zoom_requested`, `on_move_icons`. In `set_entries`, after building each `IconItem`, wire its rename callback:

```rust
            let this = field_weak.clone(); // capture an IconField clone
            let idx = items.len();
            it.connect_edit_committed(move |new_name| {
                for cb in this.inner.on_rename_requested.borrow().iter() {
                    cb(idx, new_name.clone());
                }
            });
```

(Capture a `IconField` clone before the loop; `idx` is the item's position at build time.)

Add the public `connect_*` registrars (same shape as Task 9). Add:

```rust
impl IconField {
    /// Parse a `text/uri-list` payload and fan it out to the drop listeners.
    pub(crate) fn handle_drop_text(&self, text: &str, target: i32, shift: bool) {
        let paths = crate::dnd::uris_to_paths(text);
        if paths.is_empty() {
            return;
        }
        for cb in self.inner.on_drop_uris.borrow().iter() {
            cb(paths.clone(), target, shift);
        }
    }

    /// Offset the given items by (`dx`, `dy`), store their new free positions,
    /// re-lay-out, and notify move-icons listeners.
    pub(crate) fn apply_reposition(&self, indices: &[usize], dx: f64, dy: f64) {
        {
            let items = self.inner.items.borrow();
            let mut fp = self.inner.free_positions.borrow_mut();
            for &i in indices {
                let base = fp.get(&i).copied().or_else(|| {
                    items.get(i).and_then(|it| {
                        it.widget().compute_bounds(&self.inner.fixed).map(|b| (b.x() as f64, b.y() as f64))
                    })
                });
                if let Some((x, y)) = base {
                    fp.insert(i, ((x + dx).max(0.0), (y + dy).max(0.0)));
                }
            }
        }
        self.relayout();
        for cb in self.inner.on_move_icons.borrow().iter() {
            cb();
        }
    }

    /// Snapshot of the current free-mode positions.
    pub fn free_positions(&self) -> std::collections::HashMap<usize, (f64, f64)> {
        self.inner.free_positions.borrow().clone()
    }
}
```

Wire the real GTK controllers in `new()`:

- `gtk::DragSource` on `self.inner.fixed`, actions `COPY | MOVE`. `connect_prepare`: `let paths = this.selected_paths(); if paths.is_empty() { return None; } Some(gtk::gdk::ContentProvider::for_bytes("text/uri-list", &glib::Bytes::from(paths_to_uri_list(&paths).as_bytes())))`. Guard: if an internal reposition drag is active (a `Cell<bool>` flag), return `None`.
- `gtk::DropTarget` for `String`, actions `COPY | MOVE`, on `connect_drop`: `let Ok(text) = value.get::<String>() else { return false; }; let target = this.item_at(x, y).map(|i| i as i32).unwrap_or(-1); let shift = t.current_event().map(|e| e.modifier_state().contains(ModifierType::SHIFT_MASK)).unwrap_or(false); this.handle_drop_text(&text, target, shift); true`.
- `gtk::GestureClick` with `set_button(BUTTON_SECONDARY)`: on pressed, `let hit = this.item_at(x, y); if let Some(i) = hit { if !this.selected_indices().contains(&i) { this.select_exclusive(i); } } for cb in ... on_context_menu { cb(hit.map(|i| i as i32).unwrap_or(-1), x, y); }`.
- `gtk::EventControllerScroll` with `Flags::VERTICAL`: on `connect_scroll`, if Ctrl is held, `let delta = if dy < 0.0 { 1 } else { -1 }; for cb in on_zoom_requested { cb(delta); } glib::Propagation::Stop` else `Proceed`.
- `gtk::GestureDrag` on `self.inner.fixed` for internal reposition, active only when `self.inner.mode.get() == Free` and the press lands on a selected item: on `drag-begin` set the flag and record the start; on `drag-update` `self.inner.fixed.move_` the selected items live by the running offset; on `drag-end` call `apply_reposition(&self.selected_indices(), offset_x, offset_y)` and clear the flag. Use a 4px threshold before treating motion as a drag.

- [ ] **Step 4: Run tests**

Run: `cargo test -p opusdm-iconview field`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/field.rs
git commit -m "feat(iconview): IconField DnD, reposition, context menu, zoom"
```

---

## Task 12: Crate CSS defaults and public surface review

**Files:**
- Create: `crates/iconview/src/style.rs`
- Modify: `crates/iconview/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `opusdm_iconview::style::CSS: &str` (the widget's default style: `.icon-item`, `.icon-item.selected`, `.icon-item.cursor`, `.icon-item-glyph`)
  - `opusdm_iconview::style::install()` (loads `CSS` into the default display at `STYLE_PROVIDER_PRIORITY_APPLICATION`, once)

- [ ] **Step 1: Write `style.rs`**

```rust
//! Default styling for the icon view. Consumers call `install()` once; a host
//! theme can override any of these rules at a higher priority.

use gtk4 as gtk;
use gtk::prelude::*;
use std::sync::Once;

/// Baseline CSS for `IconItem`. Colours use named theme lookups so the widget
/// follows light/dark automatically.
pub const CSS: &str = "\
.icon-item { padding: 6px; border-radius: 6px; }
.icon-item.selected { background-color: alpha(@accent_bg_color, 0.35); }
.icon-item.cursor { outline: 1px dashed alpha(@accent_bg_color, 0.8); outline-offset: -2px; }
.icon-item-glyph { font-family: \"Symbols Nerd Font\", \"Nerd Font\", monospace; font-size: 32px; }
";

static ONCE: Once = Once::new();

/// Load `CSS` into the default display. Safe to call repeatedly.
pub fn install() {
    ONCE.call_once(|| {
        let Some(display) = gtk::gdk::Display::default() else { return; };
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}
```

- [ ] **Step 2: Add `pub mod style;` to `lib.rs`; call `style::install()` at the end of `IconField::new()`**

- [ ] **Step 3: Review the re-export block in `lib.rs`**

Confirm `lib.rs` ends with:

```rust
pub use field::IconField;
pub use item::IconItem;
pub use types::{IconEntry, IconKind, LayoutMode, ThumbSize};
```

- [ ] **Step 4: Full crate build and test**

Run: `cargo test -p opusdm-iconview && cargo build -p opusdm-iconview`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/style.rs crates/iconview/src/lib.rs crates/iconview/src/field.rs
git commit -m "feat(iconview): default CSS and public surface"
```

---

## Task 13: Lister integration - mount the icon view behind a toggle

**Files:**
- Modify: `crates/lister/src/model.rs` (add `row_to_entry`)
- Modify: `crates/lister/src/window.rs` (`Ui` fields, `Stack`, toggle, feed entries)

**Interfaces:**
- Consumes: `opusdm_iconview::{IconField, IconEntry, IconKind, LayoutMode}`.
- Produces:
  - `model::row_to_entry(row: &Row, dir: &std::path::Path) -> IconEntry`
  - `window::ViewMode` (`enum { Details, Icons }`, `Cell`-friendly: derive `Clone, Copy, PartialEq, Eq`)
  - `Ui.icon_field: opusdm_iconview::IconField`
  - `Ui.view_mode: std::cell::Cell<ViewMode>`
  - `Ui::refresh_icon_view(self: &Rc<Self>)` (rebuilds `IconEntry` list from the sorted model, calls `set_entries`)

- [ ] **Step 1: Add `row_to_entry` with a test in `model.rs`**

```rust
/// Map a listing `Row` to an `opusdm_iconview::IconEntry`. `dir` is the folder
/// the row lives in, used to build the absolute path.
pub fn row_to_entry(row: &Row, dir: &Path) -> opusdm_iconview::IconEntry {
    use opusdm_iconview::IconKind;
    let kind = if row.broken_link {
        IconKind::Broken
    } else if row.is_symlink {
        IconKind::Symlink
    } else if row.is_dir {
        IconKind::Dir
    } else {
        IconKind::File
    };
    let ext = if row.is_dir { None } else { opusdm_iconview::IconEntry::ext_of(&row.name) };
    opusdm_iconview::IconEntry {
        name: row.name.clone(),
        path: dir.join(&row.name),
        kind,
        mode: row.mode,
        ext,
    }
}
```

Test:

```rust
#[test]
fn row_to_entry_maps_kind_and_path() {
    let mut r = row("photo.JPG", false, 10, 0);
    r.is_symlink = false;
    let e = row_to_entry(&r, Path::new("/home/x"));
    assert_eq!(e.path, Path::new("/home/x/photo.JPG"));
    assert_eq!(e.ext.as_deref(), Some("jpg"));
    assert_eq!(e.kind, opusdm_iconview::IconKind::File);

    let mut d = row("src", true, 0, 0);
    d.is_dir = true;
    assert_eq!(row_to_entry(&d, Path::new("/home/x")).kind, opusdm_iconview::IconKind::Dir);
}
```

Run: `cargo test -p opusdm-lister row_to_entry` -> PASS.

- [ ] **Step 2: Add the `ViewMode` enum and `Ui` fields in `window.rs`**

Near the top of `window.rs`:

```rust
/// Which representation the file area shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Details,
    Icons,
}
```

Add to `struct Ui`:

```rust
    /// Icon view, shown in place of the column view in `ViewMode::Icons`.
    pub icon_field: opusdm_iconview::IconField,
    /// Which view is active.
    pub view_mode: Cell<ViewMode>,
    /// Stack holding the column-view scroller and the icon field.
    pub view_stack: gtk::Stack,
```

- [ ] **Step 3: Build the stack in the window constructor**

Where the code currently does `let scroller = gtk::ScrolledWindow::builder().child(&column_view)...; root.append(&scroller);`, replace the `root.append(&scroller)` with:

```rust
    let icon_field = opusdm_iconview::IconField::new();
    icon_field.set_mode(opusdm_iconview::LayoutMode::Grid);

    let view_stack = gtk::Stack::new();
    view_stack.add_named(&scroller, Some("details"));
    view_stack.add_named(icon_field.widget(), Some("icons"));
    view_stack.set_visible_child_name("details");
    view_stack.set_vexpand(true);
    view_stack.set_margin_start(4);
    view_stack.set_margin_end(4);
    root.append(&view_stack);
```

Populate the new `Ui` fields in the `Ui { ... }` literal: `icon_field: icon_field.clone()`, `view_mode: Cell::new(ViewMode::Details)`, `view_stack: view_stack.clone()`.

- [ ] **Step 4: Add `refresh_icon_view` and call it from `navigate_to` / `reload`**

```rust
    /// Rebuild the icon view's entries from the current sorted model.
    pub fn refresh_icon_view(self: &Rc<Self>) {
        if self.view_mode.get() != ViewMode::Icons {
            return;
        }
        let dir = self.cwd.borrow().clone();
        let n = self.selection.n_items();
        let mut entries = Vec::with_capacity(n as usize);
        for i in 0..n {
            if let Some(row) = self.row_at(i) {
                entries.push(crate::model::row_to_entry(&row, &dir));
            }
        }
        self.icon_field.set_entries(&entries);
    }
```

At the end of `navigate_to` (after `self.update_status();`) and at the end of `reload`, add `self.refresh_icon_view();`. `row_at` already exists (used by `reload`); if it returns `Rc<Row>`, adjust the deref in `row_to_entry` call accordingly (`&*row`).

- [ ] **Step 5: Add the toggle control**

In the button row build area, add a toggle button:

```rust
    let view_toggle = gtk::ToggleButton::new();
    view_toggle.set_child(Some(&crate::icons::image("grid")));  // reuse icons.rs if a glyph exists; else a Label "▦"
    view_toggle.set_tooltip_text(Some("Toggle icon view"));
    button_row.append(&view_toggle);
```

Wire it after `ui` exists (in the block where other button-row callbacks are wired, using `ui_getter` / the `Rc<Ui>`):

```rust
    {
        let ui = ui.clone();
        view_toggle.connect_toggled(move |b| {
            let mode = if b.is_active() { ViewMode::Icons } else { ViewMode::Details };
            ui.view_mode.set(mode);
            ui.view_stack.set_visible_child_name(if b.is_active() { "icons" } else { "details" });
            if b.is_active() {
                ui.refresh_icon_view();
            }
        });
    }
```

If `crates/lister/src/icons.rs` has no suitable icon, use `gtk::Label::new(Some("\u{25a6}"))` as the button child and leave a `// TODO(phase-later): dedicated SVG` note is NOT allowed by the no-placeholder rule; instead just use the label glyph and move on.

- [ ] **Step 6: Build and smoke-test**

Run: `cargo build -p opusdm-lister && cargo test -p opusdm-lister`
Expected: PASS.

Manual: `./run-x11.sh` (or the project's usual run script), open a Lister, click the toggle, confirm icons appear for the current directory, arrow keys move a cursor outline, double-click on a folder navigates (works only after Task 14 wires `activate`; for now just confirm no crash and that icons render and select).

- [ ] **Step 7: Commit**

```bash
git add crates/lister/src/model.rs crates/lister/src/window.rs
git commit -m "feat(lister): icon view mode behind a toggle"
```

---

## Task 14: Lister integration - wire icon view callbacks

**Files:**
- Modify: `crates/lister/src/dnd.rs` (add `attach_field`)
- Modify: `crates/lister/src/window.rs` (connect callbacks)

**Interfaces:**
- Consumes: `Ui`, `opusdm_iconview::IconField`, existing handlers (`navigate_to`, `actions` rename, `context_menu`).
- Produces:
  - `dnd::attach_field(ui: &Rc<Ui>, field: &opusdm_iconview::IconField)` (wires `connect_drop_uris` to `ToHub::RunJob`)

- [ ] **Step 1: Add `attach_field` in `dnd.rs`**

```rust
/// Wire an `IconField`'s drop signal to `ToHub::RunJob`, mirroring `attach`
/// for the column view. Shift at drop time means move, otherwise copy.
pub fn attach_field(ui: &Rc<Ui>, field: &opusdm_iconview::IconField) {
    let ui = ui.clone();
    field.connect_drop_uris(move |paths, _target, shift| {
        if paths.is_empty() {
            return;
        }
        let op = if shift { Op::Move } else { Op::Copy };
        let dest_dir = Some(ui.cwd_path());
        let _ = ui.client.send(&ToHub::RunJob { op, srcs: paths, dest_dir });
    });
}
```

(`paths` is already `Vec<PathBuf>`, so `uris_to_paths` is not needed here; keep the `use opusdm_iconview::dnd::...` import only if `attach` still needs it.)

- [ ] **Step 2: Connect the remaining callbacks in `window.rs`**

In the block right after the `Rc<Ui>` is created and stored in `ui_slot` (near `crate::dnd::attach(&ui, &column_view);`), add:

```rust
    crate::dnd::attach_field(&ui, &ui.icon_field);
    {
        let ui = ui.clone();
        ui.icon_field.connect_activate(move |i| {
            let Some(row) = ui.row_at(i as u32) else { return; };
            let dir = ui.cwd.borrow().clone();
            let path = dir.join(&row.name);
            if row.is_dir {
                ui.navigate_to(&path);
            } else {
                ui.open_path(&path);   // existing helper used by column-view double-click
            }
        });
    }
    {
        let ui = ui.clone();
        ui.icon_field.connect_rename_requested(move |_i, new_name| {
            ui.rename_selected_to(&new_name);   // existing rename entry point in actions.rs
        });
    }
    {
        let ui = ui.clone();
        ui.icon_field.connect_context_menu(move |_idx, x, y| {
            ui.show_context_menu_at(x, y);      // existing context_menu.rs entry point
        });
    }
    {
        let ui = ui.clone();
        ui.icon_field.connect_zoom_requested(move |delta| {
            let steps = [48, 64, 96, 128];
            let cur = ui.icon_size.get();
            let idx = steps.iter().position(|&s| s == cur).unwrap_or(1) as i32;
            let next = (idx + delta).clamp(0, steps.len() as i32 - 1) as usize;
            ui.icon_size.set(steps[next]);
            ui.icon_field.set_icon_size(steps[next]);
        });
    }
```

Add `icon_size: Cell<i32>` to `Ui` (init `Cell::new(64)`). If any named handler above (`open_path`, `rename_selected_to`, `show_context_menu_at`) does not exist under that exact name, locate the equivalent the column view uses (grep for the column-view `connect_activate` / row-activated handler, the rename action, and the context-menu popup call) and either call it directly or extract a small `pub` method on `Ui` wrapping the existing body. Do not duplicate logic.

- [ ] **Step 3: Build and test**

Run: `cargo build -p opusdm-lister && cargo test -p opusdm-lister`
Expected: PASS.

Manual: toggle to icons, double-click a folder (navigates), F2 on an item then Enter with a new name (renames, list reloads), right-click (context menu appears), Ctrl+scroll (icons resize), drag a file from a details-view Lister onto an icon-view Lister (copy job runs).

- [ ] **Step 4: Commit**

```bash
git add crates/lister/src/dnd.rs crates/lister/src/window.rs
git commit -m "feat(lister): wire icon view activate, rename, menu, zoom, drop"
```

---

## Task 15: Hub integration - desktop directory resolution and watch

**Files:**
- Create: `crates/hub/src/desktop_files.rs`
- Modify: `crates/hub/src/lib.rs` (`pub mod desktop_files;`)
- Modify: `crates/hub/Cargo.toml` (add `opusdm-iconview`)

**Interfaces:**
- Consumes: `libc` (already a hub dep), `opusdm_iconview::scan::read_entries`.
- Produces:
  - `desktop_files::resolve_desktop_dir(home: &Path, user_dirs_file: Option<&Path>) -> Option<PathBuf>` (testable core)
  - `desktop_files::desktop_dir() -> Option<PathBuf>` (calls the core with real `$HOME` and `~/.config/user-dirs.dirs`)
  - `desktop_files::DesktopWatch` with `DesktopWatch::new(dir: PathBuf, on_change: impl Fn() + 'static) -> DesktopWatch` and `Drop` cleanup
  - re-export `opusdm_iconview::scan::read_entries` usage is direct, no wrapper

- [ ] **Step 1: Write `resolve_desktop_dir` with tests**

```rust
//! Resolve and watch the directory shown on the hub desktop window.

use std::path::{Path, PathBuf};

/// Resolve the desktop directory. Order:
/// 1. `XDG_DESKTOP_DIR` from `user_dirs_file` (freedesktop `user-dirs.dirs`),
///    with a leading `$HOME` or `~` expanded against `home`.
/// 2. `<home>/Desktop` if it exists.
/// 3. `None`.
pub fn resolve_desktop_dir(home: &Path, user_dirs_file: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = user_dirs_file.and_then(|f| parse_xdg_desktop(f, home)) {
        if path.is_dir() {
            return Some(path);
        }
    }
    let fallback = home.join("Desktop");
    if fallback.is_dir() {
        return Some(fallback);
    }
    None
}

/// Pull `XDG_DESKTOP_DIR="..."` out of a `user-dirs.dirs` file.
fn parse_xdg_desktop(file: &Path, home: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(file).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("XDG_DESKTOP_DIR=") else { continue; };
        let raw = rest.trim().trim_matches('"');
        let expanded = if let Some(tail) = raw.strip_prefix("$HOME/") {
            home.join(tail)
        } else if let Some(tail) = raw.strip_prefix("~/") {
            home.join(tail)
        } else {
            PathBuf::from(raw)
        };
        return Some(expanded);
    }
    None
}

/// Real-environment resolution.
pub fn desktop_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let user_dirs = home.join(".config/user-dirs.dirs");
    let user_dirs = user_dirs.exists().then_some(user_dirs);
    resolve_desktop_dir(&home, user_dirs.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn prefers_xdg_entry_when_dir_exists() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        fs::create_dir(home.join("Schreibtisch")).unwrap();
        fs::create_dir(home.join(".config")).unwrap();
        let ud = home.join(".config/user-dirs.dirs");
        fs::write(&ud, "# comment\nXDG_DESKTOP_DIR=\"$HOME/Schreibtisch\"\n").unwrap();
        assert_eq!(resolve_desktop_dir(home, Some(&ud)), Some(home.join("Schreibtisch")));
    }

    #[test]
    fn falls_back_to_desktop_then_none() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        assert_eq!(resolve_desktop_dir(home, None), None);
        fs::create_dir(home.join("Desktop")).unwrap();
        assert_eq!(resolve_desktop_dir(home, None), Some(home.join("Desktop")));
    }
}
```

- [ ] **Step 2: Add the dependency and module**

`crates/hub/Cargo.toml` under `[dependencies]`: `opusdm-iconview = { path = "../iconview" }`.
`crates/hub/src/lib.rs`: add `pub mod desktop_files;`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p opusdm-hub desktop`
Expected: PASS (2 tests).

- [ ] **Step 4: Implement `DesktopWatch`**

Port the single-directory essence of `crates/lister/src/watcher.rs`: one background thread owning an inotify fd watching one directory for `IN_CREATE | IN_DELETE | IN_MOVED_FROM | IN_MOVED_TO | IN_CLOSE_WRITE | IN_ATTRIB`, coalescing a burst with a short debounce (100ms), and calling `on_change` on the GTK main thread via `glib::MainContext::default().spawn_local` or an `async_channel`. Keep it under ~120 lines. `DesktopWatch` holds the thread handle and a stop flag; `Drop` sets the flag and closes the fd.

Add a minimal test that does not require inotify semantics to be exercised deterministically:

```rust
#[test]
fn desktop_watch_constructs_and_drops() {
    let td = tempfile::tempdir().unwrap();
    let w = DesktopWatch::new(td.path().to_path_buf(), || {});
    drop(w);
}
```

- [ ] **Step 5: Run and commit**

Run: `cargo test -p opusdm-hub desktop`
Expected: PASS.

```bash
git add crates/hub/Cargo.toml crates/hub/src/lib.rs crates/hub/src/desktop_files.rs Cargo.lock
git commit -m "feat(hub): desktop directory resolution and single-dir watch"
```

---

## Task 16: Hub integration - mount the icon field on the desktop window

**Files:**
- Modify: `crates/hub/src/toolbar.rs` (`build_desktop_area`, callback wiring, `Widgets`)

**Interfaces:**
- Consumes: `opusdm_iconview::{IconField, LayoutMode, scan}`, `desktop_files::{desktop_dir, DesktopWatch}`, `handle.cmd_tx` for spawning listers.
- Produces: none (leaf integration).

- [ ] **Step 1: Change `build_desktop_area` to accept the desktop dir and return the field**

```rust
/// Build the desktop area below the toolbar. When `desktop_dir` is `Some`, an
/// icon field fills it; otherwise the flat placeholder background is kept.
/// Returns the overlay plus the field (if mounted).
fn build_desktop_area(desktop_dir: Option<&std::path::Path>) -> (gtk::Overlay, Option<opusdm_iconview::IconField>) {
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);

    let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bg.add_css_class("opusdm-desktop");
    bg.set_hexpand(true);
    bg.set_vexpand(true);

    let field = match desktop_dir {
        Some(dir) => {
            let f = opusdm_iconview::IconField::new();
            f.set_mode(opusdm_iconview::LayoutMode::Free);
            match opusdm_iconview::scan::read_entries(dir, false) {
                Ok(entries) => f.set_entries(&entries),
                Err(e) => eprintln!("opusdm-hub: cannot read desktop {}: {e}", dir.display()),
            }
            bg.append(f.widget());
            f.widget().set_hexpand(true);
            f.widget().set_vexpand(true);
            Some(f)
        }
        None => {
            let label = gtk::Label::new(Some("OpusDM"));
            label.add_css_class("opusdm-desktop-label");
            label.set_hexpand(true);
            label.set_vexpand(true);
            label.set_halign(gtk::Align::Center);
            label.set_valign(gtk::Align::Center);
            bg.append(&label);
            None
        }
    };

    overlay.set_child(Some(&bg));
    (overlay, field)
}
```

- [ ] **Step 2: Update the caller in `run`/`build_ui`**

Where `let desktop_area = build_desktop_area();` is:

```rust
    let desktop_dir = crate::desktop_files::desktop_dir();
    let (desktop_area, desktop_field) = build_desktop_area(desktop_dir.as_deref());
    root.append(&desktop_area);
```

Keep `attach_desktop_menu(&desktop_area, &widgets);` as is (the right-click menu still works over the field; the field's own `context-menu` callback is additive, wired below).

- [ ] **Step 3: Wire the field callbacks (only when `desktop_field` is `Some`)**

```rust
    if let (Some(field), Some(dir)) = (desktop_field.clone(), desktop_dir.clone()) {
        // Open: directories spawn a Lister, files launch their default app.
        let cmd_tx = cmd_tx.clone();
        let dir_for_open = dir.clone();
        let field_for_open = field.clone();
        field.connect_activate(move |i| {
            let Some(entry) = field_for_open_entry(&field_for_open, i) else { return; };
            if entry.kind == opusdm_iconview::IconKind::Dir {
                let _ = cmd_tx.send(opusdm_hub::server::HubCmd::SpawnLister(Some(entry.path)));
            } else {
                let uri = format!("file://{}", entry.path.display());
                let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
            }
            let _ = &dir_for_open;
        });

        // Drop onto the desktop: copy (or move with Shift) into the desktop dir.
        let cmd_tx2 = cmd_tx.clone();
        let dir2 = dir.clone();
        field.connect_drop_uris(move |paths, _t, shift| {
            if paths.is_empty() { return; }
            let op = if shift { opusdm_ipc::Op::Move } else { opusdm_ipc::Op::Copy };
            let _ = cmd_tx2.send(opusdm_hub::server::HubCmd::RunJob {
                op, srcs: paths, dest_dir: Some(dir2.clone()),
            });
        });

        // Live refresh on directory changes.
        let field_for_watch = field.clone();
        let dir3 = dir.clone();
        let watch = crate::desktop_files::DesktopWatch::new(dir.clone(), move || {
            match opusdm_iconview::scan::read_entries(&dir3, false) {
                Ok(entries) => field_for_watch.set_entries(&entries),
                Err(e) => eprintln!("opusdm-hub: desktop reload failed: {e}"),
            }
        });
        widgets.desktop_watch.replace(Some(watch));
    }
```

Add `desktop_watch: RefCell<Option<crate::desktop_files::DesktopWatch>>` to `struct Widgets` (init `RefCell::new(None)`) so the watch lives as long as the window. Add a small local helper:

```rust
/// The entry behind an icon-field index, cloned. Rebuilds from the field's
/// current paths; the field does not expose entries directly, so re-read.
fn field_for_open_entry(field: &opusdm_iconview::IconField, i: usize) -> Option<opusdm_iconview::IconEntry> {
    field.selected_paths(); // no-op guard
    None
}
```

This helper cannot work as written because `IconField` exposes no entry accessor. **Resolution:** add `IconField::entry(&self, i: usize) -> Option<IconEntry>` to `crates/iconview/src/field.rs` in this task (one-line accessor over `self.inner.entries.borrow().get(i).cloned()`), commit it as part of this task, and use it directly:

```rust
        field.connect_activate(move |i| {
            let Some(entry) = field_for_open.entry(i) else { return; };
            // ... as above ...
        });
```

Also confirm the hub command names. If `opusdm_hub::server` does not expose `HubCmd::SpawnLister` / `HubCmd::RunJob` with those shapes, use the mechanism `toolbar.rs` already uses for the "New Lister" menu item (grep for it in `build_desktop_menu`) and the job-run path the rest of the hub uses. Do not invent new IPC.

- [ ] **Step 4: Build and manual test**

Run: `cargo build -p opusdm-hub && cargo test -p opusdm-hub`
Expected: PASS.

Manual: run the hub; if `~/Desktop` (or the XDG desktop dir) exists and has files, icons show on the desktop background; drag one to reposition it (stays put during the session); create a file in that dir from a terminal, the desktop refreshes; double-click a folder spawns a Lister; right-click still shows the desktop menu.

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/field.rs crates/hub/src/toolbar.rs
git commit -m "feat(hub): mount the icon field on the desktop window"
```

---

## Task 17: Workspace verification, docs, changelog

**Files:**
- Modify: `CHANGES.md`

- [ ] **Step 1: Full workspace build and test**

Run: `cargo test --workspace && cargo build --workspace`
Expected: PASS, no warnings. Fix any warning surfaced (unused imports left behind by the moves in Tasks 2 and 6 are the likely culprits).

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets`
Expected: no warnings. Address any in the new crate (the integration crates may have pre-existing lint debt; only fix what this change introduced).

- [ ] **Step 3: Prepend a `CHANGES.md` entry**

Under a new dated heading at the top of the file (match the existing style, Italian, no long dash):

```markdown
## 2026-09-07

- Nuovo crate `crates/iconview` (`opusdm_iconview`): vista a icone riutilizzabile. `IconField` e un `gtk::Fixed` in uno `ScrolledWindow` con un trait interno `Arrange` (`GridArrange` per il Lister, `FreeArrange` per il desktop). Selezione multipla, rubber-band, navigazione da tastiera, rename inline, zoom Ctrl+scroll e drag and drop (uri-list tra finestre + riposizionamento locale in modalita Free). Callback invece di segnali GObject. Nessuna dipendenza di terze parti nuova.
- `glyphs.rs` e il codec `text/uri-list` di `dnd.rs` spostati dal Lister a `opusdm_iconview`; `glyph_for` generalizzato a `glyph_for(kind, mode, name)`. Il Lister re-importa dal crate e tiene un `dnd.rs` ridotto ad adapter (`attach_field`).
- Lister: nuovo toggle "vista a icone" nella button row; uno `gtk::Stack` alterna `ColumnView` e `IconField`. In modalita icone gli `IconEntry` sono derivati dai `Row` gia ordinati (`model::row_to_entry`), senza rileggere la directory. `activate`/`rename`/`context-menu`/`zoom`/`drop` cablati sugli handler esistenti.
- Hub: la finestra desktop mostra il contenuto di `$XDG_DESKTOP_DIR` (fallback `~/Desktop`, altrimenti area vuota come prima) in un `IconField` in modalita `Free`. Nuovo `crates/hub/src/desktop_files.rs`: risoluzione della cartella + watch inotify a directory singola. Doppio click su cartella apre un Lister, sui file usa l'app di default; drop sul desktop lancia un job di copia/spostamento; il menu contestuale del desktop resta invariato.
- Le posizioni delle icone sul desktop non sono ancora persistite fra i riavvii (fase 3). Le thumbnail di immagini/PDF/video arrivano nella fase 2; per ora ogni icona e un glyph Nerd Font.
```

- [ ] **Step 4: Commit**

```bash
git add CHANGES.md
git commit -m "docs: changelog for the iconview component (phase 1)"
```

---

## Self-Review

**1. Spec coverage:**

| Spec section | Task(s) |
|--------------|---------|
| Struttura del crate | 1, 2, 3, 5, 6, 7, 8, 12 |
| API pubblica (`IconEntry`, `IconField`, setters) | 1, 8, 9, 11 |
| Segnali (`activate`, `selection-changed`, `rename-requested`, `zoom-requested`, `context-menu`, `drop-uris`, `move-icons`) | 9 (`activate`, `selection-changed`), 11 (rest) |
| Il seam `Arrange` / `GridArrange` | 3 |
| `FreeArrange` + navigazione direzionale | 4 |
| Interazione: selezione, click | 9 |
| Interazione: rubber-band, tastiera | 10 |
| Editing inline | 7 (widget), 10 (F2 trigger), 11 (rename-requested fan-out) |
| Zoom | 11 |
| Context menu | 11 |
| DnD tra finestre | 11 |
| Riposizionamento icone (Free) | 11 |
| `scan::read_entries` | 5 |
| Gestione errori (dir illeggibile, entry saltato, tool assenti) | 5 (entry skip, dir error), thumbnails deferred to phase 2 |
| Integrazione Lister (Stack, toggle, mapping, wiring, watcher filter) | 13, 14 |
| Integrazione hub (desktop dir, watch, mount, wiring) | 15, 16 |
| CHANGES.md | 17 |
| Persistenza `.opusdm-layout` | **Deferred to phase 3** (spec "Fase 3"); `scan::LAYOUT_FILE` const and exclusion land now in Task 5 |
| Thumbnail service + cache | **Deferred to phase 2** (spec "Fase 2") |
| `watcher.rs` filtra `.opusdm-layout` | Task 5 defines `LAYOUT_FILE`; the Lister `watcher.rs` filter change is folded into Task 13 Step 4 note — **add explicitly**: in Task 13, also add `name == opusdm_iconview::scan::LAYOUT_FILE` to the skip check in `navigate_to` so the file never shows as a row. |

Gap found and closed: Task 13 must also skip `LAYOUT_FILE` in `navigate_to`'s row loop (added to the plan text above as a note here; implementer: add the one-line guard alongside the existing `if !show_hidden && row.name.starts_with('.')` check).

**2. Placeholder scan:** One `field_for_open_entry` stub appears in Task 16 Step 3 and is immediately called out as unworkable with an explicit resolution (add `IconField::entry`). No `TODO`/`TBD` remain. The Task 13 Step 5 note explicitly forbids a placeholder comment and directs a concrete label-glyph child instead.

**3. Type consistency:**
- `IconEntry::ext_of` (Task 1) reused verbatim in Task 5 and Task 13.
- `glyph_for(kind, mode, name)` signature identical in Task 2 definition and Task 7 use.
- `Arrange::place(&self, &[ItemBox], f64) -> Placement` identical across Tasks 3, 4, 8.
- `IconField::entry(i)` is introduced in Task 16 and must also be relied on by nothing earlier — consistent (it is additive).
- Callback registrar naming is uniform: `connect_<event>` returning `()` and storing `Box<dyn Fn(...)>` in a `RefCell<Vec<...>>` (Tasks 9, 11).
- `LayoutMode` / `ViewMode`: the crate uses `LayoutMode::{Grid,Free}`; the Lister adds its own `ViewMode::{Details,Icons}` and maps one to the other in Task 13. Intentional, documented.

**4. Ambiguity:** Named Lister handlers in Task 14 (`open_path`, `rename_selected_to`, `show_context_menu_at`) may not match real names; Task 14 Step 2 explicitly instructs the implementer to grep for the column-view equivalents and reuse them rather than guess. Hub command names in Task 16 carry the same explicit fallback instruction.
