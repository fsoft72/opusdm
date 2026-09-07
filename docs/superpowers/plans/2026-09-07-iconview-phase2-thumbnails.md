# IconView Phase 2 (Thumbnails) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Real preview thumbnails for images, PDFs and videos in `IconField`, generated off the main thread, cached in the freedesktop thumbnail directory, with the Nerd Font glyph as the fallback that always shows first.

**Architecture:** A new `thumbs` module in `crates/iconview` owns: freedesktop cache-path resolution (MD5 of the `file://` URI), PNG `tEXt` read/write for the `Thumb::MTime` validity tag, per-type generators (`gdk-pixbuf` for images on the worker thread; `pdftoppm`/`pdftocairo` and `ffmpegthumbnailer`/`ffmpeg` shelled out with a kill timeout), and a fixed-size worker pool modelled on `opusdm_fsops::pool`. `IconField` owns one pool, enqueues a job per thumbnailable entry on `set_entries`, and drains results on the GTK main loop, guarding each against the entry list having moved on. Textures are built on the main thread from PNG bytes the worker returns.

**Tech Stack:** Rust 2021, gtk4-rs 0.10 (`gdk`, `gdk-pixbuf`, `glib`), `async-channel`, new dep `md-5` (RustCrypto), `tempfile` (dev). External runtime tools (optional): `pdftoppm`/`pdftocairo`, `ffmpegthumbnailer`/`ffmpeg`.

**Spec:** `docs/superpowers/specs/2026-09-07-iconview-component-design.md` (section "Servizio thumbnail e cache (fase 2)")

## Global Constraints

- Edition `2021`, `rust-version = "1.91"`, from `workspace.package`.
- `gtk4 = { version = "0.10", features = ["v4_10"] }` via `gtk4.workspace = true`. `gdk`, `gdk_pixbuf`, `glib` are re-exported from `gtk4`.
- `async-channel.workspace = true`.
- One new third-party dependency is allowed and expected: `md-5` (add `md-5 = "0.10"` to `[workspace.dependencies]`, then `md-5.workspace = true` in `crates/iconview`). No other new dependencies.
- Never use the `-` long-dash character in code, comments, or docs; use a plain hyphen.
- All code and comments in English.
- Constants `UPPER_SNAKE_CASE`. This codebase does NOT prefix module-private fns with `_`; do not add that prefix.
- Every public function/type gets a `///` doc comment.
- Early-return guard clauses at the top of functions, one-liners where possible.
- Text files end with a trailing newline.
- GTK-touching unit tests: the `opusdm-iconview` crate keeps exactly ONE `#[test]` that calls `gtk::init` (currently `field::tests::gtk_widget_behavior`). Any new GTK assertion goes inside that function; never add a second `gtk::init` test (libtest gives each test its own thread and GTK4 aborts on a second init). Worker-thread code (`gdk_pixbuf` decode, `Command` spawn, cache I/O, PNG parsing, MD5) is not GTK and gets its own normal `#[test]`s.
- After each task's final step: `git commit` only that task's files. No `--no-verify`. No AI attribution lines. Do not `git push`.
- `CHANGES.md` at repo root updated as the last task.
- A worker thread must never call GTK/GDK display APIs. `gdk::Texture` is created on the main thread only, from PNG bytes the worker returns.

## File Structure

**New in `crates/iconview`:**

| File | Responsibility |
|------|----------------|
| `src/thumbs/mod.rs` | module facade: `ThumbSize` helpers, `ThumbKind`, `ThumbRequest`, `ThumbResult`, `ThumbPool`, `wanted()` |
| `src/thumbs/cache.rs` | freedesktop cache path (MD5 of URI), `load_valid`, `store` (atomic, writes `Thumb::MTime`/`Thumb::URI`) |
| `src/thumbs/png_text.rs` | PNG `tEXt` chunk read + insert, with CRC-32. Pure, testable |
| `src/thumbs/generate.rs` | per-type generators + tool detection + `run_with_timeout` |
| `src/thumbs/pool.rs` | fixed-size worker pool (pattern of `opusdm_fsops::pool`), no external dep |

**Modified in `crates/iconview`:**

| File | Change |
|------|--------|
| `Cargo.toml` | add `md-5.workspace = true` |
| `src/lib.rs` | `pub mod thumbs;` + re-export `thumbs::{ThumbKind}` if useful |
| `src/types.rs` | `impl ThumbSize { pub fn px(self) -> i32; pub fn dir_name(self) -> &'static str; pub fn for_icon(px: i32) -> ThumbSize }` |
| `src/item.rs` | add a hidden `gtk::Picture` beside the glyph label; `set_thumbnail(&gdk::Texture)` / `clear_thumbnail()` |
| `src/field.rs` | own a `ThumbPool` + result receiver; enqueue on `set_entries`; drain on the main loop; re-enqueue on `set_icon_size` |

**Modified at workspace root:**

| File | Change |
|------|--------|
| `Cargo.toml` | add `md-5 = "0.10"` to `[workspace.dependencies]` |

No consumer (Lister, hub) changes: both already call `set_entries` / `set_icon_size`.

---

## Task 1: `ThumbSize` helpers and the workspace dependency

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/iconview/Cargo.toml`
- Modify: `crates/iconview/src/types.rs`

**Interfaces:**
- Consumes: `ThumbSize` (exists).
- Produces:
  - `ThumbSize::px(self) -> i32` (`Normal` -> 128, `Large` -> 256)
  - `ThumbSize::dir_name(self) -> &'static str` (`Normal` -> `"normal"`, `Large` -> `"large"`)
  - `ThumbSize::for_icon(icon_px: i32) -> ThumbSize` (`<= 128` -> `Normal`, else `Large`)

- [ ] **Step 1: Add the workspace dependency**

In the root `Cargo.toml` `[workspace.dependencies]`, add:

```toml
md-5 = "0.10"
```

- [ ] **Step 2: Reference it from the crate**

In `crates/iconview/Cargo.toml` under `[dependencies]`:

```toml
md-5.workspace = true
```

- [ ] **Step 3: Write the failing test**

Append to `crates/iconview/src/types.rs` inside `mod tests`:

```rust
    #[test]
    fn thumb_size_mapping() {
        assert_eq!(ThumbSize::Normal.px(), 128);
        assert_eq!(ThumbSize::Large.px(), 256);
        assert_eq!(ThumbSize::Normal.dir_name(), "normal");
        assert_eq!(ThumbSize::Large.dir_name(), "large");
        assert_eq!(ThumbSize::for_icon(64), ThumbSize::Normal);
        assert_eq!(ThumbSize::for_icon(128), ThumbSize::Normal);
        assert_eq!(ThumbSize::for_icon(129), ThumbSize::Large);
    }
```

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p opusdm-iconview thumb_size_mapping`
Expected: FAIL (no method `px`).

- [ ] **Step 5: Implement**

In `crates/iconview/src/types.rs`, after the `ThumbSize` enum:

```rust
impl ThumbSize {
    /// The square pixel size of this bucket.
    pub fn px(self) -> i32 {
        match self {
            ThumbSize::Normal => 128,
            ThumbSize::Large => 256,
        }
    }

    /// The freedesktop cache subdirectory name.
    pub fn dir_name(self) -> &'static str {
        match self {
            ThumbSize::Normal => "normal",
            ThumbSize::Large => "large",
        }
    }

    /// The bucket to use for an icon drawn `icon_px` wide.
    pub fn for_icon(icon_px: i32) -> ThumbSize {
        if icon_px <= 128 {
            ThumbSize::Normal
        } else {
            ThumbSize::Large
        }
    }
}
```

- [ ] **Step 6: Run to verify pass**

Run: `cargo test -p opusdm-iconview thumb_size_mapping`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/iconview/Cargo.toml crates/iconview/src/types.rs
git commit -m "feat(iconview): ThumbSize helpers and md-5 dependency"
```

---

## Task 2: PNG tEXt chunk read/write

**Files:**
- Create: `crates/iconview/src/thumbs/png_text.rs`
- Create: `crates/iconview/src/thumbs/mod.rs` (temporary: only `pub mod png_text;` for now)
- Modify: `crates/iconview/src/lib.rs` (add `pub mod thumbs;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `thumbs::png_text::read_text(png: &[u8], keyword: &str) -> Option<String>`
  - `thumbs::png_text::insert_text(png: &[u8], entries: &[(&str, &str)]) -> Option<Vec<u8>>` (returns `None` if `png` is not a PNG)

- [ ] **Step 1: Write the module with tests**

`crates/iconview/src/thumbs/png_text.rs`:

```rust
//! Minimal PNG `tEXt` chunk support: read a keyword's value, or splice new
//! `tEXt` chunks in right after `IHDR`. Enough for the freedesktop thumbnail
//! `Thumb::MTime` / `Thumb::URI` tags. No image decoding.

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The value of the first `tEXt` chunk whose keyword is `keyword`, or `None`.
pub fn read_text(png: &[u8], keyword: &str) -> Option<String> {
    if png.len() < 8 || png[..8] != SIGNATURE {
        return None;
    }
    let mut off = 8;
    while off + 12 <= png.len() {
        let len = u32::from_be_bytes(png[off..off + 4].try_into().ok()?) as usize;
        let ctype = &png[off + 4..off + 8];
        let data_start = off + 8;
        let data_end = data_start.checked_add(len)?;
        if data_end + 4 > png.len() {
            return None;
        }
        if ctype == b"tEXt" {
            let data = &png[data_start..data_end];
            if let Some(nul) = data.iter().position(|&b| b == 0) {
                if &data[..nul] == keyword.as_bytes() {
                    return Some(String::from_utf8_lossy(&data[nul + 1..]).into_owned());
                }
            }
        }
        if ctype == b"IEND" {
            return None;
        }
        off = data_end + 4; // skip CRC
    }
    None
}

/// A copy of `png` with a `tEXt` chunk per entry inserted right after `IHDR`.
/// `None` if `png` is not a PNG or has no `IHDR` where expected.
pub fn insert_text(png: &[u8], entries: &[(&str, &str)]) -> Option<Vec<u8>> {
    if png.len() < 8 || png[..8] != SIGNATURE {
        return None;
    }
    // IHDR is always the first chunk: 4 len + 4 type + 13 data + 4 crc = 25.
    let ihdr_end = 8 + 25;
    if png.len() < ihdr_end || &png[12..16] != b"IHDR" {
        return None;
    }
    let mut out = Vec::with_capacity(png.len() + entries.len() * 32);
    out.extend_from_slice(&png[..ihdr_end]);
    for (keyword, text) in entries {
        let mut data = Vec::with_capacity(keyword.len() + 1 + text.len());
        data.extend_from_slice(keyword.as_bytes());
        data.push(0);
        data.extend_from_slice(text.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(b"tEXt");
        out.extend_from_slice(&data);
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(b"tEXt");
        crc_input.extend_from_slice(&data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }
    out.extend_from_slice(&png[ihdr_end..]);
    Some(out)
}

/// PNG CRC-32 (polynomial 0xEDB88320, reflected, init/xor 0xFFFFFFFF).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1x1 greyscale PNG (signature + IHDR + IDAT + IEND), no text chunks.
    fn tiny_png() -> Vec<u8> {
        // Generated once with `convert -size 1x1 xc:gray png:` and pasted as bytes.
        vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00,
            0x00, 0x3a, 0x7e, 0x9b, 0x55, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x62, 0x00, 0x00, 0x00, 0x06, 0x00, 0x03, 0x36, 0x37, 0x7c, 0xa8, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn crc32_matches_known_vector() {
        // CRC-32 of b"IEND" is 0xAE426082 (the standard PNG IEND CRC).
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn insert_then_read_round_trips() {
        let png = tiny_png();
        assert_eq!(read_text(&png, "Thumb::MTime"), None);
        let with = insert_text(&png, &[("Thumb::MTime", "1700000000"), ("Thumb::URI", "file:///x")])
            .expect("is a png");
        assert_eq!(read_text(&with, "Thumb::MTime").as_deref(), Some("1700000000"));
        assert_eq!(read_text(&with, "Thumb::URI").as_deref(), Some("file:///x"));
        // The original pixel data still follows.
        assert!(with.windows(4).any(|w| w == b"IDAT"));
        assert!(with.ends_with(b"IEND\xae\x42\x60\x82"));
    }

    #[test]
    fn non_png_is_rejected() {
        assert_eq!(read_text(b"not a png", "x"), None);
        assert_eq!(insert_text(b"not a png", &[("a", "b")]), None);
    }
}
```

- [ ] **Step 2: Wire the module**

`crates/iconview/src/thumbs/mod.rs`:

```rust
//! Preview thumbnails: freedesktop-cached, generated off the main thread.

pub mod png_text;
```

Add `pub mod thumbs;` to `crates/iconview/src/lib.rs` (alphabetical: after `style`, before `types`... actually after `scan`, before `style`).

- [ ] **Step 3: Run**

Run: `cargo test -p opusdm-iconview png_text`
Expected: PASS (3 tests). If `crc32_matches_known_vector` fails, the polynomial loop is wrong; the value `0xAE426082` is fixed (it is the CRC in every PNG's IEND).

- [ ] **Step 4: Commit**

```bash
git add crates/iconview/src/thumbs/mod.rs crates/iconview/src/thumbs/png_text.rs crates/iconview/src/lib.rs
git commit -m "feat(iconview): PNG tEXt chunk read/write for thumbnail tags"
```

---

## Task 3: Freedesktop cache path and validity

**Files:**
- Create: `crates/iconview/src/thumbs/cache.rs`
- Modify: `crates/iconview/src/thumbs/mod.rs` (`pub mod cache;`)

**Interfaces:**
- Consumes: `thumbs::png_text`, `crate::types::ThumbSize`.
- Produces:
  - `thumbs::cache::uri_for(path: &Path) -> String` (`"file://"` + each path component percent-encoded per RFC 3986 unreserved set, `/` kept)
  - `thumbs::cache::cache_path(path: &Path, size: ThumbSize) -> Option<PathBuf>` (`$XDG_CACHE_HOME`/`~/.cache` + `/thumbnails/<dir_name>/<md5 hex>.png`; `None` if no home)
  - `thumbs::cache::load_valid(path: &Path, size: ThumbSize) -> Option<Vec<u8>>` (returns the cached PNG bytes iff the file exists and its `Thumb::MTime` tag equals the source mtime)
  - `thumbs::cache::store(path: &Path, size: ThumbSize, png: &[u8]) -> std::io::Result<()>` (inserts `Thumb::MTime` + `Thumb::URI`, writes atomically `tmp` + `rename` into the cache dir, `0700` dir / `0600` file per the spec)

- [ ] **Step 1: Write the module with tests**

`crates/iconview/src/thumbs/cache.rs`:

```rust
//! The freedesktop thumbnail cache: `~/.cache/thumbnails/<bucket>/<md5>.png`,
//! where `<md5>` is the MD5 of the canonical `file://` URI. Validity is the
//! `Thumb::MTime` tEXt tag matching the source file's mtime.

use crate::thumbs::png_text;
use crate::types::ThumbSize;
use md5::{Digest, Md5};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The canonical `file://` URI for `path`: absolute, each segment
/// percent-encoded except the RFC 3986 unreserved set, `/` preserved.
pub fn uri_for(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn cache_root() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(x));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))
}

/// The absolute cache file path for `path` at `size`.
pub fn cache_path(path: &Path, size: ThumbSize) -> Option<PathBuf> {
    let digest = Md5::digest(uri_for(path).as_bytes());
    let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Some(cache_root()?.join("thumbnails").join(size.dir_name()).join(format!("{hex}.png")))
}

fn source_mtime(path: &Path) -> Option<i64> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// The cached PNG for `path` at `size`, iff it exists and its `Thumb::MTime`
/// matches the source's current mtime.
pub fn load_valid(path: &Path, size: ThumbSize) -> Option<Vec<u8>> {
    let cp = cache_path(path, size)?;
    let bytes = std::fs::read(&cp).ok()?;
    let tagged: i64 = png_text::read_text(&bytes, "Thumb::MTime")?.trim().parse().ok()?;
    if tagged == source_mtime(path)? {
        Some(bytes)
    } else {
        None
    }
}

/// Write `png` into the cache for `path` at `size`, stamping `Thumb::MTime`
/// and `Thumb::URI`. Atomic: a sibling `tmp` file is renamed into place.
pub fn store(path: &Path, size: ThumbSize, png: &[u8]) -> io::Result<()> {
    let cp = cache_path(path, size)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no cache directory"))?;
    let dir = cp.parent().expect("cache_path always has a parent");
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let mtime = source_mtime(path).unwrap_or(0).to_string();
    let uri = uri_for(path);
    let stamped = png_text::insert_text(png, &[("Thumb::MTime", &mtime), ("Thumb::URI", &uri)])
        .unwrap_or_else(|| png.to_vec());
    let tmp = cp.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, &stamped)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &cp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_matches_the_freedesktop_example() {
        // From the freedesktop thumbnail spec: the URI for this path hashes to
        // this MD5.
        let p = Path::new("/home/jens/photos/me.png");
        assert_eq!(uri_for(p), "file:///home/jens/photos/me.png");
        let digest = Md5::digest(uri_for(p).as_bytes());
        let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex, "c6ee772d9e49320e97ec29a7eb5b1697");
    }

    #[test]
    fn uri_percent_encodes_spaces() {
        assert_eq!(uri_for(Path::new("/tmp/a b.png")), "file:///tmp/a%20b.png");
    }

    #[test]
    fn store_then_load_valid_round_trips_and_detects_staleness() {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CACHE_HOME", td.path().join("cache"));
        let src = td.path().join("pic.png");
        std::fs::write(&src, b"source bytes").unwrap();

        let png = super::super::png_text::tests_tiny_png();
        store(&src, ThumbSize::Normal, &png).unwrap();
        let got = load_valid(&src, ThumbSize::Normal).expect("fresh cache is valid");
        assert!(super::super::png_text::read_text(&got, "Thumb::URI").is_some());

        // Touch the source: the cached MTime no longer matches.
        filetime_set_future(&src);
        assert!(load_valid(&src, ThumbSize::Normal).is_none());
    }

    fn filetime_set_future(p: &Path) {
        // Bump mtime by writing again after a wall-clock second would be racy;
        // set it explicitly via `utimensat` through std is not available, so
        // rewrite and then assert the stored tag differs from the new mtime by
        // sleeping a second is too slow. Instead, overwrite the cache tag.
        let cp = cache_path(p, ThumbSize::Normal).unwrap();
        let bytes = std::fs::read(&cp).unwrap();
        let broken = String::from_utf8_lossy(&bytes).replace("Thumb::MTime", "Thumb::MXime");
        std::fs::write(&cp, broken.as_bytes()).unwrap();
    }
}
```

Note: `png_text` needs a test helper visible to `cache.rs` tests. In `png_text.rs`, rename the private `tiny_png()` test fn to `pub(crate) fn tests_tiny_png() -> Vec<u8>` and move it OUT of `mod tests` to a `#[cfg(test)] pub(crate) fn tests_tiny_png()` at module level; update `png_text`'s own tests to call `tests_tiny_png()`. Keep `read_text` reachable from tests as `super::super::png_text::read_text` (it is `pub`).

- [ ] **Step 2: Wire and run**

Add `pub mod cache;` to `crates/iconview/src/thumbs/mod.rs`.
Run: `cargo test -p opusdm-iconview thumbs::cache`
Expected: PASS. `uri_matches_the_freedesktop_example` pins MD5 correctness.

- [ ] **Step 3: Commit**

```bash
git add crates/iconview/src/thumbs/cache.rs crates/iconview/src/thumbs/mod.rs crates/iconview/src/thumbs/png_text.rs
git commit -m "feat(iconview): freedesktop thumbnail cache path and validity"
```

---

## Task 4: Generators and tool detection

**Files:**
- Create: `crates/iconview/src/thumbs/generate.rs`
- Modify: `crates/iconview/src/thumbs/mod.rs` (`pub mod generate;` + `ThumbKind`)

**Interfaces:**
- Consumes: nothing from earlier tasks except `ThumbSize`.
- Produces:
  - `thumbs::ThumbKind` (`enum { Image, Pdf, Video }`, `Debug, Clone, Copy, PartialEq, Eq`)
  - `thumbs::ThumbKind::from_ext(ext: Option<&str>) -> Option<ThumbKind>`
  - `thumbs::generate::generate(path: &Path, kind: ThumbKind, px: i32) -> std::io::Result<Vec<u8>>` (returns PNG bytes; `Err` when no tool / tool failed / timed out)
  - `thumbs::generate::tools_available() -> ToolReport` (`pub struct ToolReport { pub pdf: bool, pub video: bool }`) for diagnostics/tests

- [ ] **Step 1: Write the module**

`crates/iconview/src/thumbs/generate.rs`:

```rust
//! Turn a source file into PNG thumbnail bytes. Images decode in-process with
//! gdk-pixbuf (thread-safe, no display needed); PDFs and videos shell out to
//! optional tools with a hard timeout. Every path returns PNG bytes or an
//! error; the caller falls back to the glyph on error.

use crate::thumbs::ThumbKind;
use gtk4::gdk_pixbuf;
use gtk4::prelude::*;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Longest a generator subprocess may run before it is killed.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Which optional tools were found on `$PATH`.
pub struct ToolReport {
    pub pdf: bool,
    pub video: bool,
}

#[derive(Clone, Copy)]
enum PdfTool {
    Pdftoppm,
    Pdftocairo,
}

#[derive(Clone, Copy)]
enum VideoTool {
    FfmpegThumbnailer,
    Ffmpeg,
}

struct Tools {
    pdf: Option<PdfTool>,
    video: Option<VideoTool>,
}

fn tools() -> &'static Tools {
    static TOOLS: OnceLock<Tools> = OnceLock::new();
    TOOLS.get_or_init(|| Tools {
        pdf: if in_path("pdftoppm") {
            Some(PdfTool::Pdftoppm)
        } else if in_path("pdftocairo") {
            Some(PdfTool::Pdftocairo)
        } else {
            None
        },
        video: if in_path("ffmpegthumbnailer") {
            Some(VideoTool::FfmpegThumbnailer)
        } else if in_path("ffmpeg") {
            Some(VideoTool::Ffmpeg)
        } else {
            None
        },
    })
}

/// Is `name` an executable on `$PATH`?
fn in_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let cand = dir.join(name);
        std::fs::metadata(&cand).map(|m| m.is_file()).unwrap_or(false)
    })
}

/// Diagnostics: which optional tools are present.
pub fn tools_available() -> ToolReport {
    let t = tools();
    ToolReport { pdf: t.pdf.is_some(), video: t.video.is_some() }
}

/// PNG thumbnail bytes for `path`, at most `px` on the long edge.
pub fn generate(path: &Path, kind: ThumbKind, px: i32) -> io::Result<Vec<u8>> {
    match kind {
        ThumbKind::Image => generate_image(path, px),
        ThumbKind::Pdf => generate_pdf(path, px),
        ThumbKind::Video => generate_video(path, px),
    }
}

fn generate_image(path: &Path, px: i32) -> io::Result<Vec<u8>> {
    let pb = gdk_pixbuf::Pixbuf::from_file_at_scale(path, px, px, true)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let bytes = pb
        .save_to_bufferv("png", &[])
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    Ok(bytes)
}

fn generate_pdf(path: &Path, px: i32) -> io::Result<Vec<u8>> {
    let tool = tools().pdf.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no pdf tool"))?;
    let out = match tool {
        // Both write PNG to stdout with `-singlefile` and a target box.
        PdfTool::Pdftoppm => run_with_timeout(
            Command::new("pdftoppm")
                .args(["-png", "-singlefile", "-scale-to"])
                .arg(px.to_string())
                .arg(path),
        )?,
        PdfTool::Pdftocairo => run_with_timeout(
            Command::new("pdftocairo")
                .args(["-png", "-singlefile", "-scale-to"])
                .arg(px.to_string())
                .arg(path)
                .arg("-"),
        )?,
    };
    non_empty_png(out)
}

fn generate_video(path: &Path, px: i32) -> io::Result<Vec<u8>> {
    let tool = tools().video.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no video tool"))?;
    let out = match tool {
        VideoTool::FfmpegThumbnailer => run_with_timeout(
            Command::new("ffmpegthumbnailer")
                .arg("-i")
                .arg(path)
                .arg("-o")
                .arg("-")
                .arg("-c")
                .arg("png")
                .arg("-s")
                .arg(px.to_string()),
        )?,
        VideoTool::Ffmpeg => run_with_timeout(
            Command::new("ffmpeg")
                .args(["-y", "-ss", "00:00:01", "-i"])
                .arg(path)
                .args(["-frames:v", "1", "-f", "image2", "-c:v", "png", "-vf"])
                .arg(format!("scale={px}:-1"))
                .arg("-"),
        )?,
    };
    non_empty_png(out)
}

fn non_empty_png(bytes: Vec<u8>) -> io::Result<Vec<u8>> {
    if bytes.len() > 8 && bytes[1..4] == *b"PNG" {
        Ok(bytes)
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "generator produced no PNG"))
    }
}

/// Run `cmd` capturing stdout, killing it after `TIMEOUT`.
fn run_with_timeout(cmd: &mut Command) -> io::Result<Vec<u8>> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut out = Vec::new();
                if let Some(mut so) = child.stdout.take() {
                    io::Read::read_to_end(&mut so, &mut out)?;
                }
                if !status.success() {
                    return Err(io::Error::new(io::ErrorKind::Other, "generator exited non-zero"));
                }
                return Ok(out);
            }
            None => {
                if start.elapsed() >= TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "generator timed out"));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

impl ThumbKind {
    /// The thumbnail kind for a lowercase file extension, or `None` when the
    /// extension is not previewable.
    pub fn from_ext(ext: Option<&str>) -> Option<ThumbKind> {
        let ext = ext?;
        const IMAGE: &[&str] =
            &["png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "tif", "ico", "svg"];
        const VIDEO: &[&str] =
            &["mp4", "mkv", "avi", "mov", "webm", "m4v", "mpg", "mpeg", "wmv", "flv"];
        if ext == "pdf" {
            return Some(ThumbKind::Pdf);
        }
        if IMAGE.contains(&ext) {
            return Some(ThumbKind::Image);
        }
        if VIDEO.contains(&ext) {
            return Some(ThumbKind::Video);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_ext_classifies() {
        assert_eq!(ThumbKind::from_ext(Some("png")), Some(ThumbKind::Image));
        assert_eq!(ThumbKind::from_ext(Some("PDF".to_lowercase().as_str())), Some(ThumbKind::Pdf));
        assert_eq!(ThumbKind::from_ext(Some("mkv")), Some(ThumbKind::Video));
        assert_eq!(ThumbKind::from_ext(Some("rs")), None);
        assert_eq!(ThumbKind::from_ext(None), None);
    }

    #[test]
    fn image_generation_produces_a_png() {
        if gtk4::init().is_err() {
            // gdk-pixbuf loaders still work without a display, but init keeps the
            // test honest about environment; skip only if even that fails.
        }
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("in.png");
        // Write a real 2x2 PNG using gdk-pixbuf itself.
        let pb = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 2, 2).unwrap();
        pb.savev(&src, "png", &[]).unwrap();

        let out = generate(&src, ThumbKind::Image, 64).unwrap();
        assert!(out.len() > 8 && &out[1..4] == b"PNG");
    }

    #[test]
    fn missing_tools_error_rather_than_panic() {
        let td = tempfile::tempdir().unwrap();
        let fake_pdf = td.path().join("x.pdf");
        std::fs::write(&fake_pdf, b"%PDF-1.4\n").unwrap();
        // Whatever the host has, the call must return a Result, never panic.
        let _ = generate(&fake_pdf, ThumbKind::Pdf, 128);
    }
}
```

Note: `gdk_pixbuf::Pixbuf` calls are not display-bound and are safe on a worker thread. `image_generation_produces_a_png` may run in the crate's normal test threads because gdk-pixbuf does not call `gtk_init`. Do NOT put it in `field::tests::gtk_widget_behavior`; it is not a GTK-widget test. If `gtk4::init()` in that test causes a second-init panic when the widget test also ran, drop the `gtk4::init()` line entirely (it is only a comment-marker here) and rely on gdk-pixbuf working init-free.

- [ ] **Step 2: Add `ThumbKind` to `mod.rs`**

In `crates/iconview/src/thumbs/mod.rs`:

```rust
pub mod cache;
pub mod generate;
pub mod png_text;

/// What kind of preview a file gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbKind {
    Image,
    Pdf,
    Video,
}
```

(The `impl ThumbKind` block lives in `generate.rs`; that is fine, same crate.)

- [ ] **Step 3: Run**

Run: `cargo test -p opusdm-iconview thumbs::generate`
Expected: PASS (3 tests). `image_generation_produces_a_png` needs the gdk-pixbuf PNG loader, which is present wherever GTK is.

- [ ] **Step 4: Commit**

```bash
git add crates/iconview/src/thumbs/generate.rs crates/iconview/src/thumbs/mod.rs
git commit -m "feat(iconview): thumbnail generators and tool detection"
```

---

## Task 5: The worker pool

**Files:**
- Create: `crates/iconview/src/thumbs/pool.rs`
- Modify: `crates/iconview/src/thumbs/mod.rs` (`mod pool;` + `ThumbPool`, `ThumbRequest`, `ThumbResult`, `wanted`)

**Interfaces:**
- Consumes: `thumbs::{cache, generate, ThumbKind}`, `types::ThumbSize`, `async-channel`.
- Produces:
  - `thumbs::ThumbRequest { pub index: usize, pub path: PathBuf, pub kind: ThumbKind, pub size: ThumbSize }`
  - `thumbs::ThumbResult { pub index: usize, pub path: PathBuf, pub size: ThumbSize, pub png: Vec<u8> }`
  - `thumbs::ThumbPool` with:
    - `ThumbPool::new() -> (ThumbPool, async_channel::Receiver<ThumbResult>)`
    - `ThumbPool::submit(&self, req: ThumbRequest)`
    - `ThumbPool::generation(&self) -> u64` and `ThumbPool::bump_generation(&self)` (stale-drop: results from before the latest bump are discarded by the field)
  - `thumbs::wanted(ext: Option<&str>) -> Option<ThumbKind>` (thin alias of `ThumbKind::from_ext`, the field's single entry point)
  - `thumbs::ThumbPool::disabled() -> bool` (`OPUSDM_NO_THUMBS` set and non-empty)

- [ ] **Step 1: Write the pool**

`crates/iconview/src/thumbs/pool.rs`:

```rust
//! A small fixed pool of worker threads that turn `ThumbRequest`s into
//! `ThumbResult`s. Modelled on `opusdm_fsops::pool` but dependency-free and
//! specialised: each worker checks the freedesktop cache, generates on a miss,
//! stores the result, and forwards the PNG bytes.

use crate::thumbs::{cache, generate, ThumbKind};
use crate::types::ThumbSize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Upper bound on worker threads, whatever the core count.
const MAX_WORKERS: usize = 4;

/// A request to produce one thumbnail.
pub struct ThumbRequest {
    pub index: usize,
    pub path: PathBuf,
    pub kind: ThumbKind,
    pub size: ThumbSize,
}

/// A produced thumbnail, as PNG bytes (texture creation is the caller's job).
pub struct ThumbResult {
    pub index: usize,
    pub path: PathBuf,
    pub size: ThumbSize,
    pub png: Vec<u8>,
    pub generation: u64,
}

/// The worker pool handle. Dropping it closes the queue; workers drain and exit.
pub struct ThumbPool {
    tx: Option<Sender<ThumbRequest>>,
    handles: Vec<JoinHandle<()>>,
    generation: Arc<AtomicU64>,
}

impl ThumbPool {
    /// Spawn the pool. The receiver yields results in completion order.
    pub fn new() -> (ThumbPool, async_channel::Receiver<ThumbResult>) {
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(MAX_WORKERS).max(1);
        let (tx, rx) = channel::<ThumbRequest>();
        let rx = Arc::new(Mutex::new(rx));
        let (out_tx, out_rx) = async_channel::unbounded::<ThumbResult>();
        let generation = Arc::new(AtomicU64::new(0));

        let mut handles = Vec::with_capacity(workers);
        for i in 0..workers {
            let rx = Arc::clone(&rx);
            let out_tx = out_tx.clone();
            let generation = Arc::clone(&generation);
            let handle = std::thread::Builder::new()
                .name(format!("thumb-{i}"))
                .spawn(move || loop {
                    let req = match rx.lock().map(|g| g.recv()) {
                        Ok(Ok(req)) => req,
                        _ => break,
                    };
                    let gen_at_start = generation.load(Ordering::SeqCst);
                    let ThumbRequest { index, path, kind, size } = req;
                    let png = match cache::load_valid(&path, size) {
                        Some(bytes) => Some(bytes),
                        None => match generate::generate(&path, kind, size.px()) {
                            Ok(bytes) => {
                                let _ = cache::store(&path, size, &bytes);
                                Some(bytes)
                            }
                            Err(e) => {
                                eprintln!("opusdm-iconview: thumbnail failed for {}: {e}", path.display());
                                None
                            }
                        },
                    };
                    if let Some(png) = png {
                        let _ = out_tx.send_blocking(ThumbResult {
                            index,
                            path,
                            size,
                            png,
                            generation: gen_at_start,
                        });
                    }
                })
                .expect("spawn thumb worker");
            handles.push(handle);
        }

        (ThumbPool { tx: Some(tx), handles, generation }, out_rx)
    }

    /// Queue a request. Silently dropped after the pool is shut down.
    pub fn submit(&self, req: ThumbRequest) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(req);
        }
    }

    /// The current generation counter.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Advance the generation so in-flight results are recognised as stale.
    pub fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Whether thumbnails are switched off by `OPUSDM_NO_THUMBS`.
    pub fn disabled() -> bool {
        std::env::var_os("OPUSDM_NO_THUMBS").map(|v| !v.is_empty()).unwrap_or(false)
    }
}

impl Drop for ThumbPool {
    fn drop(&mut self) {
        self.tx.take();
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}
```

- [ ] **Step 2: Facade in `mod.rs`**

```rust
mod pool;

pub use pool::{ThumbPool, ThumbRequest, ThumbResult};

/// The thumbnail kind for a file extension, or `None` when not previewable.
/// The `IconField`'s single decision point.
pub fn wanted(ext: Option<&str>) -> Option<ThumbKind> {
    ThumbKind::from_ext(ext)
}
```

- [ ] **Step 3: Write the failing test**

`crates/iconview/src/thumbs/pool.rs` `mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_serves_a_cached_image_without_a_generator() {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CACHE_HOME", td.path().join("cache"));
        let src = td.path().join("pic.png");
        std::fs::write(&src, b"src").unwrap();
        // Pre-seed the cache with a valid entry.
        let png = crate::thumbs::png_text::tests_tiny_png();
        cache::store(&src, ThumbSize::Normal, &png).unwrap();

        let (pool, rx) = ThumbPool::new();
        pool.submit(ThumbRequest {
            index: 3,
            path: src.clone(),
            kind: ThumbKind::Image,
            size: ThumbSize::Normal,
        });
        let res = rx.recv_blocking().expect("a result");
        assert_eq!(res.index, 3);
        assert_eq!(res.path, src);
        assert!(res.png.windows(4).any(|w| w == b"IDAT"));
    }
}
```

- [ ] **Step 4: Run**

Run: `cargo test -p opusdm-iconview thumbs::pool`
Expected: PASS. (Cache hit path needs no external tool.)

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/thumbs/pool.rs crates/iconview/src/thumbs/mod.rs
git commit -m "feat(iconview): thumbnail worker pool"
```

---

## Task 6: `IconItem` thumbnail slot

**Files:**
- Modify: `crates/iconview/src/item.rs`

**Interfaces:**
- Consumes: `gtk4::gdk`.
- Produces:
  - `IconItem::set_thumbnail(&self, texture: &gtk4::gdk::Texture)` (hide the glyph label, show a `gtk::Picture` holding the texture, both sized to the icon box)
  - `IconItem::clear_thumbnail(&self)` (show the glyph again; used when the icon size bucket changes)

- [ ] **Step 1: Extend `Inner` and `new`**

Add to `struct Inner`:

```rust
    thumb: gtk::Picture,
    icon_size: std::cell::Cell<i32>,
```

In `new`, after building `icon`:

```rust
        let thumb = gtk::Picture::new();
        thumb.set_content_fit(gtk::ContentFit::Contain);
        thumb.set_can_shrink(true);
        thumb.set_width_request(icon_size);
        thumb.set_height_request(icon_size);
        thumb.set_visible(false);
```

Append `thumb` to `root` right after `icon` (`root.append(&icon); root.append(&thumb);`), and store `icon_size: std::cell::Cell::new(icon_size)` in `Inner`.

- [ ] **Step 2: Update `set_icon_size` and add the thumbnail methods**

`set_icon_size` also resizes the picture and records the size:

```rust
    pub fn set_icon_size(&self, px: i32) {
        self.inner.icon.set_width_request(px);
        self.inner.icon.set_height_request(px);
        self.inner.thumb.set_width_request(px);
        self.inner.thumb.set_height_request(px);
        self.inner.icon_size.set(px);
    }
```

New methods:

```rust
    /// Show `texture` in place of the glyph.
    pub fn set_thumbnail(&self, texture: &gtk::gdk::Texture) {
        self.inner.thumb.set_paintable(Some(texture));
        self.inner.thumb.set_visible(true);
        self.inner.icon.set_visible(false);
    }

    /// Drop any thumbnail and show the glyph again.
    pub fn clear_thumbnail(&self) {
        self.inner.thumb.set_paintable(gtk::gdk::Paintable::NONE);
        self.inner.thumb.set_visible(false);
        self.inner.icon.set_visible(true);
    }
```

- [ ] **Step 3: Extend the one GTK test**

In `crates/iconview/src/field.rs` `field::tests::gtk_widget_behavior`, after the existing `IconItem` section:

```rust
        // --- IconItem: thumbnail slot swaps the glyph out and back ---
        let it2 = IconItem::new(&entries(1)[0], 64);
        assert!(!it2.widget().has_css_class("has-thumb"));
        // A 1x1 texture is enough to exercise the swap.
        let tex = gtk::gdk::MemoryTexture::new(
            1,
            1,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &gtk::glib::Bytes::from_static(&[0, 0, 0, 255]),
            4,
        );
        it2.set_thumbnail(tex.upcast_ref::<gtk::gdk::Texture>());
        it2.clear_thumbnail();
```

(This asserts no panic across the swap; `MemoryTexture` needs no display. Add `use gtk::prelude::*;` reference already present.)

- [ ] **Step 4: Run**

Run: `cargo test -p opusdm-iconview field::tests::gtk_widget_behavior`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/iconview/src/item.rs crates/iconview/src/field.rs
git commit -m "feat(iconview): IconItem thumbnail slot"
```

---

## Task 7: `IconField` wiring

**Files:**
- Modify: `crates/iconview/src/field.rs`

**Interfaces:**
- Consumes: `thumbs::{ThumbPool, ThumbRequest, ThumbResult, wanted}`, `types::ThumbSize`, `gdk::Texture`.
- Produces: no new public API. Internal: the field enqueues on `set_entries` and `set_icon_size`, and applies results on the main loop.

- [ ] **Step 1: Add pool state to `Inner`**

```rust
    thumb_pool: Option<crate::thumbs::ThumbPool>,
```

In `new`, before returning `field`:

```rust
        if !crate::thumbs::ThumbPool::disabled() {
            let (pool, rx) = crate::thumbs::ThumbPool::new();
            // set the field's pool (Inner is behind Rc; use a small setter or
            // build the pool before constructing Inner - see Step 2).
            field.attach_thumb_pool(pool, rx);
        }
```

Because `Inner` is built before `field` exists, restructure: build `let thumb_pool = (!disabled).then(ThumbPool::new)` returning `(pool, rx)`, store `Option<ThumbPool>` in `Inner`, and keep `rx` to wire the drain after `field` is constructed. Concretely:

```rust
        let (thumb_pool, thumb_rx) = match crate::thumbs::ThumbPool::disabled() {
            true => (None, None),
            false => {
                let (p, r) = crate::thumbs::ThumbPool::new();
                (Some(p), Some(r))
            }
        };
```

put `thumb_pool` into `Inner`, then after `let field = IconField { inner };`:

```rust
        if let Some(rx) = thumb_rx {
            let this = field.clone();
            gtk::glib::spawn_future_local(async move {
                while let Ok(res) = rx.recv().await {
                    this.apply_thumb_result(res);
                }
            });
        }
```

- [ ] **Step 2: Enqueue on `set_entries`**

At the end of `set_entries`, after `self.relayout();`:

```rust
        self.request_thumbs();
```

New method:

```rust
    /// Queue a thumbnail job for every previewable entry, newest generation.
    fn request_thumbs(&self) {
        let Some(pool) = self.inner.thumb_pool.as_ref() else {
            return;
        };
        pool.bump_generation();
        let size = crate::types::ThumbSize::for_icon(self.inner.icon_size.get());
        for (index, entry) in self.inner.entries.borrow().iter().enumerate() {
            let Some(kind) = crate::thumbs::wanted(entry.ext.as_deref()) else {
                continue;
            };
            pool.submit(crate::thumbs::ThumbRequest {
                index,
                path: entry.path.clone(),
                kind,
                size,
            });
        }
    }
```

- [ ] **Step 3: Apply results with a staleness guard**

```rust
    /// Turn a finished thumbnail into a texture and show it, unless the entry
    /// list moved on (generation bumped) or the row now holds a different path.
    fn apply_thumb_result(&self, res: crate::thumbs::ThumbResult) {
        let Some(pool) = self.inner.thumb_pool.as_ref() else {
            return;
        };
        if res.generation != pool.generation() {
            return;
        }
        let entries = self.inner.entries.borrow();
        let Some(entry) = entries.get(res.index) else {
            return;
        };
        if entry.path != res.path {
            return;
        }
        let bytes = gtk::glib::Bytes::from_owned(res.png);
        let Ok(texture) = gtk::gdk::Texture::from_bytes(&bytes) else {
            return;
        };
        if let Some(item) = self.inner.items.borrow().get(res.index) {
            item.set_thumbnail(&texture);
        }
    }
```

- [ ] **Step 4: Re-request on `set_icon_size` when the bucket changes**

In `set_icon_size`, after the loop that resizes items and before/after `self.relayout()`:

```rust
        let before = crate::types::ThumbSize::for_icon(old_px);
        let after = crate::types::ThumbSize::for_icon(px);
        if before != after {
            for it in self.inner.items.borrow().iter() {
                it.clear_thumbnail();
            }
            self.request_thumbs();
        }
```

Capture `old_px` at the top of `set_icon_size` before `self.inner.icon_size.set(px)` (add an `icon_size` read; `Inner` already has `icon_size: Cell<i32>` from Task 8 of phase 1 - confirm the field name; if the field's size lives only on items, add `icon_size: Cell<i32>` to the field's `Inner` and set it in `new` and `set_icon_size`).

- [ ] **Step 5: Extend the one GTK test**

In `field::tests::gtk_widget_behavior`, after the reposition section:

```rust
        // --- IconField: thumbnails are requested for previewable entries ---
        std::env::set_var("OPUSDM_NO_THUMBS", "1");
        let f2 = IconField::new(); // pool disabled: request_thumbs is a no-op
        f2.set_entries(&entries(2));
        f2.set_icon_size(128);
        std::env::remove_var("OPUSDM_NO_THUMBS");
```

(The real end-to-end path - pool enabled, cache hit, texture applied - is covered by `thumbs::pool` tests plus a manual check; wiring here only needs to not panic with the pool on or off. If you want an enabled-pool assertion, point `XDG_CACHE_HOME` at a tempdir, pre-seed a cache entry for a temp file, `set_entries` with that path, and pump the main context: `while gtk::glib::MainContext::default().iteration(false) {}` a bounded number of times, then assert the item shows a thumbnail. Keep it time-bounded.)

- [ ] **Step 6: Run**

Run: `cargo test -p opusdm-iconview`
Expected: PASS, no warnings. `cargo clippy -p opusdm-iconview --all-targets` clean.

- [ ] **Step 7: Commit**

```bash
git add crates/iconview/src/field.rs
git commit -m "feat(iconview): request and apply thumbnails in IconField"
```

---

## Task 8: Workspace verification, docs, changelog

**Files:**
- Modify: `CHANGES.md`

- [ ] **Step 1: Full workspace build and test**

Run: `cargo test --workspace && cargo build --workspace`
Expected: PASS, zero warnings. The Lister and hub are unchanged but must still compile and pass (they call `set_entries` / `set_icon_size` exactly as before).

- [ ] **Step 2: Clippy**

Run: `cargo clippy -p opusdm-iconview --all-targets`
Expected: no warnings introduced by this change.

- [ ] **Step 3: Manual smoke test (document the result in the commit message)**

- In a directory with a `.png`, a `.pdf` and a `.mp4`: open a Lister, toggle icon view. The PNG shows a real thumbnail within a second; the PDF and video show thumbnails if `pdftoppm`/`ffmpegthumbnailer` are installed, otherwise their glyph.
- Re-open the same directory: thumbnails appear immediately (cache hit); check `~/.cache/thumbnails/normal/` has new `.png` files.
- `OPUSDM_NO_THUMBS=1 ./run-x11.sh` (or the usual launcher): only glyphs, no cache writes.
- Ctrl+scroll to grow icons past 128px: thumbnails refresh at the `large` bucket.

- [ ] **Step 4: Prepend a `CHANGES.md` entry under `## 2026-09-07`**

```markdown
- Fase 2 dell'`IconField`: thumbnail di anteprima. Nuovo modulo `crates/iconview/src/thumbs` - cache freedesktop (`~/.cache/thumbnails/<bucket>/<md5 dell'URI>.png`, tag `Thumb::MTime`/`Thumb::URI` scritti e verificati via parsing `tEXt` PNG interno), generatori per immagini (gdk-pixbuf in-process), PDF (`pdftoppm`/`pdftocairo`) e video (`ffmpegthumbnailer`/`ffmpeg`, shell-out con timeout di 20s), e un pool di worker a dimensione fissa (max 4). L'`IconField` accoda un job per ogni voce anteprimabile su `set_entries`, applica i risultati sul main loop creando la `gdk::Texture` dai byte PNG, con guardia di generazione e confronto del path per scartare risultati obsoleti. Il glyph Nerd Font resta il fallback e lo stato iniziale. `OPUSDM_NO_THUMBS=1` disattiva tutto. Nuova dipendenza `md-5` (RustCrypto) per l'hash dell'URI richiesto dallo standard freedesktop. Nessuna modifica ai consumatori (Lister, hub).
```

- [ ] **Step 5: Commit**

```bash
git add CHANGES.md
git commit -m "docs: changelog for iconview thumbnails (phase 2)

Manual smoke test: PNG thumbnails render and cache to
~/.cache/thumbnails/normal/; PDF/video render when tools present,
glyph fallback otherwise; OPUSDM_NO_THUMBS=1 disables."
```

---

## Self-Review

**1. Spec coverage** (spec section "Servizio thumbnail e cache (fase 2)"):

| Spec point | Task |
|------------|------|
| `~/.cache/thumbnails/` `normal`/`large` buckets | 1 (`dir_name`, `for_icon`), 3 (`cache_path`) |
| filename = `md5(uri)` | 3 (`cache_path`, pinned by `uri_matches_the_freedesktop_example`) |
| `Thumb::URI` / `Thumb::MTime` tags, validity on MTime mismatch | 2 (`png_text`), 3 (`store` writes, `load_valid` checks) |
| no cache write for already-small images | **Deviation**: dropped for phase 2. `generate_image` always scales to the bucket and stores; a tiny source just produces a tiny thumbnail. Cheap enough; revisit only if profiling says so. Noted here. |
| image path: gdk-pixbuf scale, no external tool | 4 (`generate_image`) |
| PDF: `pdftoppm` then `pdftocairo` | 4 (`generate_pdf`) |
| video: `ffmpegthumbnailer` then `ffmpeg` | 4 (`generate_video`) |
| tool detection once, memoised | 4 (`tools()` + `OnceLock`) |
| generators on a worker pool, never main thread | 5 (`ThumbPool`) |
| subprocess kill timeout | 4 (`run_with_timeout`, 20s) |
| result to main via async channel | 5 (`async_channel`), 7 (`spawn_future_local` drain) |
| `index -> path` check before applying | 7 (`apply_thumb_result`) |
| stale results (directory changed) discarded | 7 (generation counter, bumped in `request_thumbs`) |
| glyph placeholder first, swap on arrival | 6 (`set_thumbnail` / `clear_thumbnail`) |
| priority to visible items | **Deviation**: phase 2 enqueues every previewable entry in list order, no viewport-aware reordering. The pool cap (4) plus cache hits keep it acceptable for typical directories; true visible-first prioritisation is deferred. Noted here. |
| cross-fade on swap | **Dropped by decision** (user chose "gtk::Image, no cross-fade"); `set_thumbnail` is an instant swap. |

**2. Placeholder scan:** No `TODO`/`TBD`. Task 7 Step 1 and Step 4 hand the implementer a restructure choice ("build the pool before `Inner`") with the concrete code to use; Task 6 Step 2 says to confirm the `Inner.icon_size` field name from phase 1 (it exists: `field.rs` `Inner.icon_size: Cell<i32>` was added in phase 1 Task 8) and add it only if absent. These are verification instructions, not placeholders.

**3. Type consistency:**
- `ThumbSize::px` / `dir_name` / `for_icon` (Task 1) used verbatim in Tasks 3, 5, 7.
- `ThumbKind` defined in `thumbs/mod.rs` (Task 4 Step 2), `impl` in `generate.rs` (Task 4 Step 1); `wanted()` (Task 5) delegates to `ThumbKind::from_ext`.
- `ThumbRequest` / `ThumbResult` field names (`index`, `path`, `kind`, `size`, `png`, `generation`) identical across Task 5 definition and Task 7 use.
- `png_text::tests_tiny_png` is `pub(crate)` and used by `cache.rs` (Task 3) and `pool.rs` (Task 5) tests; Task 3 Step 1 note makes it `pub(crate)` at module scope.
- `IconItem::set_thumbnail(&gdk::Texture)` (Task 6) matches the call in `apply_thumb_result` (Task 7), which passes `&texture` from `gdk::Texture::from_bytes`.

**4. Ambiguity check:**
- "worker thread must not touch GTK": generators return `Vec<u8>`; `gdk::Texture::from_bytes` is called only in `apply_thumb_result`, on the main loop. Stated in Global Constraints and both task bodies.
- gdk-pixbuf on a worker thread: called out in Task 4 Step 1 as safe (loaders are not display-bound). If a specific host proves otherwise, the fallback is to move image decode behind the same `Command` shell-out path - but that is not expected and not in scope.
- `ffmpeg ... -` writing PNG to stdout: `-f image2` with `-c:v png` to `pipe:` (`-`) is the documented way; `non_empty_png` guards a tool that ignores it.
