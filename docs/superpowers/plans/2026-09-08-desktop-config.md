# opus-desktop-config Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a standalone `opus-desktop-config` app plus a configurable desktop background (solid / gradient / image), an adjustable icon grid, an overridable desktop directory, and a per-folder `.opusdm/` directory for folder-scoped state.

**Architecture:** A new `DesktopSettings` struct lives in `crates/ipc` (on-disk shape, in-memory state, wire format, exactly like the existing `Settings`). It is persisted to `~/.config/opusdm/desktop.json`, owned and written by the hub. The config app connects to the hub socket like a Lister and sends one `ToHub::SetDesktopSettings` frame; the hub sanitises it, saves it, and applies it live (regenerates the desktop CSS, resizes/re-spaces the icon grid, re-targets the desktop-directory watch). If no hub is running, the app writes `desktop.json` itself and the change lands on the next hub start.

**Tech Stack:** Rust 2021, GTK4 (`gtk4` crate v0.10, `v4_10` features), `serde` / `serde_json`, `dirs`, Unix-socket IPC with 4-byte length-prefixed JSON frames (`opusdm_ipc::frame`).

**Spec:** `docs/superpowers/specs/2026-09-08-desktop-config-design.md`

## Global Constraints

- Edition 2021, `rust-version = "1.91"` (workspace).
- GTK: `gtk4 = { version = "0.10", features = ["v4_10"] }`. Do not use APIs newer than GTK 4.10.
- No new third-party dependencies beyond crates already in `[workspace.dependencies]` (`serde`, `serde_json`, `dirs`, `gtk4`, `async-channel`, `libc`, ...). The path-to-URL percent-encoder is a small local helper, not a crate.
- Never use the `—` character in code, comments, or docs; use `-`.
- All code and comments in English.
- Constants `UPPER_SNAKE_CASE`. Private/internal free functions and helpers prefixed `_` only where the surrounding module already does so (this codebase generally does **not** prefix `_` in Rust - follow the file you are editing; none of the touched files use the `_` prefix, so do not add it).
- Every public function/method gets a `///` doc comment.
- Exit conditions first, as one-liners where possible (`if !foo { return; }`).
- Text files end with a trailing newline.
- Use `pnpm` for any JS tooling (there is none here).
- Commit only files you changed, with meaningful messages. No AI attribution lines. Do not push. Do not use `--no-verify`.
- Keep `CHANGES.md` (repo root) up to date - final task.
- Build/test commands: `cargo test -p <crate>` per crate, `cargo build --workspace` / `cargo test --workspace` for the whole thing. GTK tests that need a display self-skip when `gtk::init()` fails - follow the existing pattern in `crates/lister/src/theme.rs` tests.

---

## File Structure

**New files**

- `crates/ipc/src/desktop.rs` - `DesktopSettings`, `Background`, `BackgroundMode`, `ImageFit`, ranges, `sanitized()`, `background_css()`, `path_to_file_url()`. Pure, no GTK.
- `crates/hub/src/desktop_settings.rs` - load/save `~/.config/opusdm/desktop.json`. Mirror of `crates/hub/src/settings.rs`.
- `crates/desktop-config/Cargo.toml` - new binary crate manifest.
- `crates/desktop-config/src/main.rs` - `gtk::Application`, window assembly, button wiring.
- `crates/desktop-config/src/model.rs` - load `DesktopSettings`, build it back from widget values, `send_or_write()`, RGBA<->hex helpers.
- `crates/desktop-config/src/ui.rs` - the three sections (background / grid / folder) as builder functions returning handles.

**Modified files**

- `crates/ipc/src/lib.rs` - `pub mod desktop;` + re-exports.
- `crates/ipc/src/proto.rs` - `ToHub::SetDesktopSettings(DesktopSettings)`.
- `crates/iconview/src/scan.rs` - `OPUSDM_DIR`, `LAYOUT_BASENAME`, `layout_path()`, skip `.opusdm`.
- `crates/iconview/src/field.rs` - `grid_spacing` cell + `set_grid_spacing()`, used by `rebuild_arranger`.
- `crates/lister/src/window.rs` - row filter uses `OPUSDM_DIR`.
- `crates/hub/src/lib.rs` - `pub mod desktop_settings;`.
- `crates/hub/src/server.rs` - load desktop settings, handle `SetDesktopSettings`, new `HubEvent::DesktopSettingsChanged`.
- `crates/hub/src/toolbar.rs` - parametric `install_desktop_css`, apply desktop settings on startup and on `DesktopSettingsChanged`, `desktop_dir` becomes `RefCell`, extract `spawn_desktop_watch`, context-menu "Settings" launches the app.
- `Cargo.toml` (root) - add `crates/desktop-config` to `members`.
- `CHANGES.md` - changelog entry.

---

## Task 1: `DesktopSettings` model in `crates/ipc`

**Files:**
- Create: `crates/ipc/src/desktop.rs`
- Modify: `crates/ipc/src/lib.rs`
- Test: `crates/ipc/src/desktop.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub enum BackgroundMode { Solid, Gradient, Image }` (derives `Default` -> `Solid`)
  - `pub enum ImageFit { Fill, Fit, Center, Tile }` (derives `Default` -> `Fill`)
  - `pub struct Background { pub mode: BackgroundMode, pub solid: String, pub gradient_start: String, pub gradient_end: String, pub gradient_angle: u16, pub image_path: Option<std::path::PathBuf>, pub image_fit: ImageFit }`
  - `pub struct DesktopSettings { pub background: Background, pub icon_size: i32, pub grid_spacing: i32, pub desktop_dir: Option<std::path::PathBuf> }`
  - `pub const ICON_SIZE_RANGE: (i32, i32) = (24, 256);`
  - `pub const GRID_SPACING_RANGE: (i32, i32) = (48, 320);`
  - `pub const GRADIENT_ANGLE_MAX: u16 = 360;`
  - `impl DesktopSettings { pub fn sanitized(self) -> DesktopSettings }`

- [ ] **Step 1: Write the failing tests**

Create `crates/ipc/src/desktop.rs` with only the test module first (it will not compile yet - that is the failure):

```rust
//! Desktop-window configuration: background, icon grid, and which directory
//! is shown as the desktop. Persisted to `~/.config/opusdm/desktop.json`,
//! owned by the hub, and sent from `opus-desktop-config` as
//! `ToHub::SetDesktopSettings`. Pure data - no GTK.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// (types go here in Step 3)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_reproduces_the_current_desktop_look() {
        let d = DesktopSettings::default();
        assert_eq!(d.background.mode, BackgroundMode::Solid);
        assert_eq!(d.background.solid, "#2e3436");
        assert_eq!(d.background.image_fit, ImageFit::Fill);
        assert_eq!(d.icon_size, 64);
        assert_eq!(d.grid_spacing, 96);
        assert_eq!(d.desktop_dir, None);
    }

    #[test]
    fn missing_fields_deserialize_to_defaults() {
        let d: DesktopSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(d, DesktopSettings::default());
        let d: DesktopSettings =
            serde_json::from_str(r#"{"background":{"mode":"gradient"}}"#).unwrap();
        assert_eq!(d.background.mode, BackgroundMode::Gradient);
        assert_eq!(d.background.solid, DesktopSettings::default().background.solid);
    }

    #[test]
    fn roundtrips_through_json() {
        let mut d = DesktopSettings::default();
        d.background.mode = BackgroundMode::Image;
        d.background.image_path = Some(PathBuf::from("/home/u/pic.png"));
        d.background.image_fit = ImageFit::Tile;
        d.icon_size = 96;
        d.desktop_dir = Some(PathBuf::from("/home/u/Desk"));
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<DesktopSettings>(&s).unwrap(), d);
    }

    #[test]
    fn enum_names_are_snake_case_on_the_wire() {
        let s = serde_json::to_string(&BackgroundMode::Gradient).unwrap();
        assert_eq!(s, "\"gradient\"");
        let s = serde_json::to_string(&ImageFit::Center).unwrap();
        assert_eq!(s, "\"center\"");
    }

    #[test]
    fn sanitized_clamps_every_numeric_field_at_both_ends() {
        let mut d = DesktopSettings::default();
        d.icon_size = 5;
        d.grid_spacing = 9999;
        d.background.gradient_angle = 900;
        let d = d.sanitized();
        assert_eq!(d.icon_size, ICON_SIZE_RANGE.0);
        assert_eq!(d.grid_spacing, GRID_SPACING_RANGE.1);
        assert_eq!(d.background.gradient_angle, GRADIENT_ANGLE_MAX);

        let mut d = DesktopSettings::default();
        d.icon_size = 999;
        d.grid_spacing = 1;
        let d = d.sanitized();
        assert_eq!(d.icon_size, ICON_SIZE_RANGE.1);
        assert_eq!(d.grid_spacing, GRID_SPACING_RANGE.0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p opusdm_ipc desktop::`
Expected: FAIL - `cannot find type DesktopSettings` / module `desktop` not declared.

- [ ] **Step 3: Add the module declaration and the types**

In `crates/ipc/src/lib.rs`, add after `pub mod proto;` (keep alphabetical-ish with the rest):

```rust
pub mod desktop;
```

and extend the `pub use` block:

```rust
pub use desktop::{
    Background, BackgroundMode, DesktopSettings, ImageFit, GRADIENT_ANGLE_MAX,
    GRID_SPACING_RANGE, ICON_SIZE_RANGE,
};
```

In `crates/ipc/src/desktop.rs`, above the test module, add:

```rust
/// Lower/upper bound for [`DesktopSettings::icon_size`], in pixels. Clamped by
/// [`DesktopSettings::sanitized`] so a hand-edited desktop.json cannot make an
/// icon vanish or fill the screen.
pub const ICON_SIZE_RANGE: (i32, i32) = (24, 256);

/// Lower/upper bound for [`DesktopSettings::grid_spacing`], in pixels.
pub const GRID_SPACING_RANGE: (i32, i32) = (48, 320);

/// Largest accepted gradient angle, in degrees.
pub const GRADIENT_ANGLE_MAX: u16 = 360;

/// Which kind of background the desktop window paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundMode {
    /// A single flat colour ([`Background::solid`]).
    #[default]
    Solid,
    /// A two-stop linear gradient.
    Gradient,
    /// An image file ([`Background::image_path`]).
    Image,
}

/// How a background image is fitted to the desktop window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    /// Cover the window, cropping overflow, keeping aspect ratio.
    #[default]
    Fill,
    /// Fit the whole image inside the window, keeping aspect ratio.
    Fit,
    /// Draw at natural size, centred.
    Center,
    /// Repeat at natural size from the top-left.
    Tile,
}

/// The desktop background. Only the field selected by [`Background::mode`] is
/// painted; the others keep their last value so switching modes back and forth
/// is lossless.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Background {
    pub mode: BackgroundMode,
    /// Hex `#rrggbb`. Also the fallback colour behind an image while it loads
    /// or when its path is unreadable.
    pub solid: String,
    /// Gradient start colour, hex `#rrggbb`.
    pub gradient_start: String,
    /// Gradient end colour, hex `#rrggbb`.
    pub gradient_end: String,
    /// Gradient direction in degrees, `0..=GRADIENT_ANGLE_MAX`. 0 points up,
    /// 90 points right (CSS `linear-gradient` convention).
    pub gradient_angle: u16,
    /// Absolute path to the background image, or `None`.
    pub image_path: Option<PathBuf>,
    pub image_fit: ImageFit,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            mode: BackgroundMode::Solid,
            solid: "#2e3436".to_string(),
            gradient_start: "#2e3436".to_string(),
            gradient_end: "#555753".to_string(),
            gradient_angle: 180,
            image_path: None,
            image_fit: ImageFit::Fill,
        }
    }
}

/// Every desktop-window setting `opus-desktop-config` owns. On-disk shape of
/// `~/.config/opusdm/desktop.json`, the hub's in-memory copy, and the payload
/// of [`crate::ToHub::SetDesktopSettings`] - one struct, three roles, like
/// [`crate::Settings`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopSettings {
    pub background: Background,
    /// Icon box size in pixels for the desktop icon field.
    pub icon_size: i32,
    /// Auto-slot grid pitch in pixels for the desktop icon field.
    pub grid_spacing: i32,
    /// Directory shown as the desktop. `None` means resolve it from
    /// `$XDG_DESKTOP_DIR` then `~/Desktop`, as the hub does today.
    pub desktop_dir: Option<PathBuf>,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            background: Background::default(),
            icon_size: 64,
            grid_spacing: 96,
            desktop_dir: None,
        }
    }
}

impl DesktopSettings {
    /// Clamp every numeric field into its accepted range. Call right after
    /// loading from disk and right after receiving a value over IPC.
    pub fn sanitized(mut self) -> DesktopSettings {
        let (lo, hi) = ICON_SIZE_RANGE;
        self.icon_size = self.icon_size.clamp(lo, hi);
        let (lo, hi) = GRID_SPACING_RANGE;
        self.grid_spacing = self.grid_spacing.clamp(lo, hi);
        self.background.gradient_angle = self.background.gradient_angle.min(GRADIENT_ANGLE_MAX);
        self
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p opusdm_ipc desktop::`
Expected: PASS (5 tests).

- [ ] **Step 5: Confirm the whole ipc crate still builds and passes**

Run: `cargo test -p opusdm_ipc`
Expected: PASS, no warnings from the new file.

- [ ] **Step 6: Commit**

```bash
git add crates/ipc/src/desktop.rs crates/ipc/src/lib.rs
git commit -m "feat(ipc): DesktopSettings model with serde defaults and clamps"
```

---

## Task 2: `background_css` and `path_to_file_url` helpers

**Files:**
- Modify: `crates/ipc/src/desktop.rs`
- Test: `crates/ipc/src/desktop.rs` tests

**Interfaces:**
- Consumes: `Background`, `BackgroundMode`, `ImageFit` from Task 1.
- Produces:
  - `pub fn background_css(bg: &Background) -> String` - the body of a CSS
    rule (declarations only, no selector, no braces), e.g.
    `"background: linear-gradient(180deg, #2e3436, #555753);"`.
  - `pub fn path_to_file_url(path: &std::path::Path) -> String` - a
    `file://` URL with every byte outside the unreserved set
    (`A-Z a-z 0-9 - _ . ~ /`) percent-encoded.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/ipc/src/desktop.rs`:

```rust
#[test]
fn solid_css_is_one_background_declaration() {
    let mut bg = Background::default();
    bg.mode = BackgroundMode::Solid;
    bg.solid = "#123456".into();
    assert_eq!(background_css(&bg), "background: #123456;");
}

#[test]
fn gradient_css_renders_angle_and_stops() {
    let mut bg = Background::default();
    bg.mode = BackgroundMode::Gradient;
    bg.gradient_start = "#000000".into();
    bg.gradient_end = "#ffffff".into();
    bg.gradient_angle = 45;
    assert_eq!(
        background_css(&bg),
        "background: linear-gradient(45deg, #000000, #ffffff);"
    );
}

#[test]
fn image_css_maps_each_fit_mode() {
    let base = |fit| {
        let mut bg = Background::default();
        bg.mode = BackgroundMode::Image;
        bg.image_path = Some(PathBuf::from("/x/y.png"));
        bg.solid = "#111111".into();
        bg.image_fit = fit;
        background_css(&bg)
    };
    let fill = base(ImageFit::Fill);
    assert!(fill.contains("background-image: url(\"file:///x/y.png\");"));
    assert!(fill.contains("background-color: #111111;"));
    assert!(fill.contains("background-size: cover;"));
    assert!(fill.contains("background-repeat: no-repeat;"));

    assert!(base(ImageFit::Fit).contains("background-size: contain;"));
    assert!(base(ImageFit::Center).contains("background-size: auto;"));
    assert!(base(ImageFit::Center).contains("background-repeat: no-repeat;"));

    let tile = base(ImageFit::Tile);
    assert!(tile.contains("background-repeat: repeat;"));
    assert!(tile.contains("background-size: auto;"));
}

#[test]
fn image_mode_without_a_path_falls_back_to_solid() {
    let mut bg = Background::default();
    bg.mode = BackgroundMode::Image;
    bg.image_path = None;
    bg.solid = "#abcdef".into();
    assert_eq!(background_css(&bg), "background: #abcdef;");
}

#[test]
fn file_url_percent_encodes_spaces_unicode_and_specials() {
    use std::path::Path;
    assert_eq!(path_to_file_url(Path::new("/a/b.png")), "file:///a/b.png");
    assert_eq!(
        path_to_file_url(Path::new("/a b/c#d%e.png")),
        "file:///a%20b/c%23d%25e.png"
    );
    assert_eq!(
        path_to_file_url(Path::new("/home/\u{00fc}ser/p.png")),
        "file:///home/%C3%BCser/p.png"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p opusdm_ipc desktop::`
Expected: FAIL - `cannot find function background_css`.

- [ ] **Step 3: Implement the helpers**

Add to `crates/ipc/src/desktop.rs` (above the test module), and add `use std::path::Path;` to the existing imports:

```rust
/// Bytes that never need percent-encoding in a path -> URL conversion:
/// RFC 3986 unreserved, plus `/` which is already a path separator.
fn is_url_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/')
}

/// Turn an absolute filesystem path into a `file://` URL, percent-encoding
/// every byte outside [`is_url_safe`]. Non-absolute paths are used as-is
/// after the `file://` prefix (the caller is expected to pass absolute
/// paths; the hub already stores absolute paths for desktop entries).
pub fn path_to_file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if is_url_safe(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// Build the declaration body of the `.opusdm-desktop` CSS rule from `bg`.
/// Returns declarations only - no selector, no braces - so the caller can
/// drop it straight into `".opusdm-desktop {{ {} }}"`.
///
/// `Image` mode with no path, or (later) an unreadable path, is the caller's
/// concern for the "unreadable" case; a `None` path falls back to `solid`
/// here so the desktop is never left transparent.
pub fn background_css(bg: &Background) -> String {
    match bg.mode {
        BackgroundMode::Solid => format!("background: {};", bg.solid),
        BackgroundMode::Gradient => format!(
            "background: linear-gradient({}deg, {}, {});",
            bg.gradient_angle, bg.gradient_start, bg.gradient_end
        ),
        BackgroundMode::Image => {
            let Some(path) = bg.image_path.as_deref() else {
                return format!("background: {};", bg.solid);
            };
            let url = path_to_file_url(path);
            let (size, repeat) = match bg.image_fit {
                ImageFit::Fill => ("cover", "no-repeat"),
                ImageFit::Fit => ("contain", "no-repeat"),
                ImageFit::Center => ("auto", "no-repeat"),
                ImageFit::Tile => ("auto", "repeat"),
            };
            format!(
                "background-color: {}; background-image: url(\"{}\"); \
                 background-position: center; background-size: {}; \
                 background-repeat: {};",
                bg.solid, url, size, repeat
            )
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p opusdm_ipc desktop::`
Expected: PASS (10 tests total in the module).

- [ ] **Step 5: Commit**

```bash
git add crates/ipc/src/desktop.rs
git commit -m "feat(ipc): background_css and path_to_file_url helpers"
```

---

## Task 3: `ToHub::SetDesktopSettings` wire variant

**Files:**
- Modify: `crates/ipc/src/proto.rs`
- Test: `crates/ipc/src/proto.rs` tests (or a small new test in the existing `#[cfg(test)] mod tests` there)

**Interfaces:**
- Consumes: `DesktopSettings` (Task 1).
- Produces: `ToHub::SetDesktopSettings(DesktopSettings)` variant.

- [ ] **Step 1: Write the failing test**

Find the `#[cfg(test)] mod tests` block in `crates/ipc/src/proto.rs` (there is one - it tests the `Settings` back-compat). Add:

```rust
#[test]
fn set_desktop_settings_frames_and_unframes() {
    use crate::frame::{recv, send};
    use crate::desktop::DesktopSettings;
    let msg = ToHub::SetDesktopSettings(DesktopSettings::default());
    let mut buf = Vec::new();
    send(&mut buf, &msg).unwrap();
    let mut cur = std::io::Cursor::new(buf);
    let got: Option<ToHub> = recv(&mut cur).unwrap();
    assert_eq!(got, Some(msg));
}
```

If `ToHub` does not derive `PartialEq`, check the enum's `#[derive(...)]`: the file's doc comment and the `ToLister` tests in `crates/lister/src/client.rs` use `matches!`, so `ToHub` may lack `PartialEq`. If so, assert with a match instead:

```rust
    match got {
        Some(ToHub::SetDesktopSettings(d)) => assert_eq!(d, DesktopSettings::default()),
        other => panic!("wrong message: {other:?}"),
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p opusdm_ipc proto::`
Expected: FAIL - `no variant named SetDesktopSettings`.

- [ ] **Step 3: Add the variant**

In `crates/ipc/src/proto.rs`, add `use crate::desktop::DesktopSettings;` near the other `use` lines (or reference it as `crate::desktop::DesktopSettings` inline). Add to the `enum ToHub`:

```rust
    /// Sent by `opus-desktop-config` (not a Lister): replace the hub's
    /// desktop settings. The hub sanitises, persists to
    /// `~/.config/opusdm/desktop.json`, and applies live. No reply.
    SetDesktopSettings(DesktopSettings),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p opusdm_ipc proto::`
Expected: PASS.

- [ ] **Step 5: Build every crate that matches `ToHub` exhaustively**

Run: `cargo build --workspace`
Expected: PASS. If the hub's `server.rs` has a non-wildcard `match` on `ToHub` it will fail to compile here - that is fixed in Task 7. If the build breaks only in `crates/hub` on a `non-exhaustive patterns` error for `ToHub`, that is expected; note it and continue. If it breaks anywhere else, stop and reassess.

- [ ] **Step 6: Commit**

```bash
git add crates/ipc/src/proto.rs
git commit -m "feat(ipc): ToHub::SetDesktopSettings variant"
```

---

## Task 4: Per-folder `.opusdm/` directory in `crates/iconview`

**Files:**
- Modify: `crates/iconview/src/scan.rs`
- Modify: `crates/lister/src/window.rs` (row filter, ~line 741)
- Test: `crates/iconview/src/scan.rs` tests

**Interfaces:**
- Produces:
  - `pub const OPUSDM_DIR: &str = ".opusdm";`
  - `pub const LAYOUT_BASENAME: &str = "layout";`
  - `pub fn layout_path(dir: &std::path::Path) -> std::path::PathBuf` -> `dir/.opusdm/layout`
- Removes: `pub const LAYOUT_FILE` (was `".opusdm-layout"`).

- [ ] **Step 1: Update the tests to the new shape**

In `crates/iconview/src/scan.rs`, in `mod tests`:

- Replace `fs::write(base.join(LAYOUT_FILE), b"{}").unwrap();` with:

```rust
        fs::create_dir(base.join(OPUSDM_DIR)).unwrap();
        fs::write(base.join(OPUSDM_DIR).join(LAYOUT_BASENAME), b"{}").unwrap();
```

- The `reports_kinds_and_filters` assertion `assert_eq!(names, ["adir", "afile.txt", "bad_link", "good_link"]);` still holds (`.opusdm` starts with `.`, filtered when `show_hidden` is false).
- Replace `assert!(!with_hidden.iter().any(|e| e.name == LAYOUT_FILE));` with:

```rust
        assert!(!with_hidden.iter().any(|e| e.name == OPUSDM_DIR));
```

- Add a focused test:

```rust
    #[test]
    fn layout_path_lives_under_the_per_folder_opusdm_dir() {
        let p = layout_path(Path::new("/tmp/desk"));
        assert_eq!(p, Path::new("/tmp/desk/.opusdm/layout"));
    }

    #[test]
    fn opusdm_dir_is_hidden_even_with_show_hidden() {
        let td = tempfile::tempdir().unwrap();
        fs::create_dir(td.path().join(OPUSDM_DIR)).unwrap();
        fs::write(td.path().join(".other"), b"x").unwrap();
        let got = read_entries(td.path(), true).unwrap();
        assert!(got.iter().any(|e| e.name == ".other"));
        assert!(!got.iter().any(|e| e.name == OPUSDM_DIR));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p opusdm_iconview scan::`
Expected: FAIL - `cannot find value OPUSDM_DIR` / `LAYOUT_BASENAME` / function `layout_path`.

- [ ] **Step 3: Implement**

In `crates/iconview/src/scan.rs`, replace:

```rust
/// The per-directory layout file, always excluded from the listing.
pub const LAYOUT_FILE: &str = ".opusdm-layout";
```

with:

```rust
/// The per-folder directory holding OpusDM state scoped to that folder
/// (icon layout today, room for more later). Always excluded from a listing.
pub const OPUSDM_DIR: &str = ".opusdm";

/// File name, inside [`OPUSDM_DIR`], that stores desktop icon positions.
pub const LAYOUT_BASENAME: &str = "layout";

/// Absolute path to `dir`'s icon-layout file: `<dir>/.opusdm/layout`. The
/// writer must `create_dir_all` the parent first.
pub fn layout_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(OPUSDM_DIR).join(LAYOUT_BASENAME)
}
```

In `read_entries`, replace:

```rust
        if name == LAYOUT_FILE {
            continue;
        }
```

with:

```rust
        if name == OPUSDM_DIR {
            continue;
        }
```

Update the function doc comment line `so is `LAYOUT_FILE`` -> `so is the `.opusdm` dir`.

- [ ] **Step 4: Fix the Lister row filter**

In `crates/lister/src/window.rs` around line 741, replace:

```rust
            if row.name == opusdm_iconview::scan::LAYOUT_FILE {
```

with:

```rust
            if row.name == opusdm_iconview::scan::OPUSDM_DIR {
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p opusdm_iconview scan::`
Expected: PASS.
Run: `cargo build -p opusdm_lister`
Expected: PASS (no other reference to `LAYOUT_FILE` - `grep -rn LAYOUT_FILE crates/` must return nothing).

- [ ] **Step 6: Commit**

```bash
git add crates/iconview/src/scan.rs crates/lister/src/window.rs
git commit -m "feat(iconview): per-folder .opusdm/ directory for layout state"
```

---

## Task 5: `IconField::set_grid_spacing`

**Files:**
- Modify: `crates/iconview/src/field.rs`
- Test: `crates/iconview/src/field.rs` tests (there is a `#[cfg(test)] mod tests` near the bottom that already builds `IconField` and calls `set_icon_size` / `set_mode` / `relayout`)

**Interfaces:**
- Produces: `pub fn set_grid_spacing(&self, px: i32)` on `IconField`. Sets the `FreeArrange` auto-slot pitch (`cell`). No effect in `Grid` mode until the next `set_mode(Free)`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/iconview/src/field.rs`. Model it on the existing free-mode tests (they call `f.set_mode(LayoutMode::Free)` then `f.relayout()` and read `f` geometry). Use the public surface only:

```rust
    #[test]
    fn set_grid_spacing_changes_the_free_auto_slot_pitch() {
        let Some(_g) = gtk_init_for_test() else { return }; // see note below
        let f = IconField::new();
        f.set_mode(LayoutMode::Free);
        f.set_icon_size(48);
        let entries = sample_entries(4); // 4 plain files
        f.set_entries(&entries);
        f.set_grid_spacing(200);
        f.relayout();
        let wide = f.item_rect(1).expect("item 1 laid out");

        f.set_grid_spacing(96);
        f.relayout();
        let narrow = f.item_rect(1).expect("item 1 laid out");

        // Larger spacing pushes the second auto-slotted item further right
        // (or onto a later row); either way its origin must move.
        assert_ne!(wide.0, narrow.0, "x origin should track grid spacing");
    }
```

Check `crates/iconview/src/field.rs` for the real helpers the other tests use:
- how they gate on a display (look for an existing `gtk::init()` / `gtk_test` helper or a `#[test]` that early-returns; reuse that exact pattern, replacing `gtk_init_for_test()` above).
- the helper that builds sample `IconEntry` values (replace `sample_entries`).
- the accessor for a laid-out rectangle (there is `item_rects` internally; the tests read positions somehow - reuse that; replace `item_rect`).

If no display-gate helper exists, guard with:

```rust
        if gtk::init().is_err() { return; }
```

as `crates/lister` tests do.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p opusdm_iconview field:: -- --nocapture`
Expected: FAIL - `no method named set_grid_spacing`.

- [ ] **Step 3: Implement**

In `struct Inner`, add after `icon_size: Cell<i32>,`:

```rust
    /// Auto-slot grid pitch for `FreeArrange`, in pixels. Ignored in `Grid`
    /// mode. Default matches `FreeArrange::new`'s `cell`.
    grid_spacing: Cell<i32>,
```

In `IconField::new`, in the `Inner { ... }` initializer, after `icon_size: Cell::new(64),`:

```rust
            grid_spacing: Cell::new(96),
```

Add the setter next to `set_icon_size`:

```rust
    /// Set the desktop auto-slot grid pitch in pixels. Takes effect in
    /// `Free` mode; harmless in `Grid` mode.
    pub fn set_grid_spacing(&self, px: i32) {
        if self.inner.grid_spacing.get() == px {
            return;
        }
        self.inner.grid_spacing.set(px);
        self.rebuild_arranger();
        self.relayout();
    }
```

In `rebuild_arranger`, the `LayoutMode::Free` arm currently is:

```rust
            LayoutMode::Free => {
                let mut fa = FreeArrange::new();
                fa.known = self.inner.free_positions.borrow().clone();
                Box::new(fa)
            }
```

Change it to:

```rust
            LayoutMode::Free => {
                let mut fa = FreeArrange::new();
                fa.known = self.inner.free_positions.borrow().clone();
                fa.cell = self.inner.grid_spacing.get() as f64;
                Box::new(fa)
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p opusdm_iconview field::`
Expected: PASS (or SKIP if headless with no display - acceptable, note it and move on).

- [ ] **Step 5: Full crate test**

Run: `cargo test -p opusdm_iconview`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/iconview/src/field.rs
git commit -m "feat(iconview): IconField::set_grid_spacing for the desktop grid pitch"
```

---

## Task 6: Hub `desktop_settings.rs` load/save

**Files:**
- Create: `crates/hub/src/desktop_settings.rs`
- Modify: `crates/hub/src/lib.rs` (`pub mod desktop_settings;`)
- Test: `crates/hub/src/desktop_settings.rs` tests

**Interfaces:**
- Consumes: `opusdm_ipc::DesktopSettings` (Task 1).
- Produces:
  - `pub fn load() -> DesktopSettings` - missing or malformed file -> `DesktopSettings::default()`, always `.sanitized()`.
  - `pub fn save(settings: &DesktopSettings) -> std::io::Result<()>`
  - `pub fn config_path() -> std::path::PathBuf` - `~/.config/opusdm/desktop.json` (mirrors `settings::config_path` logic).

- [ ] **Step 1: Write the failing tests**

Create `crates/hub/src/desktop_settings.rs`:

```rust
//! Desktop settings persistence: `~/.config/opusdm/desktop.json`.
//!
//! Same three-roles-one-struct design as [`crate::settings`]: the on-disk
//! shape is `opusdm_ipc::DesktopSettings`, which is also the hub's in-memory
//! copy and the payload of `ToHub::SetDesktopSettings`.

use opusdm_ipc::DesktopSettings;
use std::fs;
use std::path::PathBuf;

// (implementation in Step 3)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_from_missing_file_is_the_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("desktop.json");
        assert_eq!(load_from(&path), DesktopSettings::default());
    }

    #[test]
    fn load_from_malformed_file_is_the_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("desktop.json");
        fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load_from(&path), DesktopSettings::default());
    }

    #[test]
    fn save_then_load_roundtrips_and_sanitizes() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("nested").join("desktop.json");
        let mut d = DesktopSettings::default();
        d.icon_size = 100_000; // out of range on purpose
        d.background.mode = opusdm_ipc::BackgroundMode::Gradient;
        save_to(&path, &d).unwrap();
        let got = load_from(&path);
        assert_eq!(got.background.mode, opusdm_ipc::BackgroundMode::Gradient);
        assert_eq!(got.icon_size, opusdm_ipc::ICON_SIZE_RANGE.1); // clamped
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod desktop_settings;` to `crates/hub/src/lib.rs` (next to `pub mod settings;`), then:

Run: `cargo test -p opusdm_hub desktop_settings::`
Expected: FAIL - `cannot find function load_from`.

- [ ] **Step 3: Implement**

Add to `crates/hub/src/desktop_settings.rs` above the tests. Copy `config_path`'s fallback logic from `crates/hub/src/settings.rs` verbatim, changing only the file name:

```rust
/// Path to `~/.config/opusdm/desktop.json`. Falls back to `$HOME/.config`
/// then the temp dir when `dirs::config_dir()` yields nothing, exactly like
/// [`crate::settings`].
pub fn config_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("desktop.json");
    path
}

/// Load desktop settings from the standard path. Missing or unparseable ->
/// [`DesktopSettings::default`]. Always clamped via
/// [`DesktopSettings::sanitized`].
pub fn load() -> DesktopSettings {
    load_from(&config_path())
}

/// Persist `settings` to the standard path, creating the directory.
pub fn save(settings: &DesktopSettings) -> std::io::Result<()> {
    save_to(&config_path(), settings)
}

/// Testable core of [`load`].
fn load_from(path: &std::path::Path) -> DesktopSettings {
    if !path.exists() {
        return DesktopSettings::default();
    }
    let parsed = fs::read_to_string(path)
        .ok()
        .and_then(|c| serde_json::from_str::<DesktopSettings>(&c).ok());
    parsed.unwrap_or_default().sanitized()
}

/// Testable core of [`save`].
fn save_to(path: &std::path::Path, settings: &DesktopSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    fs::write(path, json)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p opusdm_hub desktop_settings::`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/hub/src/desktop_settings.rs crates/hub/src/lib.rs
git commit -m "feat(hub): desktop.json load/save mirroring settings.rs"
```

---

## Task 7: Hub server - load, handle `SetDesktopSettings`, emit `DesktopSettingsChanged`

**Files:**
- Modify: `crates/hub/src/server.rs`
- Test: `crates/hub/src/server.rs` tests (there is an existing integration-style test around lines 620-690 that drives the owner loop over a socket)

**Interfaces:**
- Consumes: `desktop_settings::load` / `save` (Task 6), `ToHub::SetDesktopSettings` (Task 3).
- Produces: `HubEvent::DesktopSettingsChanged(opusdm_ipc::DesktopSettings)`.

- [ ] **Step 1: Read the existing server test harness**

Open `crates/hub/src/server.rs` and read the `#[cfg(test)] mod tests` block. Identify the helper that starts the owner loop and connects a client socket (it drives `ToHub::Register` and reads `HubEvent`s off a channel). You will reuse it.

- [ ] **Step 2: Write the failing test**

Add to that `mod tests`:

```rust
#[test]
fn set_desktop_settings_emits_a_changed_event() {
    // Reuse whatever harness the neighbouring tests use to spawn the owner
    // loop and get (client_stream, evt_rx). Name the bindings to match them.
    let Harness { mut client, evt_rx, .. } = start_test_hub();

    let mut ds = opusdm_ipc::DesktopSettings::default();
    ds.icon_size = 5; // will be clamped by the hub
    ds.background.mode = opusdm_ipc::BackgroundMode::Gradient;
    opusdm_ipc::frame::send(&mut client, &opusdm_ipc::ToHub::SetDesktopSettings(ds)).unwrap();

    // Drain events until the DesktopSettingsChanged shows up.
    let got = loop {
        match evt_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            HubEvent::DesktopSettingsChanged(d) => break d,
            _ => continue,
        }
    };
    assert_eq!(got.background.mode, opusdm_ipc::BackgroundMode::Gradient);
    assert_eq!(got.icon_size, opusdm_ipc::ICON_SIZE_RANGE.0); // clamped
}
```

Adjust `Harness` / `start_test_hub` / `evt_rx` names to the real ones in the file. If the harness writes `desktop.json` to the real config dir, set `HOME` / `XDG_CONFIG_HOME` to a tempdir at the top of the test so it does not touch the developer's config.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p opusdm_hub server:: -- set_desktop_settings`
Expected: FAIL - `no variant DesktopSettingsChanged` / pattern not matched.

- [ ] **Step 4: Implement**

In `crates/hub/src/server.rs`:

a. Add the event variant to `enum HubEvent`:

```rust
    /// Desktop settings changed (from `opus-desktop-config`). Carries the
    /// sanitised value; the GTK side repaints and re-lays the desktop.
    DesktopSettingsChanged(opusdm_ipc::DesktopSettings),
```

b. In `owner_loop`, next to `let mut current_settings = settings::load_settings();`:

```rust
    let mut current_desktop = crate::desktop_settings::load();
```

(`current_desktop` is not read again here yet - Task 8 consumes the event on the GTK side. Prefix with `let _ = &current_desktop;` is not needed; keep it, it is updated below and this silences no warning because it is assigned in the match arm. If the compiler warns "unused", change to `#[allow(unused_assignments)]` on the binding or actually use it - see note.)

Actually: to avoid an unused-variable warning, send an initial event is wrong (GTK side is not up yet). Instead, mark it used by logging once:

```rust
    eprintln!(
        "opusdm-hub: desktop settings loaded (mode {:?})",
        current_desktop.background.mode
    );
```

c. Add the match arm. Find where `OwnerMsg::FromLister(_cid, ToHub::RaiseToolbar)` is handled and add alongside:

```rust
            OwnerMsg::FromLister(_cid, ToHub::SetDesktopSettings(ds)) => {
                current_desktop = ds.sanitized();
                if let Err(e) = crate::desktop_settings::save(&current_desktop) {
                    eprintln!("opusdm-hub: cannot save desktop.json: {e}");
                }
                let _ = evt_tx.send(HubEvent::DesktopSettingsChanged(current_desktop.clone()));
            }
```

d. If `server.rs` has a catch-all `OwnerMsg::FromLister(_, _) => {}` or a non-exhaustive `ToHub` match elsewhere, make sure the new arm is above it. Run `cargo build -p opusdm_hub` and fix any `non-exhaustive patterns: ToHub::SetDesktopSettings(_)` by adding a `SetDesktopSettings` arm (ignore with `=> {}` only in places that legitimately do not care, e.g. a reader-side filter).

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p opusdm_hub server::`
Expected: PASS.

- [ ] **Step 6: Handle the GTK-side event so the build stays exhaustive**

`crates/hub/src/toolbar.rs` `apply_event` matches `HubEvent`. Add a temporary arm so the workspace builds now; Task 8 fills it in:

```rust
        HubEvent::DesktopSettingsChanged(_ds) => {
            // Applied in Task 8.
        }
```

- [ ] **Step 7: Full hub build + test**

Run: `cargo build -p opusdm_hub && cargo test -p opusdm_hub`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/hub/src/server.rs crates/hub/src/toolbar.rs
git commit -m "feat(hub): accept ToHub::SetDesktopSettings and emit DesktopSettingsChanged"
```

---

## Task 8: Hub toolbar - parametric desktop CSS + apply background/grid live

**Files:**
- Modify: `crates/hub/src/toolbar.rs`
- Test: `crates/hub/src/toolbar.rs` tests (add a `#[cfg(test)] mod tests` if none exists; gate GTK bits on `gtk::init()`)

**Interfaces:**
- Consumes: `opusdm_ipc::{DesktopSettings, background_css}` (Tasks 1-2), `IconField::set_grid_spacing` (Task 5), `desktop_settings::load` (Task 6), `HubEvent::DesktopSettingsChanged` (Task 7).
- Produces: `fn install_desktop_css(bg: &opusdm_ipc::Background)` (was zero-arg); `fn desktop_rule_css(bg: &opusdm_ipc::Background) -> String` (full stylesheet string, testable).

- [ ] **Step 1: Write the failing test**

Add to `crates/hub/src/toolbar.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_rule_css_embeds_the_background_declarations() {
        let mut bg = opusdm_ipc::Background::default();
        bg.mode = opusdm_ipc::BackgroundMode::Gradient;
        bg.gradient_angle = 30;
        let css = desktop_rule_css(&bg);
        assert!(css.contains(".opusdm-desktop {"));
        assert!(css.contains("linear-gradient(30deg,"));
        assert!(css.contains(".opusdm-desktop-label {")); // other rules kept
        assert!(css.contains(".opusdm-desktop-menu {"));
    }

    #[test]
    fn desktop_rule_css_parses_in_gtk() {
        if gtk::init().is_err() {
            return; // headless CI: skip, like the lister theme tests
        }
        let provider = gtk::CssProvider::new();
        // Must not emit a parser error for any mode.
        for mode in [
            opusdm_ipc::BackgroundMode::Solid,
            opusdm_ipc::BackgroundMode::Gradient,
            opusdm_ipc::BackgroundMode::Image,
        ] {
            let mut bg = opusdm_ipc::Background::default();
            bg.mode = mode;
            bg.image_path = Some(std::path::PathBuf::from("/tmp/opusdm-test bg.png"));
            provider.load_from_data(&desktop_rule_css(&bg));
        }
    }
}
```

Check the existing lister CSS-parse test (`crates/lister/src/theme.rs` tests) for how it asserts "no parser error" - GTK 4.10 `CssProvider` emits errors via the `parsing-error` signal, not a `Result`. Mirror that exactly (connect the signal, set a flag, assert it stayed false).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p opusdm_hub toolbar::tests`
Expected: FAIL - `cannot find function desktop_rule_css`.

- [ ] **Step 3: Refactor `install_desktop_css`**

Current (around line 281):

```rust
fn install_desktop_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&format!(
        ".opusdm-desktop {{ background-color: {DESKTOP_BG}; }}\n\
         .opusdm-desktop-label {{ color: {DESKTOP_FG}; font-size: 24px; font-weight: bold; }}\n\
         .opusdm-desktop-menu {{ background-color: {DESKTOP_MENU_BG}; color: {DESKTOP_FG}; \
             border: 1px solid {DESKTOP_MENU_BORDER}; border-radius: 4px; padding: 4px; }}\n\
         .opusdm-desktop-menu button {{ padding: 4px 12px; }}"
    ));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
```

Replace with:

```rust
/// The full desktop stylesheet: the `.opusdm-desktop` background rule built
/// from `bg`, plus the fixed label and context-menu rules.
fn desktop_rule_css(bg: &opusdm_ipc::Background) -> String {
    format!(
        ".opusdm-desktop {{ {bgdecl} }}\n\
         .opusdm-desktop-label {{ color: {DESKTOP_FG}; font-size: 24px; font-weight: bold; }}\n\
         .opusdm-desktop-menu {{ background-color: {DESKTOP_MENU_BG}; color: {DESKTOP_FG}; \
             border: 1px solid {DESKTOP_MENU_BORDER}; border-radius: 4px; padding: 4px; }}\n\
         .opusdm-desktop-menu button {{ padding: 4px 12px; }}",
        bgdecl = opusdm_ipc::background_css(bg),
    )
}

/// Install (or replace) the desktop stylesheet for `bg`. Safe to call
/// repeatedly - each call adds a fresh provider at APPLICATION priority,
/// which overrides earlier ones.
fn install_desktop_css(bg: &opusdm_ipc::Background) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&desktop_rule_css(bg));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
```

Note: stacking providers on every change is acceptable here (a handful over a session). If you prefer, keep a `thread_local!` `RefCell<Option<CssProvider>>`, `remove_provider_for_display` the old one, then add the new - either is fine; the simple version is the default.

`DESKTOP_BG` (line 24) is now unused. Remove the `const DESKTOP_BG` line. Leave `DESKTOP_FG`, `DESKTOP_MENU_BG`, `DESKTOP_MENU_BORDER` as they are still referenced.

- [ ] **Step 4: Load desktop settings at startup and use them**

In `build_ui`:

a. Near the top, after `let (mon_w, mon_h) = ...;`:

```rust
    let desktop_settings = opusdm_hub::desktop_settings::load();
```

b. Change `install_desktop_css();` (line ~102, inside `build_ui` before the window is built - check exact call site) to run after `desktop_settings` exists:

```rust
    install_desktop_css(&desktop_settings.background);
```

If the current `install_desktop_css()` call is above where you can compute `desktop_settings`, move the `let desktop_settings = ...` line up so it precedes the call. It only needs `opusdm_hub` and the config file.

c. After `build_desktop_area` returns `desktop_field`, apply size + spacing:

```rust
    if let Some(f) = &desktop_field {
        f.set_icon_size(desktop_settings.icon_size);
        f.set_grid_spacing(desktop_settings.grid_spacing);
    }
```

- [ ] **Step 5: Fill in the `DesktopSettingsChanged` arm**

Replace the temporary arm from Task 7 Step 6 in `apply_event`:

```rust
        HubEvent::DesktopSettingsChanged(ds) => {
            install_desktop_css(&ds.background);
            if let Some(field) = widgets.desktop_field.borrow().as_ref() {
                field.set_icon_size(ds.icon_size);
                field.set_grid_spacing(ds.grid_spacing);
            }
            // Desktop-directory changes are handled in Task 9.
        }
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p opusdm_hub toolbar::`
Expected: PASS (the GTK-parse test may SKIP headless).
Run: `cargo build -p opusdm_hub`
Expected: PASS, and no `unused` warning for `DESKTOP_BG` (you removed it) or `desktop_settings`.

- [ ] **Step 7: Manual smoke (if a display is available)**

Run: `./run-x11.sh` , then from another shell inside the sandbox env edit `~/.config/opusdm/desktop.json` is not yet wired to reload; instead just confirm the hub starts and the desktop paints the default `#2e3436`. Full live check happens in Task 14.

- [ ] **Step 8: Commit**

```bash
git add crates/hub/src/toolbar.rs
git commit -m "feat(hub): paint the desktop background from DesktopSettings, live"
```

---

## Task 9: Hub toolbar - live desktop-directory change

**Files:**
- Modify: `crates/hub/src/toolbar.rs`
- Modify: `crates/hub/src/desktop_files.rs` (add an override-aware resolver)
- Test: `crates/hub/src/desktop_files.rs` tests

**Interfaces:**
- Consumes: `DesktopSettings::desktop_dir`, existing `desktop_files::{desktop_dir, resolve_desktop_dir, DesktopWatch}`.
- Produces: `pub fn effective_desktop_dir(override_dir: Option<&std::path::Path>) -> Option<std::path::PathBuf>` in `desktop_files.rs`.
- Changes `Widgets.desktop_dir` from `Option<PathBuf>` to `RefCell<Option<PathBuf>>`.

- [ ] **Step 1: Write the failing test**

Add to `crates/hub/src/desktop_files.rs` `mod tests`:

```rust
    #[test]
    fn effective_desktop_dir_prefers_a_valid_override() {
        let td = tempfile::tempdir().unwrap();
        let over = td.path().join("MyDesk");
        std::fs::create_dir(&over).unwrap();
        assert_eq!(effective_desktop_dir(Some(&over)), Some(over.clone()));
    }

    #[test]
    fn effective_desktop_dir_ignores_a_missing_override() {
        let missing = std::path::Path::new("/no/such/dir/opusdm-test");
        // Falls back to the XDG/~ resolution (may be None in CI - just assert
        // it does not return the bogus path).
        assert_ne!(effective_desktop_dir(Some(missing)), Some(missing.to_path_buf()));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p opusdm_hub desktop_files::`
Expected: FAIL - `cannot find function effective_desktop_dir`.

- [ ] **Step 3: Implement the resolver**

In `crates/hub/src/desktop_files.rs`, add:

```rust
/// The directory to show as the desktop. A readable directory passed as
/// `override_dir` wins; otherwise fall back to [`desktop_dir`]'s
/// `$XDG_DESKTOP_DIR` then `~/Desktop` resolution.
pub fn effective_desktop_dir(override_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = override_dir {
        if p.is_dir() {
            return Some(p.to_path_buf());
        }
    }
    desktop_dir()
}
```

- [ ] **Step 4: Make `Widgets.desktop_dir` mutable and route the field wiring through it**

In `crates/hub/src/toolbar.rs`:

a. `struct Widgets`: change

```rust
    desktop_dir: Option<PathBuf>,
```

to

```rust
    desktop_dir: RefCell<Option<PathBuf>>,
```

b. In `build_ui`, the `Widgets { ... }` initializer: `desktop_dir: desktop_dir.clone(),` becomes `desktop_dir: RefCell::new(desktop_dir.clone()),`.

c. Replace the direct `opusdm_hub::desktop_files::desktop_dir()` call (line ~173) with:

```rust
    let desktop_dir =
        opusdm_hub::desktop_files::effective_desktop_dir(desktop_settings.desktop_dir.as_deref());
```

d. Every other read of `widgets.desktop_dir` (there are uses around lines 476 and in the drop handler) becomes `widgets.desktop_dir.borrow().clone()`. Search: `grep -n "desktop_dir" crates/hub/src/toolbar.rs` and fix each.

e. Extract the watch+reload wiring (currently inline in `build_ui` around lines 228-244) into a function so Task 9 can call it again on a directory change:

```rust
/// Start (or restart) the inotify watch on `dir` and the local future that
/// reloads the desktop icon field when the directory changes. Replaces any
/// previous watch stored in `widgets.desktop_watch`.
fn spawn_desktop_watch(widgets: &Rc<Widgets>, field: &opusdm_iconview::IconField, dir: &Path) {
    // Drop the old watch first so its thread exits before we start another.
    *widgets.desktop_watch.borrow_mut() = None;
    match opusdm_hub::desktop_files::DesktopWatch::start(dir.to_path_buf()) {
        Ok((watch, rx)) => {
            let field = field.clone();
            let dir = dir.to_path_buf();
            glib::spawn_future_local(async move {
                while rx.recv().await.is_ok() {
                    match opusdm_iconview::scan::read_entries(&dir, false) {
                        Ok(entries) => field.set_entries(&entries),
                        Err(e) => eprintln!("opusdm-hub: desktop reload failed: {e}"),
                    }
                }
            });
            *widgets.desktop_watch.borrow_mut() = Some(watch);
        }
        Err(e) => eprintln!("opusdm-hub: desktop watch unavailable: {e}"),
    }
}
```

Replace the inline `match opusdm_hub::desktop_files::DesktopWatch::start(...) { ... }` in `build_ui` with `spawn_desktop_watch(&widgets, &field, &dir);`.

- [ ] **Step 5: Handle a directory change in `DesktopSettingsChanged`**

Extend the arm from Task 8:

```rust
        HubEvent::DesktopSettingsChanged(ds) => {
            install_desktop_css(&ds.background);
            let field = widgets.desktop_field.borrow().clone();
            if let Some(field) = field.as_ref() {
                field.set_icon_size(ds.icon_size);
                field.set_grid_spacing(ds.grid_spacing);
            }

            let wanted = opusdm_hub::desktop_files::effective_desktop_dir(
                ds.desktop_dir.as_deref(),
            );
            let changed = *widgets.desktop_dir.borrow() != wanted;
            if changed {
                *widgets.desktop_dir.borrow_mut() = wanted.clone();
                if let (Some(field), Some(dir)) = (field.as_ref(), wanted.as_ref()) {
                    match opusdm_iconview::scan::read_entries(dir, false) {
                        Ok(entries) => field.set_entries(&entries),
                        Err(e) => eprintln!("opusdm-hub: cannot read desktop {}: {e}", dir.display()),
                    }
                    spawn_desktop_watch(widgets, field, dir);
                }
            }
        }
```

Note: `apply_event(widgets: &Rc<Widgets>, ...)` already takes `&Rc<Widgets>`, so `spawn_desktop_watch(widgets, ...)` type-checks. If `field` is `None` (no desktop dir at startup) a later directory change cannot create the field - that is an accepted limitation for this iteration; document it in the arm with a one-line comment and in CHANGES.

- [ ] **Step 6: Run tests + build**

Run: `cargo test -p opusdm_hub && cargo build -p opusdm_hub`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/hub/src/toolbar.rs crates/hub/src/desktop_files.rs
git commit -m "feat(hub): re-target the desktop directory watch live on settings change"
```

---

## Task 10: Hub context menu - launch `opus-desktop-config`

**Files:**
- Modify: `crates/hub/src/toolbar.rs`
- Test: `crates/hub/src/toolbar.rs` tests

**Interfaces:**
- Produces: `fn desktop_config_bin() -> std::path::PathBuf` - the app binary next to the current executable, falling back to the bare name `opus-desktop-config`.

- [ ] **Step 1: Locate the placeholder**

`grep -n "Settings" crates/hub/src/toolbar.rs`. The desktop context-menu "Settings" entry (added in the recent "Desktop submenu" commit) currently just closes the menu. Find its `connect_clicked` / activation closure.

- [ ] **Step 2: Write the failing test**

```rust
    #[test]
    fn desktop_config_bin_sits_next_to_the_current_exe_or_falls_back() {
        let p = desktop_config_bin();
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some("opus-desktop-config")
        );
        // When we can find our own dir, the result is absolute.
        if std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).is_some() {
            assert!(p.is_absolute());
        }
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p opusdm_hub toolbar:: -- desktop_config_bin`
Expected: FAIL - `cannot find function desktop_config_bin`.

- [ ] **Step 4: Implement**

Add to `crates/hub/src/toolbar.rs`:

```rust
/// Resolve the `opus-desktop-config` binary: preferably a sibling of the
/// running hub executable (so a `cargo run` / installed pair stay together),
/// otherwise the bare name for a `PATH` lookup.
fn desktop_config_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("opus-desktop-config");
            if sibling.exists() {
                return sibling;
            }
            return dir.join("opus-desktop-config"); // may not exist yet; Command will error
        }
    }
    PathBuf::from("opus-desktop-config")
}

/// Spawn the desktop configuration app, detached. Errors are logged and
/// otherwise ignored - a missing binary must not take down the hub.
fn launch_desktop_config() {
    let bin = desktop_config_bin();
    match std::process::Command::new(&bin).spawn() {
        Ok(_) => {}
        Err(e) => eprintln!("opusdm-hub: cannot launch {}: {e}", bin.display()),
    }
}
```

In the "Settings" menu-entry closure, add `launch_desktop_config();` before / instead of the menu-close call (keep the menu-close).

- [ ] **Step 5: Run test + build**

Run: `cargo test -p opusdm_hub toolbar:: && cargo build -p opusdm_hub`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/hub/src/toolbar.rs
git commit -m "feat(hub): desktop context-menu Settings launches opus-desktop-config"
```

---

## Task 11: New crate `crates/desktop-config` - scaffold + model

**Files:**
- Create: `crates/desktop-config/Cargo.toml`
- Create: `crates/desktop-config/src/main.rs`
- Create: `crates/desktop-config/src/model.rs`
- Modify: `Cargo.toml` (root) - workspace `members`
- Test: `crates/desktop-config/src/model.rs` tests

**Interfaces:**
- Consumes: `opusdm_ipc::{DesktopSettings, ToHub, frame, sockpath}`.
- Produces (in `model.rs`):
  - `pub fn load() -> DesktopSettings` - from `~/.config/opusdm/desktop.json` via a path helper duplicated from the hub (small, or factor later), missing/malformed -> default, `.sanitized()`.
  - `pub enum ApplyOutcome { SentToHub, WroteFile, Failed(String) }`
  - `pub fn apply(settings: &DesktopSettings) -> ApplyOutcome` - if `sockpath::probe_live_hub(&sockpath::hub_sock_path())` connect and send one `ToHub::SetDesktopSettings` frame -> `SentToHub`; else write `desktop.json` -> `WroteFile`; I/O error -> `Failed`.
  - `pub fn rgba_to_hex(c: &gtk::gdk::RGBA) -> String` -> `#rrggbb`
  - `pub fn hex_to_rgba(hex: &str) -> gtk::gdk::RGBA` - parse `#rrggbb`, fall back to opaque black on error.

- [ ] **Step 1: Create the manifest and wire the workspace**

`crates/desktop-config/Cargo.toml`:

```toml
[package]
name = "opusdm-desktop-config"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "opus-desktop-config"
path = "src/main.rs"

[dependencies]
opusdm_ipc = { path = "../ipc" }
gtk4 = { workspace = true }
serde_json = { workspace = true }
dirs = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

Check an existing crate manifest (`crates/lister/Cargo.toml`) for the exact `opusdm_ipc` package name and path style; match it (it may be `opusdm-ipc` vs `opusdm_ipc` - use whatever the others use).

Root `Cargo.toml`: add `"crates/desktop-config"` to `workspace.members`.

- [ ] **Step 2: Write the failing tests**

`crates/desktop-config/src/model.rs`:

```rust
//! Load desktop settings for the config window, and apply edits: send them to
//! a running hub over the socket, or write `desktop.json` directly when no hub
//! is up.

use opusdm_ipc::DesktopSettings;
use std::path::PathBuf;

// (implementation in Step 4)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrips_through_rgba() {
        let c = hex_to_rgba("#3c8ad9");
        assert_eq!(rgba_to_hex(&c), "#3c8ad9");
    }

    #[test]
    fn bad_hex_is_opaque_black() {
        assert_eq!(rgba_to_hex(&hex_to_rgba("not-a-colour")), "#000000");
    }

    #[test]
    fn apply_writes_the_file_when_no_hub_is_running() {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        std::env::set_var("OPUSDM_HUB_SOCK", td.path().join("definitely-absent.sock"));
        let mut d = DesktopSettings::default();
        d.icon_size = 88;
        match apply(&d) {
            ApplyOutcome::WroteFile => {}
            other => panic!("expected WroteFile, got {other:?}"),
        }
        let written = std::fs::read_to_string(td.path().join("opusdm").join("desktop.json")).unwrap();
        assert!(written.contains("\"icon_size\": 88"));
    }
}
```

Note the `hex_to_rgba`/`rgba_to_hex` tests need `gtk::gdk::RGBA`, which does not require `gtk::init()` (RGBA is a plain value type). If it does pull in GL/display at link time in this environment, gate these two tests behind `if gtk::init().is_err() { return; }`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p opusdm-desktop-config`
Expected: FAIL - unresolved `apply`, `ApplyOutcome`, `hex_to_rgba`, `rgba_to_hex`.

- [ ] **Step 4: Implement `model.rs`**

```rust
/// Path to `~/.config/opusdm/desktop.json`. Same resolution as the hub.
pub fn config_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("desktop.json");
    path
}

/// Load current desktop settings, or defaults if the file is missing or
/// unparseable. Always clamped.
pub fn load() -> DesktopSettings {
    let path = config_path();
    let parsed = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<DesktopSettings>(&c).ok());
    parsed.unwrap_or_default().sanitized()
}

/// What `apply` did.
#[derive(Debug)]
pub enum ApplyOutcome {
    /// Sent to a running hub; it will persist and repaint.
    SentToHub,
    /// No hub running - wrote `desktop.json`; effective on next hub start.
    WroteFile,
    /// Could not send and could not write.
    Failed(String),
}

/// Send `settings` to a running hub, or persist to disk if none is running.
pub fn apply(settings: &DesktopSettings) -> ApplyOutcome {
    let sock = opusdm_ipc::sockpath::hub_sock_path();
    if opusdm_ipc::sockpath::probe_live_hub(&sock) {
        match send_to_hub(&sock, settings) {
            Ok(()) => return ApplyOutcome::SentToHub,
            Err(e) => eprintln!("opus-desktop-config: hub send failed, writing file: {e}"),
        }
    }
    match write_file(settings) {
        Ok(()) => ApplyOutcome::WroteFile,
        Err(e) => ApplyOutcome::Failed(e.to_string()),
    }
}

fn send_to_hub(sock: &std::path::Path, settings: &DesktopSettings) -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(sock)?;
    opusdm_ipc::frame::send(
        &mut stream,
        &opusdm_ipc::ToHub::SetDesktopSettings(settings.clone()),
    )
}

fn write_file(settings: &DesktopSettings) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(settings)?)
}

/// `gdk::RGBA` -> `#rrggbb` (alpha dropped).
pub fn rgba_to_hex(c: &gtk4::gdk::RGBA) -> String {
    let to_u8 = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", to_u8(c.red()), to_u8(c.green()), to_u8(c.blue()))
}

/// `#rrggbb` (or any `gdk::RGBA::parse`-able string) -> `gdk::RGBA`.
/// Unparseable input yields opaque black.
pub fn hex_to_rgba(hex: &str) -> gtk4::gdk::RGBA {
    gtk4::gdk::RGBA::parse(hex).unwrap_or_else(|_| gtk4::gdk::RGBA::new(0.0, 0.0, 0.0, 1.0))
}
```

Adjust `gtk4::gdk` vs `gtk::gdk` to match how the crate imports GTK (the manifest package is `gtk4`, imported as `use gtk4 as gtk;` conventionally - check `crates/lister/src/window.rs` top and match it; if they `use gtk4 as gtk;` then write `gtk::gdk::RGBA`).

- [ ] **Step 5: Minimal `main.rs` so the crate builds**

```rust
//! `opus-desktop-config`: configure the OpusDM desktop - background, icon
//! grid, and which directory is shown as the desktop.

use gtk4 as gtk;
use gtk::prelude::*;

mod model;

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.opusdm.DesktopConfig")
        .build();
    app.connect_activate(build_window);
    app.run()
}

/// Build and show the single configuration window.
fn build_window(app: &gtk::Application) {
    let _settings = model::load();
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Desktop")
        .default_width(460)
        .default_height(560)
        .build();
    // Sections are added in Tasks 12-14.
    window.present();
}
```

- [ ] **Step 6: Run tests + build**

Run: `cargo test -p opusdm-desktop-config`
Expected: PASS.
Run: `cargo build --workspace`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/desktop-config/ Cargo.toml Cargo.lock
git commit -m "feat(desktop-config): new crate scaffold, settings load and apply"
```

---

## Task 12: `opus-desktop-config` UI - Background section

**Files:**
- Create: `crates/desktop-config/src/ui.rs`
- Modify: `crates/desktop-config/src/main.rs`
- Test: `crates/desktop-config/src/ui.rs` tests (pure helper only)

**Interfaces:**
- Produces:
  - `pub struct BackgroundSection { pub root: gtk::Widget, /* getters below */ }`
  - `impl BackgroundSection { pub fn new(initial: &opusdm_ipc::Background) -> Self; pub fn read(&self) -> opusdm_ipc::Background; pub fn set_image_warning(&self, visible: bool); }`
  - `pub fn image_path_is_usable(p: Option<&std::path::Path>) -> bool` - `Some` and `path.is_file()`.

- [ ] **Step 1: Write the failing test (pure helper)**

`crates/desktop-config/src/ui.rs`:

```rust
//! The three sections of the desktop-config window. Each is a builder that
//! owns its widgets and can read its values back into a settings struct.

use gtk4 as gtk;
use gtk::prelude::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_path_usable_only_for_an_existing_file() {
        assert!(!image_path_is_usable(None));
        assert!(!image_path_is_usable(Some(std::path::Path::new("/no/such/file.png"))));
        let f = tempfile::NamedTempFile::new().unwrap();
        assert!(image_path_is_usable(Some(f.path())));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p opusdm-desktop-config ui::`
Expected: FAIL - `cannot find function image_path_is_usable`.

- [ ] **Step 3: Implement the helper + the section**

Add `mod ui;` to `main.rs`. In `ui.rs`:

```rust
/// True when `p` points at a file that exists and can be used as a background.
pub fn image_path_is_usable(p: Option<&std::path::Path>) -> bool {
    p.map(|p| p.is_file()).unwrap_or(false)
}
```

Then the section. Keep it boring and explicit: a `gtk::Box` (vertical) containing:
- a heading `Label` "BACKGROUND" (`add_css_class("heading")` like the hub dialog);
- three `gtk::CheckButton` radios sharing a group (`set_group(Some(&first))`): "Solid colour", "Gradient", "Image";
- a `gtk::Stack` with three named pages:
  - `"solid"`: one `gtk::ColorDialogButton` (GTK 4.10 has `ColorButton`; `ColorDialogButton` is 4.10+ - confirm against the `v4_10` feature. If `ColorDialogButton` is not in 0.10/`v4_10`, use `gtk::ColorButton` with `.rgba()` / `.set_rgba()`).
  - `"gradient"`: two colour buttons (start, end) + a `gtk::SpinButton::with_range(0.0, 360.0, 1.0)` for the angle;
  - `"image"`: a read-only `gtk::Entry` + a "Browse..." `gtk::Button` (opens `gtk::FileDialog`, `open` with an image filter; on success set the entry text), a `gtk::DropDown::from_strings(&["Fill", "Fit", "Center", "Tile"])`, and a hidden warning `gtk::Label` ("Image not found") with `add_css_class("warning")`.
- radio `connect_toggled` sets the stack's visible child name.

`new(initial)` seeds every widget from `initial`. `read()` builds a `Background`:
- `mode` from which radio is active;
- `solid`/`gradient_*` from the colour buttons via `model::rgba_to_hex`;
- `gradient_angle` from the spin value `as u16`;
- `image_path` from the entry text (`Some(PathBuf::from(t))` when non-empty, else `None`);
- `image_fit` from the dropdown selected index (0..3 -> Fill/Fit/Center/Tile).

Store the widgets you need for `read()` in the struct (colour buttons, spin, entry, dropdown, the radios, the warning label).

Full code for `new`/`read` (write it out - do not abbreviate):

```rust
use opusdm_ipc::{Background, BackgroundMode, ImageFit};

/// The BACKGROUND section: mode radios + a stack of per-mode controls.
pub struct BackgroundSection {
    pub root: gtk::Box,
    radio_solid: gtk::CheckButton,
    radio_gradient: gtk::CheckButton,
    radio_image: gtk::CheckButton,
    solid_btn: gtk::ColorButton,
    grad_start: gtk::ColorButton,
    grad_end: gtk::ColorButton,
    grad_angle: gtk::SpinButton,
    image_entry: gtk::Entry,
    image_fit: gtk::DropDown,
    image_warning: gtk::Label,
}

impl BackgroundSection {
    /// Build the section, seeded from `initial`.
    pub fn new(initial: &Background) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("BACKGROUND"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let radio_solid = gtk::CheckButton::with_label("Solid colour");
        let radio_gradient = gtk::CheckButton::with_label("Gradient");
        let radio_image = gtk::CheckButton::with_label("Image");
        radio_gradient.set_group(Some(&radio_solid));
        radio_image.set_group(Some(&radio_solid));
        root.append(&radio_solid);
        root.append(&radio_gradient);
        root.append(&radio_image);

        let stack = gtk::Stack::new();
        stack.set_margin_top(6);

        // solid page
        let solid_btn = gtk::ColorButton::with_rgba(&crate::model::hex_to_rgba(&initial.solid));
        let solid_page = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        solid_page.append(&gtk::Label::new(Some("Colour")));
        solid_page.append(&solid_btn);
        stack.add_named(&solid_page, Some("solid"));

        // gradient page
        let grad_start = gtk::ColorButton::with_rgba(&crate::model::hex_to_rgba(&initial.gradient_start));
        let grad_end = gtk::ColorButton::with_rgba(&crate::model::hex_to_rgba(&initial.gradient_end));
        let grad_angle = gtk::SpinButton::with_range(0.0, 360.0, 1.0);
        grad_angle.set_value(initial.gradient_angle as f64);
        let grad_page = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for w in [
            gtk::Label::new(Some("Start")).upcast_ref::<gtk::Widget>(),
            grad_start.upcast_ref(),
            gtk::Label::new(Some("End")).upcast_ref(),
            grad_end.upcast_ref(),
            gtk::Label::new(Some("Angle")).upcast_ref(),
            grad_angle.upcast_ref(),
        ] {
            grad_page.append(w);
        }
        stack.add_named(&grad_page, Some("gradient"));

        // image page
        let image_entry = gtk::Entry::new();
        image_entry.set_editable(false);
        image_entry.set_hexpand(true);
        if let Some(p) = initial.image_path.as_ref() {
            image_entry.set_text(&p.to_string_lossy());
        }
        let browse = gtk::Button::with_label("Browse...");
        let image_fit = gtk::DropDown::from_strings(&["Fill", "Fit", "Center", "Tile"]);
        image_fit.set_selected(match initial.image_fit {
            ImageFit::Fill => 0,
            ImageFit::Fit => 1,
            ImageFit::Center => 2,
            ImageFit::Tile => 3,
        });
        let image_warning = gtk::Label::new(Some("Image file not found"));
        image_warning.add_css_class("warning");
        image_warning.set_xalign(0.0);
        image_warning.set_visible(false);

        let image_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&image_entry);
        row.append(&browse);
        image_page.append(&row);
        let fit_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        fit_row.append(&gtk::Label::new(Some("Fit")));
        fit_row.append(&image_fit);
        image_page.append(&fit_row);
        image_page.append(&image_warning);
        stack.add_named(&image_page, Some("image"));

        root.append(&stack);

        // radios drive the stack
        {
            let stack = stack.clone();
            let set = move |name: &str| stack.set_visible_child_name(name);
            let s = set.clone();
            radio_solid.connect_toggled(move |b| if b.is_active() { s("solid") });
            let s = set.clone();
            radio_gradient.connect_toggled(move |b| if b.is_active() { s("gradient") });
            let s = set.clone();
            radio_image.connect_toggled(move |b| if b.is_active() { s("image") });
        }

        // browse dialog
        {
            let entry = image_entry.clone();
            let warning = image_warning.clone();
            browse.connect_clicked(move |btn| {
                let dialog = gtk::FileDialog::builder().title("Choose a background image").build();
                let filter = gtk::FileFilter::new();
                filter.add_pixbuf_formats();
                filter.set_name(Some("Images"));
                let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                dialog.set_filters(Some(&filters));
                let entry = entry.clone();
                let warning = warning.clone();
                let win = btn.root().and_downcast::<gtk::Window>();
                dialog.open(win.as_ref(), gtk::gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res {
                        if let Some(path) = file.path() {
                            entry.set_text(&path.to_string_lossy());
                            warning.set_visible(!image_path_is_usable(Some(&path)));
                        }
                    }
                });
            });
        }

        let section = Self {
            root,
            radio_solid,
            radio_gradient,
            radio_image,
            solid_btn,
            grad_start,
            grad_end,
            grad_angle,
            image_entry,
            image_fit,
            image_warning,
        };
        section.set_mode(initial.mode);
        section
    }

    fn set_mode(&self, mode: BackgroundMode) {
        match mode {
            BackgroundMode::Solid => self.radio_solid.set_active(true),
            BackgroundMode::Gradient => self.radio_gradient.set_active(true),
            BackgroundMode::Image => self.radio_image.set_active(true),
        }
    }

    /// Show or hide the "image not found" warning.
    pub fn set_image_warning(&self, visible: bool) {
        self.image_warning.set_visible(visible);
    }

    /// Read every control back into a `Background`.
    pub fn read(&self) -> Background {
        let mode = if self.radio_image.is_active() {
            BackgroundMode::Image
        } else if self.radio_gradient.is_active() {
            BackgroundMode::Gradient
        } else {
            BackgroundMode::Solid
        };
        let text = self.image_entry.text().to_string();
        let image_path = (!text.is_empty()).then(|| std::path::PathBuf::from(text));
        let image_fit = match self.image_fit.selected() {
            1 => ImageFit::Fit,
            2 => ImageFit::Center,
            3 => ImageFit::Tile,
            _ => ImageFit::Fill,
        };
        Background {
            mode,
            solid: crate::model::rgba_to_hex(&self.solid_btn.rgba()),
            gradient_start: crate::model::rgba_to_hex(&self.grad_start.rgba()),
            gradient_end: crate::model::rgba_to_hex(&self.grad_end.rgba()),
            gradient_angle: self.grad_angle.value() as u16,
            image_path,
            image_fit,
        }
    }
}
```

If `gtk::ColorButton::with_rgba` / `.rgba()` are deprecated in 0.10, they still exist and compile with a warning; accept the warning or `#[allow(deprecated)]` on the module. Do not switch to `ColorDialogButton` unless it is available under `v4_10`.

- [ ] **Step 4: Run tests + build**

Run: `cargo test -p opusdm-desktop-config && cargo build -p opusdm-desktop-config`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/desktop-config/src/ui.rs crates/desktop-config/src/main.rs
git commit -m "feat(desktop-config): background section (solid / gradient / image)"
```

---

## Task 13: `opus-desktop-config` UI - Icon grid section

**Files:**
- Modify: `crates/desktop-config/src/ui.rs`
- Test: `crates/desktop-config/src/ui.rs` tests

**Interfaces:**
- Produces:
  - `pub struct GridSection { pub root: gtk::Box, /* ... */ }`
  - `impl GridSection { pub fn new(icon_size: i32, grid_spacing: i32) -> Self; pub fn icon_size(&self) -> i32; pub fn grid_spacing(&self) -> i32; }`
  - `const ICON_PRESETS: [(&str, i32); 3] = [("S", 48), ("M", 64), ("L", 96)];`
  - `const SPACING_PRESETS: [(&str, i32); 3] = [("S", 72), ("M", 96), ("L", 140)];`

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn grid_section_reports_seeded_then_edited_values() {
        if gtk::init().is_err() { return; }
        let s = GridSection::new(64, 96);
        assert_eq!(s.icon_size(), 64);
        assert_eq!(s.grid_spacing(), 96);
        s.set_icon_size_for_test(120);
        assert_eq!(s.icon_size(), 120);
    }
```

(`set_icon_size_for_test` is a `#[cfg(test)]` helper on the struct that calls `self.icon_spin.set_value(v as f64)`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p opusdm-desktop-config ui:: -- grid_section`
Expected: FAIL - `cannot find type GridSection`.

- [ ] **Step 3: Implement**

```rust
const ICON_PRESETS: [(&str, i32); 3] = [("S", 48), ("M", 64), ("L", 96)];
const SPACING_PRESETS: [(&str, i32); 3] = [("S", 72), ("M", 96), ("L", 140)];

/// The ICON GRID section: two spin buttons (icon size, grid spacing), each
/// with S/M/L preset buttons that fill the spin value.
pub struct GridSection {
    pub root: gtk::Box,
    icon_spin: gtk::SpinButton,
    spacing_spin: gtk::SpinButton,
}

impl GridSection {
    /// Build the section seeded from the current values.
    pub fn new(icon_size: i32, grid_spacing: i32) -> Self {
        let (imin, imax) = opusdm_ipc::ICON_SIZE_RANGE;
        let (smin, smax) = opusdm_ipc::GRID_SPACING_RANGE;

        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("ICON GRID"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let icon_spin = gtk::SpinButton::with_range(imin as f64, imax as f64, 1.0);
        icon_spin.set_value(icon_size.clamp(imin, imax) as f64);
        root.append(&labelled_preset_row("Icon size", &icon_spin, &ICON_PRESETS));

        let spacing_spin = gtk::SpinButton::with_range(smin as f64, smax as f64, 1.0);
        spacing_spin.set_value(grid_spacing.clamp(smin, smax) as f64);
        root.append(&labelled_preset_row("Grid spacing", &spacing_spin, &SPACING_PRESETS));

        Self { root, icon_spin, spacing_spin }
    }

    /// Current icon size in pixels.
    pub fn icon_size(&self) -> i32 {
        self.icon_spin.value() as i32
    }

    /// Current grid spacing in pixels.
    pub fn grid_spacing(&self) -> i32 {
        self.spacing_spin.value() as i32
    }

    #[cfg(test)]
    fn set_icon_size_for_test(&self, v: i32) {
        self.icon_spin.set_value(v as f64);
    }
}

/// A row: a label, a spin button, and preset buttons that set the spin value.
fn labelled_preset_row(
    label: &str,
    spin: &gtk::SpinButton,
    presets: &'static [(&'static str, i32)],
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&gtk::Label::new(Some(label)));
    row.append(spin);
    for (name, value) in presets {
        let b = gtk::Button::with_label(name);
        b.add_css_class("flat");
        let spin = spin.clone();
        let value = *value;
        b.connect_clicked(move |_| spin.set_value(value as f64));
        row.append(&b);
    }
    row
}
```

- [ ] **Step 4: Run tests + build**

Run: `cargo test -p opusdm-desktop-config && cargo build -p opusdm-desktop-config`
Expected: PASS (grid test may SKIP headless).

- [ ] **Step 5: Commit**

```bash
git add crates/desktop-config/src/ui.rs
git commit -m "feat(desktop-config): icon grid section with size and spacing presets"
```

---

## Task 14: `opus-desktop-config` UI - Desktop folder section + window assembly

**Files:**
- Modify: `crates/desktop-config/src/ui.rs`
- Modify: `crates/desktop-config/src/main.rs`
- Test: `crates/desktop-config/src/ui.rs` tests

**Interfaces:**
- Produces:
  - `pub struct FolderSection { pub root: gtk::Box, /* ... */ }`
  - `impl FolderSection { pub fn new(override_dir: Option<&std::path::Path>) -> Self; pub fn override_dir(&self) -> Option<std::path::PathBuf>; }`
    (`override_dir()` returns `None` when the entry is empty / reset.)
  - In `main.rs`: the full window with the three sections and a Cancel / Apply / Save button row calling `model::apply`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn folder_section_reports_none_when_empty_and_some_when_set() {
        if gtk::init().is_err() { return; }
        let s = FolderSection::new(None);
        assert_eq!(s.override_dir(), None);
        let s = FolderSection::new(Some(std::path::Path::new("/home/u/Desk")));
        assert_eq!(s.override_dir(), Some(std::path::PathBuf::from("/home/u/Desk")));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p opusdm-desktop-config ui:: -- folder_section`
Expected: FAIL - `cannot find type FolderSection`.

- [ ] **Step 3: Implement `FolderSection`**

```rust
/// The DESKTOP FOLDER section: an override path with Browse and Reset.
/// An empty entry means "no override - use the XDG/~ resolution".
pub struct FolderSection {
    pub root: gtk::Box,
    entry: gtk::Entry,
}

impl FolderSection {
    /// Build the section. `override_dir` seeds the entry; `None` leaves it
    /// empty (the placeholder shows the default behaviour).
    pub fn new(override_dir: Option<&std::path::Path>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("DESKTOP FOLDER"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let entry = gtk::Entry::new();
        entry.set_editable(false);
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("Default ($XDG_DESKTOP_DIR, then ~/Desktop)"));
        if let Some(p) = override_dir {
            entry.set_text(&p.to_string_lossy());
        }

        let browse = gtk::Button::with_label("Browse...");
        let reset = gtk::Button::with_label("Reset to default");
        {
            let entry = entry.clone();
            browse.connect_clicked(move |btn| {
                let dialog = gtk::FileDialog::builder().title("Choose the desktop folder").build();
                let entry = entry.clone();
                let win = btn.root().and_downcast::<gtk::Window>();
                dialog.select_folder(win.as_ref(), gtk::gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res {
                        if let Some(path) = file.path() {
                            entry.set_text(&path.to_string_lossy());
                        }
                    }
                });
            });
        }
        {
            let entry = entry.clone();
            reset.connect_clicked(move |_| entry.set_text(""));
        }

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&entry);
        row.append(&browse);
        row.append(&reset);
        root.append(&row);
        Self { root, entry }
    }

    /// The chosen override, or `None` when the entry is empty.
    pub fn override_dir(&self) -> Option<std::path::PathBuf> {
        let t = self.entry.text().to_string();
        (!t.is_empty()).then(|| std::path::PathBuf::from(t))
    }
}
```

- [ ] **Step 4: Assemble the window in `main.rs`**

Replace `build_window`:

```rust
use opusdm_ipc::DesktopSettings;

/// Build and show the single configuration window.
fn build_window(app: &gtk::Application) {
    let settings = model::load();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Desktop")
        .default_width(480)
        .default_height(600)
        .build();

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    outer.set_margin_top(12);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);

    let bg = std::rc::Rc::new(ui::BackgroundSection::new(&settings.background));
    let grid = std::rc::Rc::new(ui::GridSection::new(settings.icon_size, settings.grid_spacing));
    let folder = std::rc::Rc::new(ui::FolderSection::new(settings.desktop_dir.as_deref()));

    outer.append(&bg.root);
    outer.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    outer.append(&grid.root);
    outer.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    outer.append(&folder.root);

    let note = gtk::Label::new(None);
    note.set_xalign(0.0);
    note.set_visible(false);
    outer.append(&note);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&apply);
    buttons.append(&save);
    outer.append(&buttons);

    window.set_child(Some(&outer));

    let collect = {
        let bg = bg.clone();
        let grid = grid.clone();
        let folder = folder.clone();
        move || DesktopSettings {
            background: bg.read(),
            icon_size: grid.icon_size(),
            grid_spacing: grid.grid_spacing(),
            desktop_dir: folder.override_dir(),
        }
        .sanitized()
    };

    let run_apply = {
        let note = note.clone();
        let bg = bg.clone();
        move |settings: &DesktopSettings| {
            bg.set_image_warning(
                settings.background.mode == opusdm_ipc::BackgroundMode::Image
                    && !ui::image_path_is_usable(settings.background.image_path.as_deref()),
            );
            match model::apply(settings) {
                model::ApplyOutcome::SentToHub => {
                    note.set_visible(false);
                }
                model::ApplyOutcome::WroteFile => {
                    note.set_text("OpusDM non in esecuzione: le modifiche si applicheranno al prossimo avvio.");
                    note.set_visible(true);
                }
                model::ApplyOutcome::Failed(e) => {
                    note.set_text(&format!("Impossibile salvare: {e}"));
                    note.set_visible(true);
                }
            }
        }
    };

    {
        let collect = collect.clone();
        let run_apply = run_apply.clone();
        apply.connect_clicked(move |_| run_apply(&collect()));
    }
    {
        let collect = collect.clone();
        let run_apply = run_apply.clone();
        let window = window.clone();
        save.connect_clicked(move |_| {
            run_apply(&collect());
            window.close();
        });
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }

    window.present();
}
```

`collect` / `run_apply` are `Fn` closures used more than once - wrap them in `std::rc::Rc` or make them `Clone` via `clone!`-style manual clones as shown (they capture `Rc` handles, so a plain `move ||` that is `.clone()`d works only if the closure itself is `Clone`; simplest is to make `collect` and `run_apply` `Rc<dyn Fn...>`). Adjust: define

```rust
    let collect: std::rc::Rc<dyn Fn() -> DesktopSettings> = std::rc::Rc::new({ /* body */ });
    let run_apply: std::rc::Rc<dyn Fn(&DesktopSettings)> = std::rc::Rc::new({ /* body */ });
```

and `.clone()` the `Rc` into each button handler.

- [ ] **Step 5: Run tests + build**

Run: `cargo test -p opusdm-desktop-config && cargo build --workspace`
Expected: PASS.

- [ ] **Step 6: Manual end-to-end (needs a display)**

```bash
cargo build --release --workspace
./run-x11.sh
```

Inside the sandbox: right-click the desktop -> Desktop -> Settings. In the app:
- switch background to Gradient, set two colours + angle, Apply -> the desktop repaints immediately.
- switch to Image, browse to a PNG, choose each Fit mode, Apply -> repaints each time; a bogus path shows the warning and the desktop keeps the solid colour.
- change icon size / spacing, Apply -> the desktop icons resize and re-space.
- set the desktop folder to another directory with files, Apply -> the icons reload from it; add/remove a file in that dir -> the watch reloads it.
- close the hub, run `opus-desktop-config` from a terminal, change a value, Save -> the note about "prossimo avvio" appears and `~/.config/opusdm/desktop.json` is updated; restart the hub -> the change is visible.

- [ ] **Step 7: Commit**

```bash
git add crates/desktop-config/src/ui.rs crates/desktop-config/src/main.rs
git commit -m "feat(desktop-config): desktop folder section and window assembly"
```

---

## Task 15: Documentation + full-workspace verification

**Files:**
- Modify: `CHANGES.md`
- Modify: `docs/usage.md` (if it documents the desktop context menu or settings - check)

- [ ] **Step 1: Full workspace test**

Run: `cargo test --workspace`
Expected: PASS (headless-skipped GTK tests are acceptable; note the count).

Run: `cargo build --workspace --release`
Expected: PASS. Confirm `target/release/opus-desktop-config` exists.

Run: `cargo clippy --workspace --all-targets` (if clippy is available)
Expected: no new warnings in the touched crates. Fix any that are trivial (`#[allow(deprecated)]` on the colour-button module is acceptable and called out in Task 12).

- [ ] **Step 2: Update `CHANGES.md`**

Add a dated section at the top (match the file's Italian style and its `##  YYYY-MM-DD` heading format). Cover:
- config: nuovo `~/.config/opusdm/desktop.json`, gemello di `settings.json`, scritto dall'hub;
- stato per-cartella spostato da `.opusdm-layout` a `.opusdm/layout` (dir `.opusdm/` per cartella);
- sfondo del desktop configurabile: tinta unita / gradiente (2 colori + angolo) / immagine (Fill/Fit/Center/Tile), reso dall'hub via CSS rigenerato;
- `IconField::set_grid_spacing` per il passo della griglia del desktop; l'hub applica dimensione icona + spaziatura da `DesktopSettings`;
- cartella desktro sovrascrivibile (`desktop_dir`), con re-targeting live del `DesktopWatch`;
- nuovo `ToHub::SetDesktopSettings` e `HubEvent::DesktopSettingsChanged`;
- nuova app `opus-desktop-config` (crate `crates/desktop-config`), lanciata da "Desktop -> Settings" nel menu contestuale; scrive il file direttamente se l'hub non gira;
- limite noto: se all'avvio non c'e cartella desktop, cambiarla dall'app non crea il campo icone finche l'hub non riparte.

- [ ] **Step 3: Check `docs/usage.md`**

`grep -n -i "desktop\|settings" docs/usage.md`. If it lists the desktop context menu or the hub Settings dialog, add a short paragraph on `opus-desktop-config` and the background options. If it does not mention them, skip.

- [ ] **Step 4: Commit**

```bash
git add CHANGES.md docs/usage.md
git commit -m "docs: changelog for opus-desktop-config and desktop background"
```

---

## Self-Review

**1. Spec coverage**

| Spec section | Task(s) |
|---|---|
| 1. Per-folder `.opusdm/` | Task 4 |
| 2. `DesktopSettings` model, ranges, `sanitized`, defaults | Task 1 |
| 2. `background_css` helper + path->URL | Task 2 |
| 3. `desktop.json` file + `desktop_settings.rs`, hub sole writer | Task 6, Task 11 (app-side fallback writer) |
| 4. IPC `ToHub::SetDesktopSettings`, no new `ToLister` | Task 3 |
| 4. server load + handle + `HubEvent::DesktopSettingsChanged` | Task 7 |
| 5. `install_desktop_css(&Background)` + pure helper, startup + live | Task 8 |
| 5. `IconField::set_grid_spacing` -> `FreeArrange.cell` | Task 5 |
| 5. live desktop-dir re-resolve + watch rebuild | Task 9 |
| 5. context-menu "Settings" launches the app | Task 10 |
| 6. new crate, `main.rs` / `model.rs` / `ui.rs`, three sections, Cancel/Apply/Save, no-hub note | Tasks 11-14 |
| Error handling (malformed file, clamps, bad image path, missing override dir, no hub, spawn failure) | Task 1 (clamps), Task 6/11 (malformed->default), Task 2/8 (image fallback), Task 9 (missing override), Task 11/14 (no hub note), Task 10 (spawn failure) |
| Testing (ipc serde/back-compat/clamp, css per mode, url encoding, GTK-parse, scan `.opusdm`, `set_grid_spacing` pitch, desktop_settings roundtrip, dir resolution, server emit, manual E2E) | Tasks 1-14 as listed per task |
| Files touched list | matches "File Structure" above |

No gaps.

**2. Placeholder scan**

- No "TBD"/"TODO"/"implement later" in task steps. Task 15 Step 3 is a conditional ("if it mentions... else skip") with both branches specified - acceptable.
- Every code step has a code block.
- "Adjust names to the real ones" notes (Tasks 5, 7) point at concrete existing test harnesses the implementer must read; the surrounding code is fully specified. Acceptable because the harness names cannot be known without reading the file and the plan says exactly which file and block.

**3. Type consistency**

- `DesktopSettings` / `Background` / `BackgroundMode` / `ImageFit` field and variant names identical across Tasks 1, 2, 3, 6, 7, 8, 9, 11, 12, 13, 14.
- `background_css` (ipc) vs `desktop_rule_css` (hub, full stylesheet) - two distinct names, distinct jobs, used consistently.
- `HubEvent::DesktopSettingsChanged(DesktopSettings)` - same shape in Tasks 7, 8, 9.
- `ToHub::SetDesktopSettings(DesktopSettings)` - same in Tasks 3, 7, 11.
- `IconField::set_grid_spacing(i32)` - Task 5 defines, Task 8 calls.
- `effective_desktop_dir(Option<&Path>) -> Option<PathBuf>` - Task 9 defines and calls.
- `model::apply` / `ApplyOutcome::{SentToHub, WroteFile, Failed}` - Task 11 defines, Task 14 matches all three arms.
- `ui::{BackgroundSection, GridSection, FolderSection}` with `.root` fields and reader methods - Tasks 12-14 define, Task 14 `main.rs` consumes `bg.read()`, `grid.icon_size()`, `grid.grid_spacing()`, `folder.override_dir()`, `bg.set_image_warning(..)`, `ui::image_path_is_usable(..)` - all defined.
- `ICON_SIZE_RANGE` / `GRID_SPACING_RANGE` / `GRADIENT_ANGLE_MAX` used in Tasks 1, 6, 7, 13 with the same tuple/const shape.

Consistent.
