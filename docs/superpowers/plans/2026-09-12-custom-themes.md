# Custom Themes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users install icon/colour/background/behavior themes as archives (`.zip`, `.tar.gz`, `.tar.bz2`) imported through `opusdm-theme`, extracted to `~/.config/opusdm/themes/<name>/`, replacing the current compiled-in `BreezeDark` icon set with an equivalent importable package.

**Architecture:** `opusdm-ipc::theme::IconSetId` shrinks to `{None, Custom(String)}`. A new `opusdm-ipc::themes` module owns theme-dir discovery and parsing (`scan_themes`, `load_theme_data`) - pure data, no GTK. `opusdm-iconview::icon_set` drops its compiled-in Breeze Dark bytes and gains an on-demand, memoized loader for `Custom` icon sets. `opusdm-theme` (GTK binary) gains an archive importer module (zip/tar extraction with safety checks) and an apply-flow merge function that layers a selected custom theme's data over the user's current `Settings`/`DesktopSettings`, changing only the fields the theme specifies.

**Tech Stack:** Rust, GTK4, serde/serde_json, `zip` + `tar`/`flate2`/`bzip2` (new deps, `opusdm-theme-config` crate only), `tempfile` (already a dev-dep, becomes a runtime dep of the importer).

**Spec:** `docs/superpowers/specs/2026-09-12-custom-themes-design.md`

## Global Constraints

- `theme.json`/`manifest.json` fields are all optional except `manifest.json`'s `name` and `label` - use `#[serde(default)]` throughout, matching the existing `Settings`/`DesktopSettings` pattern.
- `manifest.json`'s `name` must match `^[a-z0-9-]+$` and be non-empty; `label` must be non-empty.
- Applying a custom theme is a **partial overwrite**: a field absent from `theme.json` leaves the user's current value untouched (colours, appearance, behavior, desktop metrics alike). This is the same "allow partial, missing falls back" principle already agreed for icons.
- Theme import is all-or-nothing: extraction happens in a temp dir first; `~/.config/opusdm/themes/` is only touched on full success, and only by replacing any existing `<name>/` wholesale.
- No new IPC message types - reuse existing `ToHub::SetTheme` and `ToHub::SetDesktopSettings`.
- `opusdm-ipc` and `opusdm-iconview` must NOT gain the `zip`/`tar`/`flate2`/`bzip2` dependencies - those stay confined to `opusdm-theme-config`.

---

## Task 1: `IconSetId` shrinks to `None`/`Custom`, with backward compat

**Files:**
- Modify: `crates/ipc/src/theme.rs`

**Interfaces:**
- Produces: `IconSetId::None`, `IconSetId::Custom(String)`; `IconSetId::ALL: [IconSetId; 1]`; `IconSetId::id(self) -> String`; `IconSetId::label(&self) -> String`; `IconSetId::from_id(id: &str) -> Option<IconSetId>`; `IconSetId::default() -> IconSetId` (still `None`).

- [ ] **Step 1: Write the failing tests**

Replace the existing `theme.rs` test module with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_set_ids_are_unique_and_round_trip() {
        let mut ids: Vec<String> = IconSetId::ALL.iter().map(|s| s.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate icon set id");
        for s in IconSetId::ALL {
            assert_eq!(IconSetId::from_id(&s.id()), Some(s));
        }
        assert_eq!(IconSetId::from_id("nonsense"), None);
    }

    #[test]
    fn custom_ids_round_trip_through_the_custom_prefix() {
        let custom = IconSetId::Custom("breeze-dark".to_string());
        assert_eq!(custom.id(), "custom:breeze-dark");
        assert_eq!(IconSetId::from_id("custom:breeze-dark"), Some(custom));
        assert_eq!(IconSetId::from_id("custom:"), Some(IconSetId::Custom(String::new())));
    }

    #[test]
    fn a_pre_migration_breeze_dark_tag_deserializes_to_none() {
        let v: IconSetId = serde_json::from_str("\"BreezeDark\"").unwrap();
        assert_eq!(v, IconSetId::None);
    }

    #[test]
    fn theme_default_has_no_icon_set_and_default_colors() {
        let t = Theme::default();
        assert_eq!(t.icon_set, IconSetId::None);
        assert_eq!(t.colors, crate::proto::ColorSettings::default());
        assert_eq!(t.background, crate::desktop::Background::default());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p opusdm-ipc theme::`
Expected: compile error (`IconSetId::Custom` and `IconSetId::ALL: [IconSetId; 1]` don't exist yet; `id()`/`from_id()` still return `&'static str`).

- [ ] **Step 3: Rewrite `IconSetId`**

Replace the whole enum and its impls in `crates/ipc/src/theme.rs` with:

```rust
//! Icon set choice and the `Theme` bundle: icon set + colours + desktop
//! background, edited together by `opusdm-theme` and applied to the running
//! desktop in one atomic `ToHub::SetTheme`.

use crate::desktop::Background;
use crate::proto::ColorSettings;
use serde::{Deserialize, Serialize};

/// Which icon library the Lister and desktop icon grid use. No icon set
/// ships compiled into the binary: `None` falls back to glyphs, and every
/// other option is a user-imported theme under `~/.config/opusdm/themes/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconSetId {
    /// No icon images: Nerd Font glyphs when a patched font is available, a
    /// plain name otherwise.
    None,
    /// A user-imported theme's icon set, identified by its `manifest.json`
    /// `name` (the theme directory's name under `~/.config/opusdm/themes/`).
    Custom(String),
    /// A pre-migration on-disk value naming the icon set that used to be
    /// compiled into the binary. Kept only so an old `settings.json` still
    /// deserializes; never constructed by current code. Treated as `None`
    /// everywhere it is read (see `Default`/consumers) - the equivalent
    /// theme is now importable as `breeze-dark.zip`, see
    /// `themes/breeze-dark/`.
    #[serde(rename = "BreezeDark")]
    #[deprecated(note = "compiled-in Breeze Dark is gone; import breeze-dark.zip instead")]
    LegacyBreezeDark,
}

impl IconSetId {
    /// Every icon set that exists independent of what's installed on disk.
    /// `Custom` entries are never part of this list - `opusdm-theme` builds
    /// its picker from `ALL` plus a live directory scan.
    #[allow(deprecated)]
    pub const ALL: [IconSetId; 1] = [IconSetId::None];

    /// Stable identifier stored in settings.json.
    #[allow(deprecated)]
    pub fn id(&self) -> String {
        match self {
            IconSetId::None => "none".to_string(),
            IconSetId::Custom(slug) => format!("custom:{slug}"),
            IconSetId::LegacyBreezeDark => "none".to_string(),
        }
    }

    /// Human-readable name for `opusdm-theme`'s picker. `Custom` variants
    /// are never shown via this method - the picker reads the theme's own
    /// `manifest.json` label instead.
    #[allow(deprecated)]
    pub fn label(&self) -> String {
        match self {
            IconSetId::None => "None (text glyphs)".to_string(),
            IconSetId::Custom(slug) => slug.clone(),
            IconSetId::LegacyBreezeDark => "None (text glyphs)".to_string(),
        }
    }

    /// Look an icon set up by its [`id`](Self::id). Accepts the `"custom:"`
    /// prefix for any slug, `"none"` for [`IconSetId::None`], `None` for
    /// anything else.
    pub fn from_id(id: &str) -> Option<IconSetId> {
        if id == "none" {
            return Some(IconSetId::None);
        }
        id.strip_prefix("custom:").map(|slug| IconSetId::Custom(slug.to_string()))
    }
}

impl Default for IconSetId {
    fn default() -> Self {
        IconSetId::None
    }
}

/// A theme bundle: icon set, Lister colours, and desktop background. What
/// `opusdm-theme` edits and applies to the hub as one unit via
/// `ToHub::SetTheme`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Theme {
    pub icon_set: IconSetId,
    pub colors: ColorSettings,
    pub background: Background,
}
```

Note `Theme` drops `Eq` (kept `PartialEq`) since `IconSetId::Custom(String)` still supports `Eq` fine actually - `String` is `Eq`, so `Eq` can stay. Keep `#[derive(..., PartialEq, Eq, ...)]` on both `IconSetId` and `Theme` as before (revert the `Eq` removal above - no field breaks `Eq`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p opusdm-ipc theme::`
Expected: PASS (4 tests: `icon_set_ids_are_unique_and_round_trip`, `custom_ids_round_trip_through_the_custom_prefix`, `a_pre_migration_breeze_dark_tag_deserializes_to_none`, `theme_default_has_no_icon_set_and_default_colors`).

- [ ] **Step 5: Fix the crate-wide build**

Run: `cargo build --workspace 2>&1 | head -100`

This will surface every other place `IconSetId::BreezeDark`, `IconSetId::ALL` (as `&'static str` labels), or `.id()`/`.label()` (as `&'static str`) is used. Do NOT fix those here - Tasks 4, 6 and 7 own those call sites specifically. For this task, only confirm `opusdm-ipc` itself builds:

Run: `cargo build -p opusdm-ipc`
Expected: success.

- [ ] **Step 6: Commit**

```bash
git add crates/ipc/src/theme.rs
git commit -m "feat(ipc): shrink IconSetId to None/Custom, drop compiled-in BreezeDark"
```

---

## Task 2: theme discovery module (`opusdm-ipc::themes`)

**Files:**
- Create: `crates/ipc/src/themes.rs`
- Modify: `crates/ipc/src/lib.rs` (add `pub mod themes;` and re-export the public types)

**Interfaces:**
- Consumes: `crate::proto::{ColorSettings, Appearance, Behavior}`, `crate::desktop::{BackgroundMode, ImageFit}` (all already `Serialize + Deserialize + Default`).
- Produces: `pub struct ThemeManifest { pub name: String, pub label: String, pub author: Option<String>, pub version: Option<String> }`; `pub struct CustomThemeSummary { pub slug: String, pub label: String, pub preview_path: Option<PathBuf> }`; `pub struct ThemeDesktopMetrics { pub icon_size: Option<i32>, pub grid_spacing: Option<i32>, pub grid_gap: Option<i32> }`; `pub struct ThemeBackground { pub mode: Option<BackgroundMode>, pub solid: Option<String>, pub gradient_start: Option<String>, pub gradient_end: Option<String>, pub gradient_angle: Option<u16>, pub image: Option<String>, pub image_fit: Option<ImageFit> }`; `pub struct CustomThemeData { pub colors: Option<ColorSettings>, pub appearance: Option<Appearance>, pub behavior: Option<Behavior>, pub desktop: Option<ThemeDesktopMetrics>, pub background: Option<ThemeBackground> }`; `pub fn themes_dir() -> PathBuf`; `pub fn valid_slug(s: &str) -> bool`; `pub fn scan_themes() -> Vec<CustomThemeSummary>`; `pub fn load_theme_data(slug: &str) -> CustomThemeData` (never `None` - an all-`None`-fields value on any read failure, matching `#[serde(default)]` tolerance); `pub fn theme_dir(slug: &str) -> PathBuf`; `pub fn background_image_path(slug: &str, filename: &str) -> PathBuf`.

- [ ] **Step 1: Write the failing tests**

```rust
// bottom of crates/ipc/src/themes.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_config_home() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        td
    }

    fn write_theme(root: &std::path::Path, slug: &str, manifest: &str, theme_json: Option<&str>) {
        let dir = root.join("opusdm").join("themes").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
        if let Some(t) = theme_json {
            std::fs::write(dir.join("theme.json"), t).unwrap();
        }
    }

    #[test]
    fn valid_slug_rules() {
        assert!(valid_slug("breeze-dark"));
        assert!(valid_slug("a1"));
        assert!(!valid_slug(""));
        assert!(!valid_slug("Breeze-Dark"));
        assert!(!valid_slug("has/slash"));
        assert!(!valid_slug("has.dot"));
        assert!(!valid_slug("has space"));
    }

    #[test]
    fn scan_themes_finds_valid_dirs_and_sorts_by_label() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(td.path(), "zeta", r#"{"name":"zeta","label":"Zeta Theme"}"#, None);
        write_theme(td.path(), "alpha", r#"{"name":"alpha","label":"Alpha Theme"}"#, None);
        let found = scan_themes();
        let labels: Vec<&str> = found.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, vec!["Alpha Theme", "Zeta Theme"]);
    }

    #[test]
    fn scan_themes_skips_missing_or_invalid_manifests() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(td.path(), "good", r#"{"name":"good","label":"Good"}"#, None);
        write_theme(td.path(), "no-label", r#"{"name":"no-label","label":""}"#, None);
        write_theme(td.path(), "bad-name", r#"{"name":"Bad Name","label":"x"}"#, None);
        std::fs::create_dir_all(td.path().join("opusdm").join("themes").join("no-manifest")).unwrap();
        let found = scan_themes();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].slug, "good");
    }

    #[test]
    fn scan_themes_on_missing_dir_is_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _td = with_config_home();
        assert!(scan_themes().is_empty());
    }

    #[test]
    fn load_theme_data_full_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(
            td.path(),
            "full",
            r#"{"name":"full","label":"Full"}"#,
            Some(r#"{"colors":{"file_fg":"#abcdef"},"desktop":{"icon_size":80},"background":{"mode":"image","image":"bg.jpg"}}"#),
        );
        let data = load_theme_data("full");
        assert_eq!(data.colors.unwrap().file_fg, "#abcdef");
        assert_eq!(data.desktop.unwrap().icon_size, Some(80));
        assert_eq!(data.background.unwrap().image.as_deref(), Some("bg.jpg"));
    }

    #[test]
    fn load_theme_data_missing_file_is_all_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = with_config_home();
        write_theme(td.path(), "icons-only", r#"{"name":"icons-only","label":"Icons Only"}"#, None);
        let data = load_theme_data("icons-only");
        assert!(data.colors.is_none());
        assert!(data.appearance.is_none());
        assert!(data.behavior.is_none());
        assert!(data.desktop.is_none());
        assert!(data.background.is_none());
    }

    #[test]
    fn background_image_path_join() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _td = with_config_home();
        let p = background_image_path("mytheme", "wall.png");
        assert!(p.ends_with("themes/mytheme/backgrounds/wall.png"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p opusdm-ipc themes::`
Expected: FAIL to compile (`themes` module doesn't exist).

- [ ] **Step 3: Write the module**

```rust
// crates/ipc/src/themes.rs
//! Discovery and parsing of user-installed themes under
//! `~/.config/opusdm/themes/<slug>/`. Pure data - no GTK, no archive
//! extraction (that lives in `opusdm-theme-config`, the only binary that
//! imports archives).

use crate::desktop::{BackgroundMode, ImageFit};
use crate::proto::{Appearance, Behavior, ColorSettings};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `~/.config/opusdm/themes`. Same `dirs::config_dir()` resolution used
/// throughout the app.
pub fn themes_dir() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("themes");
    path
}

/// The directory for one installed theme.
pub fn theme_dir(slug: &str) -> PathBuf {
    themes_dir().join(slug)
}

/// A background image's path inside a theme's `backgrounds/` folder.
pub fn background_image_path(slug: &str, filename: &str) -> PathBuf {
    theme_dir(slug).join("backgrounds").join(filename)
}

/// A valid theme slug / `manifest.json` `name`: non-empty, lowercase
/// ASCII letters, digits and `-` only.
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `manifest.json`'s shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeManifest {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

impl ThemeManifest {
    fn is_valid(&self) -> bool {
        valid_slug(&self.name) && !self.label.trim().is_empty()
    }
}

/// One theme as `opusdm-theme`'s picker shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomThemeSummary {
    pub slug: String,
    pub label: String,
    pub preview_path: Option<PathBuf>,
}

/// List every valid installed theme, sorted by display label. Skips (and
/// logs) any subdirectory with a missing, unparseable, or invalid
/// `manifest.json`. An unreadable or absent `themes_dir()` yields an empty
/// list, not an error.
pub fn scan_themes() -> Vec<CustomThemeSummary> {
    let dir = themes_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        let Ok(content) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<ThemeManifest>(&content) else {
            eprintln!("opusdm: {} has an unparseable manifest.json", path.display());
            continue;
        };
        if !manifest.is_valid() {
            eprintln!("opusdm: {} has an invalid manifest.json (name/label)", path.display());
            continue;
        }
        let preview = path.join("preview.png");
        found.push(CustomThemeSummary {
            slug: manifest.name,
            label: manifest.label,
            preview_path: preview.is_file().then_some(preview),
        });
    }
    found.sort_by(|a, b| a.label.cmp(&b.label));
    found
}

/// `theme.json`'s `desktop` section: a subset of `DesktopSettings`,
/// deliberately excluding `desktop_dir` (an environment fact, not a look).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeDesktopMetrics {
    pub icon_size: Option<i32>,
    pub grid_spacing: Option<i32>,
    pub grid_gap: Option<i32>,
}

/// `theme.json`'s `background` section: mirrors
/// [`crate::desktop::Background`] but `image` is a filename relative to the
/// theme's `backgrounds/` folder, resolved to an absolute path only when
/// applying the theme.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeBackground {
    pub mode: Option<BackgroundMode>,
    pub solid: Option<String>,
    pub gradient_start: Option<String>,
    pub gradient_end: Option<String>,
    pub gradient_angle: Option<u16>,
    pub image: Option<String>,
    pub image_fit: Option<ImageFit>,
}

/// `theme.json`'s full shape. Every field optional - a theme can supply
/// only icons and nothing else.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomThemeData {
    pub colors: Option<ColorSettings>,
    pub appearance: Option<Appearance>,
    pub behavior: Option<Behavior>,
    pub desktop: Option<ThemeDesktopMetrics>,
    pub background: Option<ThemeBackground>,
}

/// Parse `<slug>/theme.json`. A missing or unparseable file yields
/// [`CustomThemeData::default`] (every field `None`), the same tolerance
/// `Settings` already has for a hand-edited `settings.json`.
pub fn load_theme_data(slug: &str) -> CustomThemeData {
    let path = theme_dir(slug).join("theme.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p opusdm-ipc themes::`
Expected: PASS (8 tests).

- [ ] **Step 5: Export the module**

In `crates/ipc/src/lib.rs`, add near the other `pub mod` declarations:

```rust
pub mod themes;
```

Check the existing file first (`grep -n "^pub mod" crates/ipc/src/lib.rs`) to match the existing re-export style (some modules are flattened with `pub use module::*;`, some stay namespaced as `crate::theme::IconSetId`). Match whatever convention `crate::theme::Theme` already follows - if `theme.rs` is flattened at the crate root (i.e. `opusdm_ipc::Theme` works, not just `opusdm_ipc::theme::Theme`), do the same for `themes` so callers can write `opusdm_ipc::themes::scan_themes()`.

- [ ] **Step 6: Run the ipc crate's full test suite**

Run: `cargo test -p opusdm-ipc`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/ipc/src/themes.rs crates/ipc/src/lib.rs
git commit -m "feat(ipc): add theme discovery/parsing module"
```

---

## Task 3: `opusdm-iconview` icon lookup rewrite (Custom loader, drop compiled Breeze Dark)

**Files:**
- Modify: `crates/iconview/src/icon_set.rs`
- Delete: `crates/iconview/assets/breeze-dark/*.png` (moved in Task 5, not deleted outright here - see note in Step 3)

**Interfaces:**
- Consumes: `opusdm_ipc::IconSetId`, `opusdm_ipc::themes::theme_dir`.
- Produces: `pub fn icon_bytes_for(kind: IconKind, mode: u32, ext: Option<&str>, set: &IconSetId) -> Option<std::borrow::Cow<'static, [u8]>>` (note: `set` becomes a reference since `IconSetId` no longer implements `Copy` - it now owns a `String` in `Custom`).

- [ ] **Step 1: Write the failing tests**

Replace `icon_set.rs`'s test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::IconSetId;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn none_set_returns_nothing() {
        assert!(icon_bytes_for(IconKind::Dir, 0, None, &IconSetId::None).is_none());
    }

    #[test]
    fn custom_set_returns_bytes_for_a_present_icon_and_none_for_an_absent_one() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let icons_dir = td.path().join("opusdm").join("themes").join("mytheme").join("icons");
        std::fs::create_dir_all(&icons_dir).unwrap();
        std::fs::write(icons_dir.join("dir.png"), b"fake-png-bytes").unwrap();

        let set = IconSetId::Custom("mytheme".to_string());
        let dir_icon = icon_bytes_for(IconKind::Dir, 0, None, &set);
        assert_eq!(dir_icon.as_deref(), Some(&b"fake-png-bytes"[..]));

        let file_icon = icon_bytes_for(IconKind::File, 0o100_644, None, &set);
        assert!(file_icon.is_none());
    }

    #[test]
    fn custom_set_with_no_theme_dir_returns_none_for_everything() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let set = IconSetId::Custom("does-not-exist".to_string());
        assert!(icon_bytes_for(IconKind::Dir, 0, None, &set).is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p opusdm-iconview icon_set::`
Expected: compile error (current signature takes `IconSetId` by value with `Copy`, returns `Option<&'static [u8]>`, has no `Custom` arm).

- [ ] **Step 3: Rewrite `icon_set.rs`**

```rust
//! Real icon image lookup, sibling of `glyphs::glyph_for`. Reuses the same
//! `IconKind` + extension classification; returns PNG bytes instead of a
//! Nerd Font `char`. No icon set is compiled into this crate - `None`
//! yields no bytes (caller falls back to the glyph) and every other set is
//! a user-imported theme loaded from disk on first use per process.

use crate::types::IconKind;
use opusdm_ipc::IconSetId;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const S_IFMT: u32 = 0o170_000;
const EXEC_BITS: u32 = 0o111;

const ARCHIVE_EXTS: &[&str] = &["zip", "tar", "gz", "bz2", "xz", "7z", "rar"];
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "svg", "webp"];
const AUDIO_EXTS: &[&str] = &["mp3", "flac", "ogg", "opus", "wav", "m4a"];
const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "avi", "mov", "webm"];
const CODE_EXTS: &[&str] = &[
    "rs", "py", "js", "mjs", "ts", "tsx", "jsx", "go", "java", "kt", "rb", "php", "c", "h",
    "cpp", "cc", "hpp", "html", "htm", "css", "scss", "json", "toml", "yaml", "yml", "ini",
    "conf",
];
const TEXT_EXTS: &[&str] = &["md", "txt"];

/// Which fixed icon filename a `(kind, mode, ext)` combination maps to.
fn icon_filename(kind: IconKind, mode: u32, ext: Option<&str>) -> &'static str {
    match kind {
        IconKind::Dir => "dir.png",
        IconKind::Symlink => "symlink.png",
        IconKind::Broken => "broken.png",
        IconKind::File => {
            if mode & EXEC_BITS != 0 && mode & S_IFMT != 0o140_000 {
                "exec.png"
            } else {
                match ext {
                    Some(e) if ARCHIVE_EXTS.contains(&e) => "archive.png",
                    Some(e) if IMAGE_EXTS.contains(&e) => "image.png",
                    Some(e) if AUDIO_EXTS.contains(&e) => "audio.png",
                    Some(e) if VIDEO_EXTS.contains(&e) => "video.png",
                    Some(e) if CODE_EXTS.contains(&e) => "code.png",
                    Some(e) if TEXT_EXTS.contains(&e) => "text.png",
                    Some("pdf") => "pdf.png",
                    _ => "file.png",
                }
            }
        }
    }
}

/// A custom theme's loaded icon bytes, keyed by fixed filename (e.g.
/// `"dir.png"`). Only files that exist on disk are present.
type CustomIcons = HashMap<&'static str, Vec<u8>>;

static CUSTOM_CACHE: OnceLock<Mutex<HashMap<String, std::sync::Arc<CustomIcons>>>> = OnceLock::new();

const ALL_ICON_FILENAMES: [&str; 12] = [
    "dir.png", "file.png", "symlink.png", "broken.png", "exec.png", "archive.png",
    "image.png", "audio.png", "video.png", "code.png", "text.png", "pdf.png",
];

/// Load (or fetch from the process-local cache) every present icon PNG for
/// `slug`'s `icons/` folder.
fn load_custom(slug: &str) -> std::sync::Arc<CustomIcons> {
    let cache = CUSTOM_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap();
    if let Some(existing) = guard.get(slug) {
        return existing.clone();
    }
    let icons_dir = opusdm_ipc::themes::theme_dir(slug).join("icons");
    let mut loaded = CustomIcons::new();
    for name in ALL_ICON_FILENAMES {
        if let Ok(bytes) = std::fs::read(icons_dir.join(name)) {
            loaded.insert(name, bytes);
        }
    }
    let arc = std::sync::Arc::new(loaded);
    guard.insert(slug.to_string(), arc.clone());
    arc
}

/// PNG bytes for `kind`/`mode`/`ext` under icon set `set`, or `None` when
/// `set` is [`IconSetId::None`] or the theme has no file for that icon
/// (caller falls back to `glyphs::glyph_for` in both cases).
pub fn icon_bytes_for(kind: IconKind, mode: u32, ext: Option<&str>, set: &IconSetId) -> Option<Cow<'static, [u8]>> {
    let slug = match set {
        IconSetId::None => return None,
        IconSetId::Custom(slug) => slug,
    };
    let filename = icon_filename(kind, mode, ext);
    let icons = load_custom(slug);
    icons.get(filename).map(|bytes| Cow::Owned(bytes.clone()))
}
```

Note: `match set { IconSetId::None => ..., IconSetId::Custom(slug) => ... }` is exhaustive only because Task 1 kept `#[allow(deprecated)]` `LegacyBreezeDark` out of pattern matches that don't need it - but since the enum now has 3 variants (`None`, `Custom`, `LegacyBreezeDark`), this match needs a third arm. Add:

```rust
        #[allow(deprecated)]
        IconSetId::LegacyBreezeDark => return None,
```

right after the `IconSetId::None => return None,` arm (restructure the `match` to a block with an early return for both `None` and `LegacyBreezeDark`, then continue with `Custom` below it), so the function still compiles and treats the legacy tag exactly like `None`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p opusdm-iconview icon_set::`
Expected: PASS (3 tests).

- [ ] **Step 5: Delete the compiled-in assets directory**

The `include_bytes!` calls are gone, so `crates/iconview/assets/breeze-dark/` is now unused by this crate. Move it (don't just delete) - Task 5 turns it into `themes/breeze-dark/`:

```bash
mkdir -p themes/breeze-dark/icons
git mv crates/iconview/assets/breeze-dark/archive.png themes/breeze-dark/icons/archive.png
git mv crates/iconview/assets/breeze-dark/audio.png themes/breeze-dark/icons/audio.png
git mv crates/iconview/assets/breeze-dark/broken.png themes/breeze-dark/icons/broken.png
git mv crates/iconview/assets/breeze-dark/code.png themes/breeze-dark/icons/code.png
git mv crates/iconview/assets/breeze-dark/dir.png themes/breeze-dark/icons/dir.png
git mv crates/iconview/assets/breeze-dark/exec.png themes/breeze-dark/icons/exec.png
git mv crates/iconview/assets/breeze-dark/file.png themes/breeze-dark/icons/file.png
git mv crates/iconview/assets/breeze-dark/image.png themes/breeze-dark/icons/image.png
git mv crates/iconview/assets/breeze-dark/pdf.png themes/breeze-dark/icons/pdf.png
git mv crates/iconview/assets/breeze-dark/symlink.png themes/breeze-dark/icons/symlink.png
git mv crates/iconview/assets/breeze-dark/text.png themes/breeze-dark/icons/text.png
git mv crates/iconview/assets/breeze-dark/video.png themes/breeze-dark/icons/video.png
rmdir crates/iconview/assets/breeze-dark crates/iconview/assets 2>/dev/null || true
```

(The final `rmdir` is best-effort - if `crates/iconview/assets/` holds anything else, it stays; check with `ls crates/iconview/assets/` first and only run the `rmdir` if it's now empty.)

- [ ] **Step 6: Commit**

```bash
git add -A crates/iconview themes/breeze-dark
git commit -m "feat(iconview): drop compiled-in icon bytes, add Custom theme loader"
```

---

## Task 4: fix icon-byte call sites for the new `Cow`/reference signature

**Files:**
- Modify: `crates/iconview/src/item.rs:35,61-71`
- Modify: `crates/lister/src/glyphs.rs:33-36,151-159`
- Modify: `crates/lister/src/columns.rs:164-165`

**Interfaces:**
- Consumes: `opusdm_iconview::icon_set::icon_bytes_for` (Task 3's new signature).
- Produces: `opusdm_lister::glyphs::icon_bytes_for_row(row: &Row, set: &opusdm_ipc::IconSetId) -> Option<std::borrow::Cow<'static, [u8]>>`.

- [ ] **Step 1: Update `crates/iconview/src/item.rs`**

Change the `new` signature (line 35) from `icon_set: opusdm_ipc::IconSetId` to `icon_set: &opusdm_ipc::IconSetId`, and the lookup block (lines 61-71) from:

```rust
        if let Some(bytes) =
            crate::icon_set::icon_bytes_for(entry.kind, entry.mode, entry.ext.as_deref(), icon_set)
        {
            let gbytes = glib::Bytes::from_static(bytes);
            if let Ok(texture) = gtk::gdk::Texture::from_bytes(&gbytes) {
```

to:

```rust
        if let Some(bytes) =
            crate::icon_set::icon_bytes_for(entry.kind, entry.mode, entry.ext.as_deref(), icon_set)
        {
            let gbytes = glib::Bytes::from_owned(bytes.into_owned());
            if let Ok(texture) = gtk::gdk::Texture::from_bytes(&gbytes) {
```

(`glib::Bytes::from_static` requires a `&'static [u8]`, which a `Cow::Owned` no longer is; `from_owned` takes any `impl AsRef<[u8]> + Send + Sync + 'static`, and `Vec<u8>` qualifies.)

Find every caller of `IconItem::new` (search: `grep -rn "IconItem::new" crates/`) and update the call site to pass a reference (`&icon_set` instead of `icon_set`) if it isn't already.

- [ ] **Step 2: Update `crates/lister/src/glyphs.rs`**

Change:

```rust
pub fn icon_bytes_for_row(row: &Row, set: opusdm_ipc::IconSetId) -> Option<&'static [u8]> {
    let ext = opusdm_iconview::IconEntry::ext_of(&row.name);
    opusdm_iconview::icon_set::icon_bytes_for(kind_of(row), row.mode, ext.as_deref(), set)
}
```

to:

```rust
pub fn icon_bytes_for_row(row: &Row, set: &opusdm_ipc::IconSetId) -> Option<std::borrow::Cow<'static, [u8]>> {
    let ext = opusdm_iconview::IconEntry::ext_of(&row.name);
    opusdm_iconview::icon_set::icon_bytes_for(kind_of(row), row.mode, ext.as_deref(), set)
}
```

And update its two tests (currently at the end of the file):

```rust
    #[test]
    fn icon_bytes_for_row_is_none_without_a_real_icon_set() {
        assert!(icon_bytes_for_row(&row("a.rs"), &opusdm_ipc::IconSetId::None).is_none());
    }

    #[test]
    fn icon_bytes_for_row_matches_the_extension() {
        let _guard = crate::glyphs::tests::ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let icons_dir = td.path().join("opusdm").join("themes").join("t").join("icons");
        std::fs::create_dir_all(&icons_dir).unwrap();
        std::fs::write(icons_dir.join("code.png"), b"bytes").unwrap();
        let set = opusdm_ipc::IconSetId::Custom("t".to_string());
        assert!(icon_bytes_for_row(&row("a.rs"), &set).is_some());
    }
```

This introduces a shared `ENV_LOCK` for `glyphs.rs`'s test module (mutating `XDG_CONFIG_HOME` is process-global) - add it near the top of the existing `#[cfg(test)] mod tests` block:

```rust
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

and make it `pub(crate)` if needed, or simply duplicate the guard pattern already used in `crates/ipc/src/themes.rs` (Task 2) and `crates/theme-config/src/model.rs` - each test module gets its own private `ENV_LOCK`, no cross-module sharing needed; drop the `crate::glyphs::tests::ENV_LOCK` reference above and just declare `static ENV_LOCK: Mutex<()> = Mutex::new(());` locally inside `glyphs.rs`'s own test module, add `use std::sync::Mutex;` to that module's imports.

Also add `tempfile.workspace = true` to `crates/lister/Cargo.toml`'s `[dev-dependencies]` if not already present (check first: `grep tempfile crates/lister/Cargo.toml`).

- [ ] **Step 3: Update `crates/lister/src/columns.rs:164-165`**

Change:

```rust
                if let Some(bytes) = crate::glyphs::icon_bytes_for_row(&row, icon_set) {
                    let gbytes = gtk::glib::Bytes::from_static(bytes);
```

to:

```rust
                if let Some(bytes) = crate::glyphs::icon_bytes_for_row(&row, &icon_set) {
                    let gbytes = gtk::glib::Bytes::from_owned(bytes.into_owned());
```

Check the surrounding function signature for how `icon_set` arrives here (`grep -n "icon_set" crates/lister/src/columns.rs`) - if it's already a reference, drop the extra `&`; if it's `opusdm_ipc::IconSetId` by value (it was `Copy` before, now isn't), change the parameter to take it by reference too and fix its own callers, following the reference chain up to wherever `IconSetId` first comes from (`Settings`/`Theme`) - it should be borrowed all the way down since it no longer implements `Copy`.

- [ ] **Step 4: Build and fix remaining `Copy`-removal fallout**

Run: `cargo build --workspace 2>&1 | head -150`

`IconSetId` losing `Copy` (because `Custom(String)` isn't `Copy`) will surface anywhere the old code did `icon_set` (moved/copied) instead of `&icon_set` or `icon_set.clone()`. Fix each site the compiler points at by preferring a borrow; only `.clone()` where a borrow can't reach (e.g. storing it in a struct field that outlives the borrow's scope).

- [ ] **Step 5: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: all PASS. (Some GTK-dependent tests may be skipped/no-op in a headless environment - that's pre-existing behavior, not something this task changes.)

- [ ] **Step 6: Commit**

```bash
git add -A crates/iconview crates/lister
git commit -m "fix: propagate Cow<[u8]> icon bytes and non-Copy IconSetId through call sites"
```

---

## Task 5: package Breeze Dark as `themes/breeze-dark/`

**Files:**
- Create: `themes/breeze-dark/manifest.json`
- Create: `xtask/src/main.rs` (new minimal binary crate for build-time packaging - see rationale below)
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces: a `cargo xtask package-themes` command that zips every `themes/<slug>/` into `target/themes/<slug>.zip`.

Rationale for an `xtask` crate over a shell script: the workspace already has no `Makefile`/build-script convention (checked: `ls Makefile* *.sh 2>/dev/null` at repo root returns nothing), and `xtask` is the standard cargo-only way to add a project command without a new external tool dependency (just `cargo run -p xtask -- package-themes`). Keep it minimal - one file, no dependencies beyond what's needed for zipping (reuse the `zip` crate this feature already adds in Task 9; add it to `xtask`'s own `Cargo.toml` rather than depending on `opusdm-theme-config` to avoid a workspace-graph cycle risk).

- [ ] **Step 1: Write `themes/breeze-dark/manifest.json`**

```bash
mkdir -p themes/breeze-dark
cat > themes/breeze-dark/manifest.json << 'EOF'
{
  "name": "breeze-dark",
  "label": "Breeze Dark"
}
EOF
```

- [ ] **Step 2: Write a failing test for the packaged theme's shape**

Add to `crates/ipc/src/themes.rs`'s test module (this crate already parses `ThemeManifest`, so it's the natural home for "does the shipped package look right" - it reads the repo-relative `themes/breeze-dark/` directly via `CARGO_MANIFEST_DIR`, not `themes_dir()`):

```rust
    #[test]
    fn the_shipped_breeze_dark_package_has_every_icon_and_a_valid_manifest() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let theme_dir = repo_root.join("themes").join("breeze-dark");
        let manifest: ThemeManifest =
            serde_json::from_str(&std::fs::read_to_string(theme_dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.name, "breeze-dark");
        assert!(manifest.is_valid());

        const EXPECTED: [&str; 12] = [
            "dir.png", "file.png", "symlink.png", "broken.png", "exec.png", "archive.png",
            "image.png", "audio.png", "video.png", "code.png", "text.png", "pdf.png",
        ];
        for name in EXPECTED {
            assert!(theme_dir.join("icons").join(name).is_file(), "missing icons/{name}");
        }
    }
```

`ThemeManifest::is_valid` is currently a private (`fn is_valid`) method - this test lives in the same module (`mod tests` inside `themes.rs`), so it can call it directly; no visibility change needed.

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p opusdm-ipc the_shipped_breeze_dark_package`
Expected: FAIL (`themes/breeze-dark/manifest.json` exists from Step 1, but `icons/` is still empty until Task 3 Step 5 moved the PNGs there - if Task 3 Step 5 was already done, this should actually PASS already; run it to confirm which).

- [ ] **Step 4: If it fails, fix it**

Confirm the 12 PNGs from Task 3 Step 5 landed at `themes/breeze-dark/icons/*.png` (`ls themes/breeze-dark/icons/`). If any are missing, re-run the `git mv` commands from Task 3 Step 5 for the missing ones.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p opusdm-ipc the_shipped_breeze_dark_package`
Expected: PASS.

- [ ] **Step 6: Add the `xtask` crate**

```toml
# xtask/Cargo.toml
[package]
name = "xtask"
version = "0.1.0"
edition = "2021"

[dependencies]
zip = "2"
walkdir = "2.5"
```

```rust
// xtask/src/main.rs
//! Repo maintenance commands run via `cargo run -p xtask -- <command>`.
//! Not part of the app's own build - `cargo build --workspace` does not
//! need this crate to succeed for the apps to run.

use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() {
    let cmd = std::env::args().nth(1);
    match cmd.as_deref() {
        Some("package-themes") => package_themes(),
        _ => {
            eprintln!("usage: cargo run -p xtask -- package-themes");
            std::process::exit(1);
        }
    }
}

/// Zip every `themes/<slug>/` directory into `target/themes/<slug>.zip`.
fn package_themes() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let themes_src = repo_root.join("themes");
    let out_dir = repo_root.join("target").join("themes");
    std::fs::create_dir_all(&out_dir).expect("create target/themes");

    for entry in std::fs::read_dir(&themes_src).expect("read themes/") {
        let entry = entry.expect("read themes/ entry");
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let slug = path.file_name().unwrap().to_string_lossy().to_string();
        let zip_path = out_dir.join(format!("{slug}.zip"));
        let file = File::create(&zip_path).expect("create zip file");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();

        for walk_entry in walkdir::WalkDir::new(&path).into_iter().filter_map(|e| e.ok()) {
            let full_path = walk_entry.path();
            let rel_path = full_path.strip_prefix(&path).unwrap();
            if rel_path.as_os_str().is_empty() {
                continue;
            }
            let rel_str = rel_path.to_string_lossy().replace('\\', "/");
            if full_path.is_dir() {
                zip.add_directory(format!("{rel_str}/"), options).expect("add dir");
            } else {
                zip.start_file(rel_str, options).expect("start file");
                let data = std::fs::read(full_path).expect("read theme file");
                zip.write_all(&data).expect("write zip entry");
            }
        }
        zip.finish().expect("finish zip");
        println!("wrote {}", zip_path.display());
    }
}
```

- [ ] **Step 7: Add `xtask` to the workspace and verify it runs**

In the root `Cargo.toml`'s `[workspace] members`, add `"xtask"`.

Run: `cargo run -p xtask -- package-themes`
Expected: prints `wrote <repo>/target/themes/breeze-dark.zip`.

Run: `unzip -l target/themes/breeze-dark.zip`
Expected: lists `manifest.json` and 12 files under `icons/`.

- [ ] **Step 8: Commit**

```bash
git add themes/breeze-dark/manifest.json xtask Cargo.toml
git commit -m "feat: package Breeze Dark as an importable theme via cargo xtask"
```

---

## Task 6: `opusdm-theme` picker lists installed custom themes

**Files:**
- Modify: `crates/theme-config/src/ui.rs:9-36` (`IconSetSection`)
- Modify: `crates/theme-config/src/main.rs:44` (construction call site)

**Interfaces:**
- Consumes: `opusdm_ipc::themes::scan_themes() -> Vec<CustomThemeSummary>`, `opusdm_ipc::IconSetId`.
- Produces: `IconSetSection::new(current: &IconSetId) -> Self`; `IconSetSection::value(&self) -> IconSetId` (now returns an owned `IconSetId`, since `Custom(String)` can't be indexed out of a `'static` array the way `IconSetId::ALL` was).

- [ ] **Step 1: Rewrite `IconSetSection`**

```rust
/// The ICON SET section.
pub struct IconSetSection {
    pub root: gtk::Box,
    dropdown: gtk::DropDown,
    /// Parallel to the dropdown's rows: `None` (index 0) then every
    /// installed custom theme, in the order shown.
    options: Vec<IconSetId>,
}

impl IconSetSection {
    pub fn new(current: &IconSetId) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("ICON SET"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let mut options: Vec<IconSetId> = IconSetId::ALL.to_vec();
        let mut labels: Vec<String> = options.iter().map(|s| s.label()).collect();
        for theme in opusdm_ipc::themes::scan_themes() {
            options.push(IconSetId::Custom(theme.slug));
            labels.push(theme.label);
        }
        let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let dropdown = gtk::DropDown::from_strings(&label_refs);
        let selected = options.iter().position(|s| s == current).unwrap_or(0);
        dropdown.set_selected(selected as u32);
        root.append(&dropdown);

        Self { root, dropdown, options }
    }

    /// The currently selected icon set.
    pub fn value(&self) -> IconSetId {
        self.options[self.dropdown.selected() as usize].clone()
    }
}
```

(`preview.png` thumbnails next to each row are a nice-to-have the spec allows omitting for a first cut - not included here to keep this task's GTK surface small; if wanted later, it's an additive change to this same function, not a redesign.)

- [ ] **Step 2: Fix the call site in `main.rs:44`**

Change:

```rust
    let icon_set = Rc::new(ui::IconSetSection::new(theme.icon_set));
```

to:

```rust
    let icon_set = Rc::new(ui::IconSetSection::new(&theme.icon_set));
```

- [ ] **Step 3: Build**

Run: `cargo build -p opusdm-theme-config`
Expected: success. (No unit test here - this is GTK widget wiring; correctness is covered by Task 7's pure merge-function tests plus manual verification in Task 10.)

- [ ] **Step 4: Commit**

```bash
git add crates/theme-config/src/ui.rs crates/theme-config/src/main.rs
git commit -m "feat(theme-config): list installed custom themes in the icon set picker"
```

---

## Task 7: apply-flow merge function (pure, testable)

**Files:**
- Create: `crates/theme-config/src/apply.rs`
- Modify: `crates/theme-config/src/main.rs` (add `mod apply;`)

**Interfaces:**
- Consumes: `opusdm_ipc::themes::{CustomThemeData, ThemeDesktopMetrics, ThemeBackground}`, `opusdm_ipc::{Theme, Settings, DesktopSettings, IconSetId}`, `opusdm_ipc::desktop::{Background, BackgroundMode, ImageFit}`.
- Produces: `pub struct MergedTheme { pub theme: Theme, pub appearance: opusdm_ipc::Appearance, pub behavior: opusdm_ipc::Behavior, pub desktop: DesktopSettings }`; `pub fn merge_custom_theme(slug: &str, data: &CustomThemeData, current: &Settings, current_desktop: &DesktopSettings) -> MergedTheme`.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/theme-config/src/apply.rs
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::desktop::{Background, BackgroundMode, ImageFit};
    use opusdm_ipc::themes::{CustomThemeData, ThemeBackground, ThemeDesktopMetrics};
    use opusdm_ipc::{Appearance, Behavior, ColorSettings, DesktopSettings, IconSetId, Settings};

    fn base_settings() -> Settings {
        let mut s = Settings::default();
        s.appearance.monospace_list = true;
        s.behavior.auto_refresh = true;
        s
    }

    fn base_desktop() -> DesktopSettings {
        let mut d = DesktopSettings::default();
        d.icon_size = 64;
        d.grid_spacing = 96;
        d.grid_gap = 4;
        d
    }

    #[test]
    fn empty_theme_data_changes_nothing_but_the_icon_set() {
        let data = CustomThemeData::default();
        let settings = base_settings();
        let desktop = base_desktop();
        let merged = merge_custom_theme("mytheme", &data, &settings, &desktop);

        assert_eq!(merged.theme.icon_set, IconSetId::Custom("mytheme".to_string()));
        assert_eq!(merged.theme.colors, settings.theme.colors);
        assert_eq!(merged.theme.background, settings.theme.background);
        assert_eq!(merged.appearance, settings.appearance);
        assert_eq!(merged.behavior, settings.behavior);
        assert_eq!(merged.desktop, desktop);
    }

    #[test]
    fn theme_supplied_colors_win_others_keep_current() {
        let mut data = CustomThemeData::default();
        data.colors = Some(ColorSettings { file_fg: "#ff00ff".to_string(), ..ColorSettings::default() });
        let settings = base_settings();
        let desktop = base_desktop();
        let merged = merge_custom_theme("t", &data, &settings, &desktop);

        assert_eq!(merged.theme.colors.file_fg, "#ff00ff");
        assert_eq!(merged.theme.colors.dir_fg, ColorSettings::default().dir_fg);
        assert_eq!(merged.appearance, settings.appearance);
    }

    #[test]
    fn theme_supplied_desktop_metrics_overwrite_only_the_fields_set() {
        let mut data = CustomThemeData::default();
        data.desktop = Some(ThemeDesktopMetrics { icon_size: Some(128), grid_spacing: None, grid_gap: None });
        let desktop = base_desktop();
        let merged = merge_custom_theme("t", &data, &Settings::default(), &desktop);

        assert_eq!(merged.desktop.icon_size, 128);
        assert_eq!(merged.desktop.grid_spacing, desktop.grid_spacing);
        assert_eq!(merged.desktop.grid_gap, desktop.grid_gap);
        assert_eq!(merged.desktop.desktop_dir, desktop.desktop_dir, "themes never touch desktop_dir");
    }

    #[test]
    fn theme_supplied_appearance_and_behavior_fields_overwrite_only_what_is_set() {
        let mut data = CustomThemeData::default();
        data.appearance = Some(Appearance { type_indicators: true, monospace_list: false, theme_window: false });
        let settings = base_settings();
        let merged = merge_custom_theme("t", &data, &settings, &base_desktop());

        assert!(merged.appearance.type_indicators);
        assert_eq!(merged.behavior, settings.behavior, "behavior untouched when theme omits it");
    }

    #[test]
    fn image_background_resolves_the_filename_under_the_theme_dir() {
        let _guard_env = std::sync::Mutex::new(());
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let mut data = CustomThemeData::default();
        data.background = Some(ThemeBackground {
            mode: Some(BackgroundMode::Image),
            image: Some("wall.jpg".to_string()),
            image_fit: Some(ImageFit::Fit),
            ..Default::default()
        });
        let merged = merge_custom_theme("t", &data, &Settings::default(), &base_desktop());

        assert_eq!(merged.theme.background.mode, BackgroundMode::Image);
        assert_eq!(merged.theme.background.image_fit, ImageFit::Fit);
        let path = merged.theme.background.image_path.unwrap();
        assert!(path.ends_with("themes/t/backgrounds/wall.jpg"));
    }

    #[test]
    fn background_solid_and_gradient_fields_overwrite_independently() {
        let mut data = CustomThemeData::default();
        data.background = Some(ThemeBackground { solid: Some("#123456".to_string()), ..Default::default() });
        let mut current = Settings::default();
        current.theme.background.gradient_start = "#abcdef".to_string();
        let merged = merge_custom_theme("t", &data, &current, &base_desktop());

        assert_eq!(merged.theme.background.solid, "#123456");
        assert_eq!(merged.theme.background.gradient_start, "#abcdef");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p opusdm-theme-config apply::`
Expected: FAIL to compile (`merge_custom_theme` doesn't exist).

- [ ] **Step 3: Write `merge_custom_theme`**

```rust
//! Pure merge logic for applying a custom theme: a theme's `theme.json`
//! fields overwrite the corresponding current value only where the theme
//! actually set them; everything else keeps the user's current setting.
//! No GTK, no I/O beyond resolving a background image's absolute path -
//! fully unit-testable.

use opusdm_ipc::desktop::Background;
use opusdm_ipc::themes::CustomThemeData;
use opusdm_ipc::{Appearance, Behavior, DesktopSettings, IconSetId, Settings, Theme};

/// Everything a successful "apply a custom theme" produces, ready to send
/// as `SetTheme` + (if changed) `SetDesktopSettings`.
pub struct MergedTheme {
    pub theme: Theme,
    pub appearance: Appearance,
    pub behavior: Behavior,
    pub desktop: DesktopSettings,
}

/// Merge `slug`'s `data` over `current`/`current_desktop`. Only fields
/// `data` sets are changed; everything else is copied from `current`
/// unmodified.
pub fn merge_custom_theme(
    slug: &str,
    data: &CustomThemeData,
    current: &Settings,
    current_desktop: &DesktopSettings,
) -> MergedTheme {
    let mut colors = current.theme.colors.clone();
    if let Some(c) = &data.colors {
        colors = c.clone();
    }

    let mut background = current.theme.background.clone();
    if let Some(b) = &data.background {
        merge_background(&mut background, b, slug);
    }

    let mut appearance = current.appearance.clone();
    if let Some(a) = &data.appearance {
        appearance = a.clone();
    }

    let mut behavior = current.behavior.clone();
    if let Some(b) = &data.behavior {
        behavior = b.clone();
    }

    let mut desktop = current_desktop.clone();
    if let Some(m) = &data.desktop {
        if let Some(v) = m.icon_size {
            desktop.icon_size = v;
        }
        if let Some(v) = m.grid_spacing {
            desktop.grid_spacing = v;
        }
        if let Some(v) = m.grid_gap {
            desktop.grid_gap = v;
        }
    }

    MergedTheme {
        theme: Theme { icon_set: IconSetId::Custom(slug.to_string()), colors, background },
        appearance,
        behavior,
        desktop,
    }
}

fn merge_background(bg: &mut Background, theme_bg: &opusdm_ipc::themes::ThemeBackground, slug: &str) {
    if let Some(mode) = theme_bg.mode {
        bg.mode = mode;
    }
    if let Some(solid) = &theme_bg.solid {
        bg.solid = solid.clone();
    }
    if let Some(start) = &theme_bg.gradient_start {
        bg.gradient_start = start.clone();
    }
    if let Some(end) = &theme_bg.gradient_end {
        bg.gradient_end = end.clone();
    }
    if let Some(angle) = theme_bg.gradient_angle {
        bg.gradient_angle = angle;
    }
    if let Some(fit) = theme_bg.image_fit {
        bg.image_fit = fit;
    }
    if let Some(filename) = &theme_bg.image {
        bg.image_path = Some(opusdm_ipc::themes::background_image_path(slug, filename));
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p opusdm-theme-config apply::`
Expected: PASS (6 tests).

- [ ] **Step 5: Register the module**

In `crates/theme-config/src/main.rs`, add `mod apply;` next to the existing `mod model; mod ui;`.

- [ ] **Step 6: Commit**

```bash
git add crates/theme-config/src/apply.rs crates/theme-config/src/main.rs
git commit -m "feat(theme-config): add pure merge function for applying a custom theme"
```

---

## Task 8: wire the merge function into the Apply button

**Files:**
- Modify: `crates/theme-config/src/model.rs` (extend `apply`/`ApplyOutcome` to also carry `DesktopSettings` when needed)
- Modify: `crates/theme-config/src/main.rs:99-119` (the `apply.connect_clicked` handler)

**Interfaces:**
- Consumes: `apply::merge_custom_theme`, `opusdm_ipc::ToHub::SetDesktopSettings`.
- Produces: `pub fn apply_icon_set(icon_set: &IconSetId) -> (Theme, Option<DesktopSettings>)` in `model.rs` - the piece that decides whether a merge is needed at all; `pub fn apply_full(theme: &Theme, desktop: Option<&DesktopSettings>) -> ApplyOutcome` replacing the old `apply(theme: &Theme)`.

- [ ] **Step 1: Write the failing test**

Add to `crates/theme-config/src/model.rs`'s existing test module:

```rust
    #[test]
    fn apply_icon_set_none_needs_no_desktop_merge() {
        let (theme, desktop) = apply_icon_set(&opusdm_ipc::IconSetId::None, &Settings::default(), &opusdm_ipc::DesktopSettings::default());
        assert_eq!(theme.icon_set, opusdm_ipc::IconSetId::None);
        assert!(desktop.is_none());
    }

    #[test]
    fn apply_icon_set_custom_merges_theme_json_over_current_settings() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let dir = td.path().join("opusdm").join("themes").join("t");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"name":"t","label":"T"}"#).unwrap();
        std::fs::write(dir.join("theme.json"), r#"{"desktop":{"icon_size":100}}"#).unwrap();

        let mut current_desktop = opusdm_ipc::DesktopSettings::default();
        current_desktop.icon_size = 64;
        let (theme, desktop) = apply_icon_set(
            &opusdm_ipc::IconSetId::Custom("t".to_string()),
            &Settings::default(),
            &current_desktop,
        );
        assert_eq!(theme.icon_set, opusdm_ipc::IconSetId::Custom("t".to_string()));
        assert_eq!(desktop.unwrap().icon_size, 100);
    }
```

Note the test module needs `ENV_LOCK` - check if `model.rs` already has one from its existing tests (it does, per the file read earlier: `static ENV_LOCK: Mutex<()> = Mutex::new(());`). Reuse it.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p opusdm-theme-config model::`
Expected: FAIL to compile (`apply_icon_set` doesn't exist).

- [ ] **Step 3: Implement `apply_icon_set` and extend the apply/send path**

Add to `crates/theme-config/src/model.rs` (keep the existing `Theme`-only `apply`/`ApplyOutcome`/`send_to_hub`/`write_file` - extend, don't remove, since `None` selections still only need `Theme`):

```rust
use opusdm_ipc::{DesktopSettings, IconSetId};

/// Decide what a selected icon set needs applied: `IconSetId::None` needs
/// only the given `theme`'s other fields (colours/background as already
/// edited in the UI) with `icon_set = None`; `IconSetId::Custom(slug)`
/// merges that theme's `theme.json` over `current`/`current_desktop` via
/// [`crate::apply::merge_custom_theme`], returning a `DesktopSettings` to
/// send only when the theme actually touches desktop metrics.
pub fn apply_icon_set(
    icon_set: &IconSetId,
    current: &Settings,
    current_desktop: &DesktopSettings,
) -> (Theme, Option<DesktopSettings>) {
    match icon_set {
        IconSetId::None => {
            let mut theme = current.theme.clone();
            theme.icon_set = IconSetId::None;
            (theme, None)
        }
        IconSetId::Custom(slug) => {
            let data = opusdm_ipc::themes::load_theme_data(slug);
            let merged = crate::apply::merge_custom_theme(slug, &data, current, current_desktop);
            let desktop_changed = merged.desktop != *current_desktop;
            (merged.theme, desktop_changed.then_some(merged.desktop))
        }
    }
}
```

This needs `Settings` imported (`use opusdm_ipc::Settings;` already present) and `DesktopSettings` derives `PartialEq` (confirm: yes, per the earlier read of `crates/ipc/src/desktop.rs` - `#[derive(Debug, Clone, PartialEq, Eq, ...)]`).

Then add a second apply function that also sends/writes `DesktopSettings` when present:

```rust
/// Apply `theme`, and `desktop` if a custom theme's metrics changed it.
/// Same hub-or-file fallback as [`apply`], run for both messages.
pub fn apply_full(theme: &Theme, desktop: Option<&DesktopSettings>) -> ApplyOutcome {
    let outcome = apply(theme);
    if let (ApplyOutcome::SentToHub, Some(d)) = (&outcome, desktop) {
        let sock = opusdm_ipc::sockpath::hub_sock_path();
        if let Err(e) = send_desktop_to_hub(&sock, d) {
            eprintln!("opusdm-theme: desktop settings send failed: {e}");
        }
    } else if let (ApplyOutcome::WroteFile, Some(d)) = (&outcome, desktop) {
        if let Err(e) = write_desktop_file(d) {
            eprintln!("opusdm-theme: desktop settings write failed: {e}");
        }
    }
    outcome
}

fn send_desktop_to_hub(sock: &std::path::Path, settings: &DesktopSettings) -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(sock)?;
    opusdm_ipc::frame::send(&mut stream, &opusdm_ipc::ToHub::SetDesktopSettings(settings.clone()))
}

fn write_desktop_file(settings: &DesktopSettings) -> std::io::Result<()> {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("desktop.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(settings)?)
}
```

Overall apply outcome reporting to the user stays keyed on the `Theme` send's outcome (`apply_full`'s return value) - a `DesktopSettings` send failure is logged, not surfaced as a second dialog, keeping the existing single-`ApplyOutcome`-per-click UI contract from `main.rs` intact.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p opusdm-theme-config model::`
Expected: PASS.

- [ ] **Step 5: Wire it into `main.rs`'s Apply handler**

Change the `apply.connect_clicked` closure (lines 99-119) from:

```rust
        apply.connect_clicked(move |_| {
            let theme = opusdm_ipc::Theme {
                icon_set: icon_set.value(),
                colors: colors.read(),
                background: bg.read(),
            };
            match model::apply(&theme) {
```

to:

```rust
        apply.connect_clicked(move |_| {
            let selected = icon_set.value();
            let (theme, desktop) = match &selected {
                opusdm_ipc::IconSetId::None => (
                    opusdm_ipc::Theme { icon_set: selected, colors: colors.read(), background: bg.read() },
                    None,
                ),
                opusdm_ipc::IconSetId::Custom(_) => {
                    let current_settings = opusdm_ipc::Settings { theme: model::load_full(), ..Default::default() };
                    // model::load_full() below returns the current full Theme; appearance/
                    // behavior come from settings.json too - see model.rs change in Step 6.
                    model::apply_icon_set(&selected, &current_settings, &desktop_config::load())
                }
            };
            match model::apply_full(&theme, desktop.as_ref()) {
```

This references `model::load_full()` (a full `Settings`, not just `Theme`) and `desktop_config::load()` (the current `DesktopSettings`) which don't exist yet in `theme-config` - fix in the next step rather than leaving this as pseudo-code.

- [ ] **Step 6: Add the two missing loaders and fix the closure for real**

`opusdm-theme-config` already depends on `opusdm-desktop-config` (see `Cargo.toml`), which exposes `opusdm_desktop_config::model::load() -> DesktopSettings` and `opusdm_desktop_config::model::config_path()`. Use that directly - no new loader needed for desktop settings.

For full `Settings` (not just `Theme`), add to `crates/theme-config/src/model.rs`:

```rust
/// Load the full current `Settings` (defaults if missing/unparseable) -
/// unlike [`load`], keeps `appearance`/`behavior` too, needed to merge a
/// custom theme's partial overrides over them.
pub fn load_settings() -> Settings {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<Settings>(&c).ok())
        .unwrap_or_default()
}
```

Now rewrite the `main.rs` closure correctly:

```rust
        apply.connect_clicked(move |_| {
            let selected = icon_set.value();
            let current_settings = model::load_settings();
            let current_desktop = opusdm_desktop_config::model::load();
            let (theme, desktop) = match &selected {
                opusdm_ipc::IconSetId::None => (
                    opusdm_ipc::Theme { icon_set: selected.clone(), colors: colors.read(), background: bg.read() },
                    None,
                ),
                opusdm_ipc::IconSetId::Custom(_) => {
                    model::apply_icon_set(&selected, &current_settings, &current_desktop)
                }
            };
            match model::apply_full(&theme, desktop.as_ref()) {
                model::ApplyOutcome::SentToHub => {
                    note.set_text("Applied to the running desktop.");
                    note.set_visible(true);
                }
                model::ApplyOutcome::WroteFile => {
                    note.set_text("No hub running - saved, effective on next login.");
                    note.set_visible(true);
                }
                model::ApplyOutcome::Failed(e) => {
                    note.set_text(&format!("Could not apply: {e}"));
                    note.set_visible(true);
                }
            }
        });
```

- [ ] **Step 7: Build**

Run: `cargo build -p opusdm-theme-config`
Expected: success.

- [ ] **Step 8: Run the full theme-config test suite**

Run: `cargo test -p opusdm-theme-config`
Expected: all PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/theme-config/src/model.rs crates/theme-config/src/main.rs
git commit -m "feat(theme-config): apply a selected custom theme's full merged settings"
```

---

## Task 9: archive importer (zip-slip-safe extraction)

**Files:**
- Create: `crates/theme-config/src/importer.rs`
- Modify: `crates/theme-config/Cargo.toml` (add `zip`, `tar`, `flate2`, `bzip2`, `tempfile` as runtime deps)
- Modify: `crates/theme-config/src/main.rs` (add `mod importer;`)

**Interfaces:**
- Produces: `pub enum ImportError { UnsupportedExtension, ExtractFailed(String), PathTraversal(String), MissingManifest, InvalidManifest(String), Io(std::io::Error) }` (implements `std::fmt::Display`); `pub fn import_theme_archive(archive_path: &std::path::Path) -> Result<String, ImportError>` (returns the installed theme's slug on success).

- [ ] **Step 1: Add dependencies**

In `crates/theme-config/Cargo.toml`, add to `[dependencies]`:

```toml
zip = "2"
tar = "0.4"
flate2 = "1"
bzip2 = "0.4"
tempfile.workspace = true
```

(These four - `zip`, `tar`, `flate2`, `bzip2` - are not in `[workspace.dependencies]` yet; add matching version-pinned entries there instead, then reference with `.workspace = true` here, to follow this repo's existing convention of centralizing dependency versions in the root `Cargo.toml`. Check the root `Cargo.toml`'s `[workspace.dependencies]` table and add:)

```toml
zip = "2"
tar = "0.4"
flate2 = "1"
bzip2 = "0.4"
```

then in `crates/theme-config/Cargo.toml`:

```toml
zip.workspace = true
tar.workspace = true
flate2.workspace = true
bzip2.workspace = true
tempfile.workspace = true
```

- [ ] **Step 2: Write the failing tests**

```rust
// crates/theme-config/src/importer.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn build_zip(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let file = tempfile::Builder::new().suffix(".zip").tempfile().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
        file
    }

    fn valid_theme_entries() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("manifest.json", br#"{"name":"my-theme","label":"My Theme"}"#),
            ("icons/dir.png", b"fake-png"),
        ]
    }

    #[test]
    fn imports_a_valid_zip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive = build_zip(&valid_theme_entries());

        let slug = import_theme_archive(archive.path()).unwrap();
        assert_eq!(slug, "my-theme");
        let installed = opusdm_ipc::themes::theme_dir("my-theme");
        assert!(installed.join("manifest.json").is_file());
        assert!(installed.join("icons").join("dir.png").is_file());
    }

    #[test]
    fn rejects_unsupported_extension() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let file = tempfile::Builder::new().suffix(".rar").tempfile().unwrap();
        assert!(matches!(import_theme_archive(file.path()), Err(ImportError::UnsupportedExtension)));
    }

    #[test]
    fn rejects_missing_manifest() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive = build_zip(&[("icons/dir.png", b"x")]);
        assert!(matches!(import_theme_archive(archive.path()), Err(ImportError::MissingManifest)));
        assert!(!opusdm_ipc::themes::themes_dir().exists() || std::fs::read_dir(opusdm_ipc::themes::themes_dir()).unwrap().next().is_none());
    }

    #[test]
    fn rejects_invalid_manifest_name() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive = build_zip(&[("manifest.json", br#"{"name":"Bad Name","label":"x"}"#)]);
        assert!(matches!(import_theme_archive(archive.path()), Err(ImportError::InvalidManifest(_))));
    }

    #[test]
    fn rejects_a_zip_slip_entry_and_leaves_themes_dir_untouched() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive = build_zip(&[
            ("manifest.json", br#"{"name":"evil","label":"Evil"}"#),
            ("../../evil.txt", b"pwned"),
        ]);
        assert!(matches!(import_theme_archive(archive.path()), Err(ImportError::PathTraversal(_))));
        assert!(!opusdm_ipc::themes::theme_dir("evil").exists());
    }

    #[test]
    fn reimporting_overwrites_the_existing_theme() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive1 = build_zip(&[("manifest.json", br#"{"name":"t","label":"V1"}"#)]);
        import_theme_archive(archive1.path()).unwrap();
        let archive2 = build_zip(&[("manifest.json", br#"{"name":"t","label":"V2"}"#)]);
        import_theme_archive(archive2.path()).unwrap();

        let manifest: opusdm_ipc::themes::ThemeManifest = serde_json::from_str(
            &std::fs::read_to_string(opusdm_ipc::themes::theme_dir("t").join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.label, "V2");
    }

    #[test]
    fn imports_a_tar_gz() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let file = tempfile::Builder::new().suffix(".tar.gz").tempfile().unwrap();
        {
            let enc = flate2::write::GzEncoder::new(file.reopen().unwrap(), flate2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let data = br#"{"name":"gz-theme","label":"Gz Theme"}"#;
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, "manifest.json", &data[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let slug = import_theme_archive(file.path()).unwrap();
        assert_eq!(slug, "gz-theme");
    }

    #[test]
    fn imports_a_tar_bz2() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let file = tempfile::Builder::new().suffix(".tar.bz2").tempfile().unwrap();
        {
            let enc = bzip2::write::BzEncoder::new(file.reopen().unwrap(), bzip2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let data = br#"{"name":"bz2-theme","label":"Bz2 Theme"}"#;
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, "manifest.json", &data[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let slug = import_theme_archive(file.path()).unwrap();
        assert_eq!(slug, "bz2-theme");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p opusdm-theme-config importer::`
Expected: FAIL to compile (`importer` module doesn't exist).

- [ ] **Step 4: Write the importer**

```rust
//! Theme archive import: extract a `.zip`/`.tar.gz`/`.tar.bz2` into a temp
//! dir, validate it's a theme, then move it into
//! `~/.config/opusdm/themes/<name>/`. All-or-nothing - `themes/` is only
//! ever touched on full success.

use opusdm_ipc::themes::{theme_dir, themes_dir, ThemeManifest};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum ImportError {
    UnsupportedExtension,
    ExtractFailed(String),
    PathTraversal(String),
    MissingManifest,
    InvalidManifest(String),
    Io(std::io::Error),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::UnsupportedExtension => {
                write!(f, "unsupported file type (expected .zip, .tar.gz or .tar.bz2)")
            }
            ImportError::ExtractFailed(e) => write!(f, "could not read archive: {e}"),
            ImportError::PathTraversal(entry) => {
                write!(f, "archive contains an unsafe path: {entry}")
            }
            ImportError::MissingManifest => write!(f, "not a theme archive (no manifest.json at the root)"),
            ImportError::InvalidManifest(e) => write!(f, "invalid manifest.json: {e}"),
            ImportError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        ImportError::Io(e)
    }
}

/// True if `entry_name` (a `/`-separated archive path) is safe to extract
/// under an extraction root: no absolute path, no `..` component.
fn is_safe_entry(entry_name: &str) -> bool {
    let path = Path::new(entry_name);
    if path.is_absolute() {
        return false;
    }
    !path.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Import the archive at `archive_path`. Returns the installed theme's
/// slug on success.
pub fn import_theme_archive(archive_path: &Path) -> Result<String, ImportError> {
    let name = archive_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let staging = tempfile::tempdir()?;

    if name.ends_with(".zip") {
        extract_zip(archive_path, staging.path())?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(archive_path, staging.path())?;
    } else if name.ends_with(".tar.bz2") {
        extract_tar_bz2(archive_path, staging.path())?;
    } else {
        return Err(ImportError::UnsupportedExtension);
    }

    let manifest_path = staging.path().join("manifest.json");
    let content = std::fs::read_to_string(&manifest_path).map_err(|_| ImportError::MissingManifest)?;
    let manifest: ThemeManifest =
        serde_json::from_str(&content).map_err(|e| ImportError::InvalidManifest(e.to_string()))?;
    if !opusdm_ipc::themes::valid_slug(&manifest.name) {
        return Err(ImportError::InvalidManifest(format!("invalid name: {:?}", manifest.name)));
    }
    if manifest.label.trim().is_empty() {
        return Err(ImportError::InvalidManifest("label is empty".to_string()));
    }

    std::fs::create_dir_all(themes_dir())?;
    let dest = theme_dir(&manifest.name);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    // `rename` across filesystems can fail (EXDEV) - tempfile's default
    // temp dir is usually the same filesystem as $XDG_CONFIG_HOME under
    // $HOME, but fall back to a recursive copy if not.
    if std::fs::rename(staging.path(), &dest).is_err() {
        copy_dir_recursive(staging.path(), &dest)?;
    }

    Ok(manifest.name)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

fn extract_zip(archive_path: &Path, dest: &Path) -> Result<(), ImportError> {
    let file = File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| ImportError::ExtractFailed(e.to_string()))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| ImportError::ExtractFailed(e.to_string()))?;
        let name = entry.name().to_string();
        if !is_safe_entry(&name) {
            return Err(ImportError::PathTraversal(name));
        }
        let out_path = dest.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out_file = File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out_file)?;
    }
    Ok(())
}

fn extract_tar_entries<R: Read>(mut archive: tar::Archive<R>, dest: &Path) -> Result<(), ImportError> {
    for entry in archive.entries().map_err(|e| ImportError::ExtractFailed(e.to_string()))? {
        let mut entry = entry.map_err(|e| ImportError::ExtractFailed(e.to_string()))?;
        let path = entry.path().map_err(|e| ImportError::ExtractFailed(e.to_string()))?.into_owned();
        let name = path.to_string_lossy().to_string();
        if !is_safe_entry(&name) {
            return Err(ImportError::PathTraversal(name));
        }
        let out_path = dest.join(&path);
        entry.unpack(&out_path).map_err(|e| ImportError::ExtractFailed(e.to_string()))?;
    }
    Ok(())
}

fn extract_tar_gz(archive_path: &Path, dest: &Path) -> Result<(), ImportError> {
    let file = File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    extract_tar_entries(tar::Archive::new(decoder), dest)
}

fn extract_tar_bz2(archive_path: &Path, dest: &Path) -> Result<(), ImportError> {
    let file = File::open(archive_path)?;
    let decoder = bzip2::read::BzDecoder::new(file);
    extract_tar_entries(tar::Archive::new(decoder), dest)
}
```

Make `ThemeManifest`'s fields (`name`, `label`) `pub` (they already are per Task 2's definition) so this module can read `manifest.name`/`manifest.label` directly.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p opusdm-theme-config importer::`
Expected: PASS (8 tests).

Note: `tar::Entry::unpack` follows symlinks/hardlinks inside the tar in ways that could themselves be a traversal vector for a malicious tarball entry of type symlink - `is_safe_entry` only checks the entry's own *path*, not a symlink target it might create. Since this importer targets user-supplied theme archives (not fully untrusted internet uploads processed server-side), and the spec's threat model is "don't let an archive write outside the extraction root via its own path", this is an accepted scope boundary - note it in the module doc comment:

```rust
// Threat model: this guards against a malicious *path* (zip-slip / tar-slip)
// escaping the extraction root. It does not attempt to sanitize symlink or
// hardlink entries inside a tarball - `tar::Entry::unpack` handles those
// per the `tar` crate's own defaults. Theme archives are user-supplied and
// imported locally, not processed as untrusted server-side uploads.
```

Add this comment to the top of `extract_tar_entries`.

- [ ] **Step 6: Register the module**

Add `mod importer;` to `crates/theme-config/src/main.rs`.

- [ ] **Step 7: Run the full theme-config suite**

Run: `cargo test -p opusdm-theme-config`
Expected: all PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/theme-config/src/importer.rs crates/theme-config/src/main.rs crates/theme-config/Cargo.toml Cargo.toml Cargo.lock
git commit -m "feat(theme-config): add zip/tar.gz/tar.bz2 theme archive importer"
```

---

## Task 10: "Import theme..." UI + picker refresh

**Files:**
- Modify: `crates/theme-config/src/main.rs` (add an Import button and file chooser near the icon-set picker; refresh `IconSetSection` after a successful import)

**Interfaces:**
- Consumes: `importer::import_theme_archive`, `ui::IconSetSection::new`.

- [ ] **Step 1: Add the button and wire the file chooser**

In `build_window`, right after `desktop_page.append(&icon_set.root);` (before the separator), add:

```rust
    let import_btn = gtk::Button::with_label("Import theme...");
    desktop_page.append(&import_btn);
```

Then, after `icon_set` and `window` are both in scope and before `window.present();`, wire the click handler (place this block near the other `connect_clicked` handlers, after `apply`'s):

```rust
    {
        let window = window.clone();
        let icon_set = icon_set.clone();
        let desktop_page = desktop_page.clone();
        let note = note.clone();
        import_btn.connect_clicked(move |_| {
            let filter = gtk::FileFilter::new();
            filter.add_pattern("*.zip");
            filter.add_pattern("*.tar.gz");
            filter.add_pattern("*.tgz");
            filter.add_pattern("*.tar.bz2");
            filter.set_name(Some("Theme archives"));
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);

            let dialog = gtk::FileDialog::builder().title("Import Theme").filters(&filters).build();
            let window = window.clone();
            let icon_set = icon_set.clone();
            let desktop_page = desktop_page.clone();
            let note = note.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else { return };
                let Some(path) = file.path() else { return };
                match crate::importer::import_theme_archive(&path) {
                    Ok(slug) => {
                        note.set_text(&format!("Imported theme \"{slug}\". Select it above and click Apply."));
                        note.set_visible(true);
                        // Rebuild the dropdown so the newly-imported theme appears.
                        let new_section = ui::IconSetSection::new(&opusdm_ipc::IconSetId::Custom(slug));
                        desktop_page.remove(&icon_set.root);
                        desktop_page.prepend(&new_section.root);
                        // icon_set is an Rc<IconSetSection>; swapping its inner
                        // widget tree in place isn't possible through a shared
                        // Rc from inside this closure - see Step 2 for the fix.
                    }
                    Err(e) => {
                        note.set_text(&format!("Import failed: {e}"));
                        note.set_visible(true);
                    }
                }
            });
        });
    }
```

This has a real problem flagged inline: `icon_set` is `Rc<ui::IconSetSection>` (immutable once built), so a closure can't swap its dropdown's contents through a shared `Rc` reference. Fix in Step 2 rather than leaving broken code.

- [ ] **Step 2: Make `IconSetSection` rebuildable in place**

The simplest fix keeping `IconSetSection` itself simple: instead of trying to mutate the existing section, hold the icon-set dropdown's *container* (`desktop_page`) and rebuild+reinsert the whole section, using `gtk::Box`'s child position. Change the closure to not capture `icon_set: Rc<ui::IconSetSection>` for mutation - instead capture a `Rc<RefCell<Rc<ui::IconSetSection>>>` so the outer scope's reference can be swapped too (needed so the later `apply.connect_clicked` handler, wired before this one in the file, reads the *current* section). Restructure `build_window` as follows:

Change the declaration:

```rust
    let icon_set = Rc::new(RefCell::new(Rc::new(ui::IconSetSection::new(&theme.icon_set))));
```

(add `use std::cell::RefCell;` to the top imports)

Every other place in `build_window` that reads `icon_set.value()` or `icon_set.root` becomes `icon_set.borrow().value()` / `icon_set.borrow().root` - specifically:
- `desktop_page.append(&icon_set.root);` -> `desktop_page.append(&icon_set.borrow().root);`
- Inside `apply.connect_clicked`: `let icon_set = icon_set.clone();` (clones the `Rc<RefCell<..>>`) then `icon_set.borrow().value()` where `icon_set.value()` was used.

And the import handler's success arm becomes:

```rust
                    Ok(slug) => {
                        note.set_text(&format!("Imported theme \"{slug}\". Select it above and click Apply."));
                        note.set_visible(true);
                        let old_root = icon_set.borrow().root.clone();
                        let new_section = Rc::new(ui::IconSetSection::new(&opusdm_ipc::IconSetId::Custom(slug)));
                        desktop_page.insert_child_after(&new_section.root, Some(&old_root));
                        desktop_page.remove(&old_root);
                        *icon_set.borrow_mut() = new_section;
                    }
```

Update the closure's captures accordingly (`icon_set` captured as the `Rc<RefCell<Rc<IconSetSection>>>`, cloned before the `dialog.open` call same as `window`/`desktop_page`/`note`).

- [ ] **Step 3: Build**

Run: `cargo build -p opusdm-theme-config`
Expected: success. Fix any remaining `icon_set.value()`/`icon_set.root` references the compiler flags that Step 2 missed.

- [ ] **Step 4: Manual smoke test**

This step is GTK UI wiring with no automated test (consistent with Task 6's note - correctness of the underlying logic is covered by Tasks 7-9's unit tests). Run manually:

```bash
cargo run -p xtask -- package-themes
mkdir -p /tmp/opusdm-theme-test && cp target/themes/breeze-dark.zip /tmp/opusdm-theme-test/
cargo run -p opusdm-theme-config
```

In the opened window: click "Import theme...", pick `/tmp/opusdm-theme-test/breeze-dark.zip`, confirm the note says "Imported theme...", confirm "Breeze Dark" now appears in the ICON SET dropdown, select it, click Apply, confirm the note reports success (or "No hub running - saved..." if no hub is active), then check `~/.config/opusdm/themes/breeze-dark/` exists with `manifest.json` and `icons/`.

- [ ] **Step 5: Commit**

```bash
git add crates/theme-config/src/main.rs
git commit -m "feat(theme-config): add theme archive import UI"
```

---

## Task 11: full workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Full build**

Run: `cargo build --workspace`
Expected: success, no warnings about unused `#[allow(deprecated)]` or dead code beyond the intentionally-kept `IconSetId::LegacyBreezeDark`.

- [ ] **Step 2: Full test suite**

Run: `cargo test --workspace`
Expected: all PASS.

- [ ] **Step 3: Lint**

Run: `cargo clippy --workspace --all-targets -- -D warnings` (check first whether this repo's CI already runs clippy at this strictness - if a `.github/workflows/*.yml` or similar exists, match its exact clippy invocation instead of guessing flags: `grep -rn clippy .github 2>/dev/null || echo "no CI clippy config found, using default -D warnings"`)
Expected: no errors. Fix anything flagged (expect possibly: the `#[allow(deprecated)]` uses in Task 1/3 needing to stay, unused `mut`, etc.) - do not silence a real clippy finding with a blanket `#[allow]` unless it's the intentional `deprecated` one from Task 1.

- [ ] **Step 4: Re-run the xtask packaging step and confirm the zip is well-formed**

Run: `cargo run -p xtask -- package-themes && unzip -t target/themes/breeze-dark.zip`
Expected: `No errors detected in compressed data of target/themes/breeze-dark.zip.`

- [ ] **Step 5: Update `CHANGES.md`**

Per this project's CLAUDE.md convention ("Always keep a Markdown file with progressive changes... call it `CHANGES.md`"), check if `CHANGES.md` exists at the repo root (`ls CHANGES.md`). If it exists, add an entry; if not, this convention may not yet be in use in this repo - check `git log --all --oneline -- CHANGES.md` to confirm before creating one from scratch (don't introduce a new top-level file the project hasn't adopted without checking first).

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "chore: verify workspace build/test/lint after custom themes feature"
```

(Only if Step 5 produced a `CHANGES.md` change or Steps 1-4 required fixes not already committed in their owning tasks - if everything was already green and committed task-by-task, skip this final commit.)

---

## Self-review notes (for whoever executes this plan)

- **Spec coverage:** on-disk format (Task 2), `IconSetId` change + backward compat (Task 1), icon loader (Task 3), call-site fixes (Task 4), Breeze Dark packaging (Task 5), picker UI (Task 6), partial-override merge semantics (Task 7-8), archive import with zip-slip guard and overwrite-on-reimport (Task 9-10), full verification (Task 11). The two spec "open questions" are resolved: discovery/parsing lives in `opusdm-ipc::themes` (Task 2), the importer lives in `opusdm-theme-config` only (Task 9); the colours/appearance/behavior/desktop merge is "partial overwrite, keep current for unset fields" throughout (Task 7).
- **Not covered by this plan** (per spec's explicit non-goals): live-reload, marketplace/download UI, per-Lister overrides, slideshow backgrounds, `desktop_dir` theming. `preview.png` thumbnails in the picker are spec-optional and deliberately deferred past Task 6's note - a small additive follow-up, not a gap in this plan's own scope.
- **Type consistency check:** `IconSetId::Custom(String)` (Task 1) flows unchanged through `opusdm_ipc::themes` (Task 2, `theme_dir(slug: &str)`), `opusdm_iconview::icon_set::icon_bytes_for(..., set: &IconSetId)` (Task 3), `glyphs::icon_bytes_for_row(row, set: &IconSetId)` (Task 4), `apply::merge_custom_theme(slug: &str, ...)` (Task 7), and `model::apply_icon_set(icon_set: &IconSetId, ...)` (Task 8) - every signature takes `&IconSetId` or an already-extracted `&str` slug, never a bare owned `IconSetId` that would force an unnecessary clone.
