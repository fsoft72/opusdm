# Theme Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Full theme support for OpusDM: a bundled icon library (Breeze Dark), Lister colours, and desktop background, edited together in a new `opusdm-theme` app and applied live via IPC.

**Architecture:** A new `Theme` struct (icon set + colours + background) replaces the old scattered `ThemePreset`/`ColorSettings`-in-`Settings`/`Background`-in-`DesktopSettings` storage. One new `ToHub::SetTheme(Theme)` message applies all three atomically. `opusdm-theme` is a new GTK binary crate, structured like the existing `opus-desktop-config`. Real icon images render through a new `icon_path_for` lookup in `opusdm-iconview`, sitting next to the existing glyph lookup, used by both the Lister's Name column and the desktop icon grid.

**Tech Stack:** Rust, GTK4 (gtk-rs), serde/serde_json, existing Unix-socket IPC framing (`opusdm-ipc`).

**Spec:** `docs/superpowers/specs/2026-09-12-theme-support-design.md`

## Global Constraints

- No backward compatibility: old `ThemePreset`, `ColorSettings`-inside-`Settings`, `Background`-inside-`DesktopSettings`, `Appearance.preset`, `Appearance.file_icons` are removed outright, not migrated or dual-read.
- No saved/named custom theme list in v1 - only direct editing of icon set / colours / background.
- Only Breeze Dark ships as an icon set beyond "None". Only the icon subset covering `IconKind` (Dir, File, Symlink, Broken) plus the extension-table categories already in `opusdm-iconview::glyphs::EXT_GLYPHS` (code, image, archive, audio, video, text/doc, pdf) - not a full Breeze Dark set.
- Bundled as PNG, embedded with `include_bytes!` at compile time - not read from disk at runtime, not SVG (avoids a runtime dependency on a gdk-pixbuf SVG loader).
- `crates/desktop-config`'s `BackgroundSection`/`ColorRow`/`hex_to_rgba`/`rgba_to_hex` are reused by `opusdm-theme` via a library target on `opusdm-desktop-config`, not duplicated.
- Every text file ends with a trailing empty line. Constants upper-snake-case. Public items get doc comments.

---

## File Structure

**New:**
- `crates/theme-config/Cargo.toml`, `src/main.rs`, `src/model.rs`, `src/ui.rs` - the `opusdm-theme` binary.
- `crates/desktop-config/src/lib.rs` - exposes `model` and `ui` as a library so `theme-config` can reuse `BackgroundSection`/`ColorRow`/hex helpers.
- `crates/iconview/assets/breeze-dark/*.png` - bundled icon subset.
- `crates/iconview/src/icon_set.rs` - `icon_path_for` real-icon lookup, sibling of `glyphs.rs`.

**Modified:**
- `crates/ipc/src/theme.rs` - rewritten: `ThemePreset` removed, `IconSetId` + `Theme` added.
- `crates/ipc/src/proto.rs` - `Appearance` loses `preset`/`file_icons`; `Settings` gains `theme: Theme`, loses `colors`.
- `crates/ipc/src/desktop.rs` - `DesktopSettings` loses `background`.
- `crates/ipc/src/lib.rs` - export updates.
- `crates/hub/src/server.rs` - handle `ToHub::SetTheme`; new `HubEvent::ThemeChanged`.
- `crates/hub/src/toolbar.rs` - remove colour/theme/file-icons UI from the Settings dialog; initial paint reads `Theme.background`; `ThemeChanged`/`DesktopSettingsChanged` event handling split; add "Theme..." desktop-menu launcher.
- `crates/iconview/src/lib.rs` - export `icon_set` module.
- `crates/iconview/src/item.rs` - `IconItem::new` takes an `IconSetId`, shows a real icon when available.
- `crates/iconview/src/field.rs` - `IconField::set_icon_set`.
- `crates/lister/src/glyphs.rs` - `decorate_name` replaced by a real-icon-aware render helper used from `columns.rs`.
- `crates/lister/src/columns.rs` - Name column cell becomes an image+label box when a real icon set is active.
- `crates/lister/src/theme.rs`, `crates/lister/src/window.rs` - adjust to the new `Settings`/`Appearance` shape.
- `Cargo.toml` (workspace) - add `crates/theme-config` member.

---

## Task 1: `Theme` and `IconSetId` replace `ThemePreset`

**Files:**
- Modify: `crates/ipc/src/theme.rs` (full rewrite)
- Modify: `crates/ipc/src/lib.rs`

**Interfaces:**
- Produces: `opusdm_ipc::theme::IconSetId { None, BreezeDark }` with `ALL`, `id()`, `label()`, `from_id()`; `opusdm_ipc::theme::Theme { icon_set: IconSetId, colors: ColorSettings, background: Background }` implementing `Default`.

- [ ] **Step 1: Write the failing tests**

Replace the whole `#[cfg(test)] mod tests` block at the bottom of `crates/ipc/src/theme.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_set_ids_are_unique_and_round_trip() {
        let mut ids: Vec<&str> = IconSetId::ALL.iter().map(|s| s.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate icon set id");
        for s in IconSetId::ALL {
            assert_eq!(IconSetId::from_id(s.id()), Some(s));
        }
        assert_eq!(IconSetId::from_id("nonsense"), None);
    }

    #[test]
    fn icon_set_labels_are_distinct() {
        let mut labels: Vec<&str> = IconSetId::ALL.iter().map(|s| s.label()).collect();
        let count = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), count);
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

This will not compile yet (old types still present) - that is the expected red state before Step 3.

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test -p opusdm-ipc theme::tests`
Expected: compile error, `Theme`/`IconSetId` not found (old `ThemePreset` code still in the file).

- [ ] **Step 3: Replace the implementation**

Replace everything above the `#[cfg(test)]` block in `crates/ipc/src/theme.rs` with:

```rust
//! Icon set choice and the `Theme` bundle: icon set + colours + desktop
//! background, edited together by `opusdm-theme` and applied to the running
//! desktop in one atomic `ToHub::SetTheme`.

use crate::desktop::Background;
use crate::proto::ColorSettings;
use serde::{Deserialize, Serialize};

/// Which bundled icon library the Lister and desktop icon grid use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconSetId {
    /// No icon images: Nerd Font glyphs when a patched font is available, a
    /// plain name otherwise. This is the only icon "set" that does not need
    /// bundled assets.
    None,
    /// Breeze Dark (KDE), bundled subset covering `IconKind` plus the file
    /// categories `opusdm_iconview::glyphs` already classifies by extension.
    BreezeDark,
}

impl IconSetId {
    /// Every icon set, in the order `opusdm-theme` lists them.
    pub const ALL: [IconSetId; 2] = [IconSetId::None, IconSetId::BreezeDark];

    /// Stable identifier stored in settings.json.
    pub fn id(self) -> &'static str {
        match self {
            IconSetId::None => "none",
            IconSetId::BreezeDark => "breeze-dark",
        }
    }

    /// Human-readable name for `opusdm-theme`'s picker.
    pub fn label(self) -> &'static str {
        match self {
            IconSetId::None => "None (text glyphs)",
            IconSetId::BreezeDark => "Breeze Dark",
        }
    }

    /// Look an icon set up by its [`id`](Self::id). `None` for an unknown
    /// value written by a newer version.
    pub fn from_id(id: &str) -> Option<IconSetId> {
        IconSetId::ALL.into_iter().find(|s| s.id() == id)
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

- [ ] **Step 4: Update `crates/ipc/src/lib.rs` exports**

Change:
```rust
pub use theme::ThemePreset;
```
to:
```rust
pub use theme::{IconSetId, Theme};
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p opusdm-ipc theme::tests`
Expected: 3 passed (fails first for unrelated compile errors in `proto.rs`/`desktop.rs` referencing the old `colors`/`background` shape until Tasks 2-3 land - if so, proceed to Task 2 immediately and come back to run this once the crate compiles).

- [ ] **Step 6: Commit**

```bash
git add crates/ipc/src/theme.rs crates/ipc/src/lib.rs
git commit -m "feat(ipc): replace ThemePreset with Theme + IconSetId"
```

---

## Task 2: `Settings`/`Appearance` restructured to hold `Theme`

**Files:**
- Modify: `crates/ipc/src/proto.rs`

**Interfaces:**
- Consumes: `opusdm_ipc::theme::Theme` (Task 1).
- Produces: `Settings { theme: Theme, appearance: Appearance, behavior: Behavior }`; `Appearance { type_indicators, monospace_list, theme_window }` (no `preset`, no `file_icons`).

- [ ] **Step 1: Update `Appearance` and `Settings`**

In `crates/ipc/src/proto.rs`, remove `PRESET_CUSTOM` and replace the `Appearance` struct, its `Default` impl, and `Settings`:

```rust
/// Look-and-feel switches that are not part of the theme (icon set, colours,
/// background) - those live in [`crate::theme::Theme`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Append an `ls -F` style type character (`/ * @ = |`) to each name.
    pub type_indicators: bool,
    /// Render the file list in the system monospace font.
    pub monospace_list: bool,
    /// Carry the file colours into the window chrome too. Off by default so a
    /// user who only picked accent colours keeps their system theme.
    pub theme_window: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { type_indicators: false, monospace_list: false, theme_window: false }
    }
}
```

```rust
/// Every user-configurable setting the hub owns, persists to
/// settings.json, and distributes to every Lister. This one struct is the
/// wire format (`Welcome`/`SettingsUpdate`), the on-disk JSON shape, and the
/// hub's in-memory state - one place to extend when a new setting is added,
/// instead of a new IPC field plus a new HubCmd variant each time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub theme: crate::theme::Theme,
    pub appearance: Appearance,
    pub behavior: Behavior,
}
```

`Theme` derives `PartialEq` but not `Eq` (it contains no floats itself, but `Background` - check: `Background`'s fields, per `crates/ipc/src/desktop.rs`, are hex `String`s and enums, so `Eq` is derivable there too). Confirm `Background` derives `Eq`; if it does not yet, add `Eq` to its derive list so `Settings` (which derives `Eq`) still compiles, and add `Eq` to `Theme`'s derive list.

- [ ] **Step 2: Add `SetTheme` to `ToHub`**

In the `ToHub` enum, after `SetDesktopSettings`:

```rust
    /// Sent by `opusdm-theme` (not a Lister): replace the hub's theme (icon
    /// set, colours, background). The hub sanitises, persists to both
    /// settings.json and desktop.json, and applies live. No reply.
    SetTheme(crate::theme::Theme),
```

- [ ] **Step 3: Fix the existing proto.rs tests that reference the old shape**

Replace the two blocks that reference `s.colors`, `ThemePreset::Cyberpunk`, `PRESET_CUSTOM` (around what was line 406-452) with:

```rust
        assert_eq!(s.theme.colors.source_bg, "#111111");
        assert_eq!(s.theme.colors.file_fg, "#888888");
        assert_eq!(s.theme.colors.selected_bg, ColorSettings::default().selected_bg);
```

and the settings-with-custom-theme test body:

```rust
    #[test]
    fn settings_roundtrip_with_a_custom_theme() {
        let mut s = Settings::default();
        s.theme = crate::theme::Theme {
            icon_set: crate::theme::IconSetId::BreezeDark,
            colors: ColorSettings { source_bg: "#111111".into(), file_fg: "#888888".into(), ..ColorSettings::default() },
            background: crate::desktop::Background::default(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
```

(Find the exact surrounding test function name via `grep -n "fn.*colors\|fn.*preset" crates/ipc/src/proto.rs` before editing, and replace the whole function body, not just the assertions, so the test still compiles standalone.)

- [ ] **Step 4: Add `SetTheme` to the roundtrip coverage test**

In `every_tohub_variant_roundtrips`, add:

```rust
        roundtrip_tohub(ToHub::SetTheme(crate::theme::Theme::default()));
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p opusdm-ipc`
Expected: all pass. (Task 3 must land first if `desktop.rs` fails to compile because `DesktopSettings` still has `background` at this point - that's fine, do Task 3 next then return.)

- [ ] **Step 6: Commit**

```bash
git add crates/ipc/src/proto.rs
git commit -m "feat(ipc): move colours into Settings.theme, add SetTheme"
```

---

## Task 3: `DesktopSettings` loses `background`

**Files:**
- Modify: `crates/ipc/src/desktop.rs`

**Interfaces:**
- Produces: `DesktopSettings { icon_size, grid_spacing, grid_gap, desktop_dir }` (no `background`); `Background`/`BackgroundMode`/`ImageFit`/`background_css` unchanged in shape, just no longer nested under `DesktopSettings`.

- [ ] **Step 1: Read the current struct and its `sanitized`**

Run: `grep -n "pub struct DesktopSettings\|impl DesktopSettings\|fn sanitized" crates/ipc/src/desktop.rs`

- [ ] **Step 2: Remove the `background` field**

Remove the `pub background: Background,` field from `DesktopSettings`, remove it from `DesktopSettings::default()` (or its `#[derive(Default)]`/manual impl, whichever is present), and remove the corresponding `self.background = self.background.sanitized();` (or equivalent) line inside `DesktopSettings::sanitized`. Check whether `Background` has its own `sanitized()` method used only from there - if so, keep `Background::sanitized()` itself (still used by `Theme` sanitisation added in Task 5), just drop the call site inside `DesktopSettings::sanitized`.

- [ ] **Step 3: Fix the two proto.rs tests that touch `DesktopSettings.background`**

Back in `crates/ipc/src/proto.rs`, the tests `roundtrip_tohub(ToHub::SetDesktopSettings(...))` and `set_desktop_settings_carries_the_value` reference `ds.background.mode`. Update `set_desktop_settings_carries_the_value` to mutate a field `DesktopSettings` still has instead:

```rust
    #[test]
    fn set_desktop_settings_carries_the_value() {
        let mut ds = crate::desktop::DesktopSettings::default();
        ds.icon_size = 120;
        let mut buf = Vec::new();
        send(&mut buf, &ToHub::SetDesktopSettings(ds.clone())).unwrap();
        let got: Option<ToHub> = recv(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, Some(ToHub::SetDesktopSettings(ds)));
    }
```

- [ ] **Step 4: Fix `desktop.rs`'s own tests**

Run: `grep -n "background" crates/ipc/src/desktop.rs` and remove/update any test that sets or reads `DesktopSettings.background` (e.g. `sanitized_clamps_every_numeric_field_at_both_ends` likely only touches numeric fields already and needs no change; confirm by reading it).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p opusdm-ipc`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/ipc/src/desktop.rs crates/ipc/src/proto.rs
git commit -m "feat(ipc): move desktop background out of DesktopSettings into Theme"
```

---

## Task 4: Hub applies `SetTheme` atomically and repaints live

**Files:**
- Modify: `crates/hub/src/server.rs`
- Modify: `crates/hub/src/toolbar.rs`

**Interfaces:**
- Consumes: `ToHub::SetTheme(Theme)` (Task 2), `Theme` (Task 1).
- Produces: `HubEvent::ThemeChanged(opusdm_ipc::Theme)`.

- [ ] **Step 1: Add the `HubEvent` variant**

In `crates/hub/src/server.rs`, near `DesktopSettingsChanged(DesktopSettings)` (around line 50), add:

```rust
    ThemeChanged(opusdm_ipc::Theme),
```

- [ ] **Step 2: Handle `ToHub::SetTheme` in the owner loop**

In `owner_loop`, after the `ToHub::SetDesktopSettings` arm (around line 226-232), add:

```rust
            OwnerMsg::FromLister(_cid, ToHub::SetTheme(theme)) => {
                current_settings.theme = theme;
                if let Err(e) = settings::save_settings(&current_settings) {
                    eprintln!("opusdm-hub: cannot save settings.json: {e}");
                }
                for c in conns.values_mut() {
                    if c.registered {
                        let _ = send(&mut c.write, &ToLister::SettingsUpdate(current_settings.clone()));
                    }
                }
                let _ = evt_tx.send(HubEvent::ThemeChanged(current_settings.theme.clone()));
            }
```

This mirrors `HubCmd::UpdateSettings`'s broadcast loop (around line 423-428) - reuse the exact same `for c in conns.values_mut() { if c.registered { ... } }` pattern found there; read that block first (`sed -n '420,432p' crates/hub/src/server.rs`) to match its exact style before writing this arm.

- [ ] **Step 3: Split `DesktopSettingsChanged` handling in `toolbar.rs`**

In `crates/hub/src/toolbar.rs`'s `apply_event`, the `HubEvent::DesktopSettingsChanged(ds)` arm currently calls `install_desktop_css(&ds.background)` first. Remove that line (background no longer lives on `ds`):

```rust
        HubEvent::DesktopSettingsChanged(ds) => {
            let field = widgets.desktop_field.borrow().clone();
            if let Some(field) = field.as_ref() {
                field.set_icon_size(ds.icon_size);
                field.set_grid_spacing(ds.grid_spacing);
                field.set_grid_gap(ds.grid_gap);
            }
            // ... rest of the arm (desktop_dir handling) unchanged
        }
```

Add a sibling arm right after it:

```rust
        HubEvent::ThemeChanged(theme) => {
            install_desktop_css(&theme.background);
            if let Some(field) = widgets.desktop_field.borrow().as_ref() {
                field.set_icon_set(theme.icon_set);
            }
        }
```

(`IconField::set_icon_set` is added in Task 6; this line will not compile until then - acceptable, Task 6 follows immediately in execution order.)

- [ ] **Step 4: Fix the initial paint in `build_ui`**

`build_ui` currently does:
```rust
    let desktop_settings = opusdm_hub::desktop_settings::load();
    install_desktop_css(&desktop_settings.background);
```
Change to load the theme's background from `settings.json` instead:
```rust
    let desktop_settings = opusdm_hub::desktop_settings::load();
    let initial_settings = settings::load_settings();
    install_desktop_css(&initial_settings.theme.background);
```
`desktop_settings` (icon_size/grid/dir) is still used further down in `build_ui` for the icon field - keep that variable, just source the background from `initial_settings` instead.

- [ ] **Step 5: Write a server.rs test for `SetTheme`**

Find the existing test `set_desktop_settings_emits_a_changed_event` (around line 706) and add a sibling right after it:

```rust
    #[test]
    fn set_theme_persists_and_emits_a_changed_event() {
        let (owner_tx, rx) = channel();
        let (evt_tx, evt_rx) = channel();
        let handle = spawn_test_owner_loop(owner_tx.clone(), rx, evt_tx);
        let mut c = handle.connect_test_client();
        let mut theme = opusdm_ipc::Theme::default();
        theme.icon_set = opusdm_ipc::IconSetId::BreezeDark;
        send(&mut c, &ToHub::SetTheme(theme.clone())).unwrap();

        let mut got = None;
        for ev in evt_rx.iter() {
            if let HubEvent::ThemeChanged(t) = ev {
                got = Some(t);
                break;
            }
        }
        let got = got.expect("ThemeChanged not emitted");
        assert_eq!(got, theme);
    }
```

Read the actual helper names (`spawn_test_owner_loop`, `handle.connect_test_client`, or whatever the existing `set_desktop_settings_emits_a_changed_event` test actually calls - it will differ from this sketch) with `sed -n '690,735p' crates/hub/src/server.rs` first, and match this new test's setup to that exact pattern rather than the placeholder names above.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p opusdm-hub server::`
Expected: `set_theme_persists_and_emits_a_changed_event` passes alongside the existing desktop-settings test.

- [ ] **Step 7: Commit**

```bash
git add crates/hub/src/server.rs crates/hub/src/toolbar.rs
git commit -m "feat(hub): apply SetTheme atomically and repaint live"
```

---

## Task 5: Remove the old colour/theme/file-icons UI from the hub Settings dialog

**Files:**
- Modify: `crates/hub/src/toolbar.rs`

**Interfaces:**
- Consumes: `Appearance` (Task 2, no `preset`/`file_icons`), `Settings` (Task 2, no `colors`).
- Produces: a Settings dialog with only Behaviour + the 3 remaining Appearance toggles (colours/icon-set/background now live in `opusdm-theme`).

- [ ] **Step 1: Delete the colour-editing machinery**

Delete these items entirely from `crates/hub/src/toolbar.rs` (they exist only to edit `Settings.colors`/`Appearance.preset`/`Appearance.file_icons`, all removed in Task 2):
- `fn build_color_row` (~line 679)
- `fn add_color_section` (~line 727)
- `struct ColorEntries` and its `impl` (~line 749-792)
- `fn preset_id_for` (~line 794)
- `fn add_theme_section` (~line 828), replaced below

Run `grep -n "fn build_color_row\|fn add_color_section\|struct ColorEntries\|fn preset_id_for\|fn add_theme_section" crates/hub/src/toolbar.rs` first to get exact current line ranges before deleting (the plan's earlier reads were against pre-Task-2 line numbers).

- [ ] **Step 2: Replace `AppearanceChecks` and `add_theme_section` with a smaller "Appearance" section**

```rust
/// The appearance toggles of the settings dialog. Icon set, colours, and
/// background live in the separate `opusdm-theme` app.
struct AppearanceChecks {
    type_indicators: gtk::CheckButton,
    monospace_list: gtk::CheckButton,
    theme_window: gtk::CheckButton,
}

impl AppearanceChecks {
    fn read(&self) -> opusdm_ipc::Appearance {
        opusdm_ipc::Appearance {
            type_indicators: self.type_indicators.is_active(),
            monospace_list: self.monospace_list.is_active(),
            theme_window: self.theme_window.is_active(),
        }
    }
}

/// The "Appearance" section: the non-theme look-and-feel toggles.
fn add_appearance_section(vbox: &gtk::Box, appearance: &opusdm_ipc::Appearance) -> AppearanceChecks {
    let heading = gtk::Label::new(Some("APPEARANCE"));
    heading.set_xalign(0.0);
    heading.add_css_class("heading");
    vbox.append(&heading);

    let checks = AppearanceChecks {
        type_indicators: gtk::CheckButton::with_label("Type indicators after names (/ * @ = |)"),
        monospace_list: gtk::CheckButton::with_label("Monospace file list"),
        theme_window: gtk::CheckButton::with_label("Apply theme colours to the whole window"),
    };
    checks.type_indicators.set_active(appearance.type_indicators);
    checks.monospace_list.set_active(appearance.monospace_list);
    checks.theme_window.set_active(appearance.theme_window);
    for c in [&checks.type_indicators, &checks.monospace_list, &checks.theme_window] {
        vbox.append(c);
    }

    checks
}
```

- [ ] **Step 3: Rewrite `show_settings_dialog`**

```rust
fn show_settings_dialog(parent: &gtk::ApplicationWindow, cmd_tx: &Sender<HubCmd>) {
    let settings_data = settings::load_settings();

    let shell = opusdm_ui::DialogShell::new(parent, "OpusDM Settings")
        .size(460, 300)
        .scrolled();
    let vbox = shell.content();

    let appearance_checks = add_appearance_section(vbox, &settings_data.appearance);
    let auto_refresh = add_behavior_section(vbox, &settings_data.behavior);

    shell.close_button("Cancel");
    let save_btn = shell.button("Save");

    {
        let window = shell.window().clone();
        let cmd_tx = cmd_tx.clone();
        let theme = settings_data.theme.clone();
        save_btn.connect_clicked(move |_| {
            let appearance = appearance_checks.read();
            let behavior = opusdm_ipc::Behavior {
                auto_refresh: auto_refresh.is_active(),
                auto_refresh_debounce_ms: settings_data.behavior.auto_refresh_debounce_ms,
            };
            let _ = cmd_tx.send(HubCmd::UpdateSettings(Box::new(opusdm_ipc::Settings {
                theme: theme.clone(),
                appearance,
                behavior,
            })));
            window.close();
        });
    }

    shell.present();
}
```

- [ ] **Step 4: Add a "Theme..." launcher next to "Settings" in the Desktop submenu**

Mirroring `desktop_config_bin`/`launch_desktop_config` (~line 291-307), add:

```rust
/// Resolve the `opusdm-theme` binary, same lookup strategy as
/// `desktop_config_bin`.
fn theme_config_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("opusdm-theme");
        }
    }
    PathBuf::from("opusdm-theme")
}

/// Spawn the theme configuration app, detached.
fn launch_theme_config() {
    let bin = theme_config_bin();
    if let Err(e) = std::process::Command::new(&bin).spawn() {
        eprintln!("opusdm-hub: cannot launch {}: {e}", bin.display());
    }
}
```

Then, in the Desktop submenu block (the one that appends the "Settings" `menu_button`, ~line 501-510), add right after it:

```rust
        // Theme: launch the standalone opusdm-theme app.
        {
            let menu_ref = menu.clone();
            let btn = menu_button("Theme");
            btn.connect_clicked(move |_| {
                menu_ref.set_visible(false);
                launch_theme_config();
            });
            sub.append(&btn);
        }
```

- [ ] **Step 5: Build the hub crate**

Run: `cargo build -p opusdm-hub`
Expected: compiles (Task 6's `set_icon_set` call in `toolbar.rs` from Task 4 Step 3 will only compile once Task 6 lands - if this crate is built before Task 6, expect that one error only; otherwise clean).

- [ ] **Step 6: Commit**

```bash
git add crates/hub/src/toolbar.rs
git commit -m "feat(hub): move colour/icon-set/background editing out of Settings dialog"
```

---

## Task 6: Bundled Breeze Dark icon subset + `icon_path_for` lookup

**Files:**
- Create: `crates/iconview/assets/breeze-dark/dir.png`
- Create: `crates/iconview/assets/breeze-dark/file.png`
- Create: `crates/iconview/assets/breeze-dark/symlink.png`
- Create: `crates/iconview/assets/breeze-dark/broken.png`
- Create: `crates/iconview/assets/breeze-dark/exec.png`
- Create: `crates/iconview/assets/breeze-dark/archive.png`
- Create: `crates/iconview/assets/breeze-dark/image.png`
- Create: `crates/iconview/assets/breeze-dark/audio.png`
- Create: `crates/iconview/assets/breeze-dark/video.png`
- Create: `crates/iconview/assets/breeze-dark/code.png`
- Create: `crates/iconview/assets/breeze-dark/text.png`
- Create: `crates/iconview/assets/breeze-dark/pdf.png`
- Create: `crates/iconview/src/icon_set.rs`
- Test: `crates/iconview/src/icon_set.rs` (inline `#[cfg(test)]`)
- Modify: `crates/iconview/src/lib.rs`

**Interfaces:**
- Consumes: `IconKind` (existing, `crates/iconview/src/types.rs`), `opusdm_ipc::IconSetId` (Task 1) - add `opusdm-ipc` as a dependency of `opusdm-iconview`.
- Produces: `pub fn icon_bytes_for(kind: IconKind, mode: u32, ext: Option<&str>, set: IconSetId) -> Option<&'static [u8]>` in `crate::icon_set`.

- [ ] **Step 1: Add the `opusdm-iconview -> opusdm-ipc` dependency**

`crates/iconview/Cargo.toml`, in `[dependencies]`:
```toml
opusdm-ipc = { path = "../ipc" }
```

- [ ] **Step 2: Produce the 12 PNG assets**

These are placeholder-quality solid-colour PNGs at this step (32x32, one flat colour per category, generated programmatically) - visually crude but structurally correct and sufficient to make the lookup and rendering pipeline testable; replacing them with real Breeze Dark artwork is a follow-up content task, not a code task, and does not block this plan. Generate them with a small one-off script (not committed):

```bash
python3 - <<'EOF'
import struct, zlib, os

def write_png(path, rgb):
    w = h = 32
    def chunk(tag, data):
        c = tag + data
        return struct.pack('>I', len(data)) + c + struct.pack('>I', zlib.crc32(c))
    sig = b'\x89PNG\r\n\x1a\n'
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)
    raw = b''.join(b'\x00' + bytes(rgb) * w for _ in range(h))
    idat = zlib.compress(raw)
    with open(path, 'wb') as f:
        f.write(sig + chunk(b'IHDR', ihdr) + chunk(b'IDAT', idat) + chunk(b'IEND', b''))

colors = {
    'dir': (61, 174, 233), 'file': (200, 200, 200), 'symlink': (100, 200, 100),
    'broken': (200, 80, 80), 'exec': (80, 200, 80), 'archive': (180, 140, 60),
    'image': (200, 100, 180), 'audio': (180, 60, 180), 'video': (60, 100, 200),
    'code': (100, 180, 220), 'text': (220, 220, 220), 'pdf': (200, 60, 60),
}
os.makedirs('crates/iconview/assets/breeze-dark', exist_ok=True)
for name, rgb in colors.items():
    write_png(f'crates/iconview/assets/breeze-dark/{name}.png', rgb)
EOF
```

Run this from the repo root once; commit the resulting 12 files as binary assets.

- [ ] **Step 3: Write the failing test**

`crates/iconview/src/icon_set.rs`:

```rust
//! Real icon image lookup, sibling of `glyphs::glyph_for`. Reuses the same
//! `IconKind` + extension classification; returns PNG bytes instead of a
//! Nerd Font `char`.

use crate::types::IconKind;
use opusdm_ipc::IconSetId;

const S_IFMT: u32 = 0o170_000;
const EXEC_BITS: u32 = 0o111;

const BREEZE_DIR: &[u8] = include_bytes!("../assets/breeze-dark/dir.png");
const BREEZE_FILE: &[u8] = include_bytes!("../assets/breeze-dark/file.png");
const BREEZE_SYMLINK: &[u8] = include_bytes!("../assets/breeze-dark/symlink.png");
const BREEZE_BROKEN: &[u8] = include_bytes!("../assets/breeze-dark/broken.png");
const BREEZE_EXEC: &[u8] = include_bytes!("../assets/breeze-dark/exec.png");
const BREEZE_ARCHIVE: &[u8] = include_bytes!("../assets/breeze-dark/archive.png");
const BREEZE_IMAGE: &[u8] = include_bytes!("../assets/breeze-dark/image.png");
const BREEZE_AUDIO: &[u8] = include_bytes!("../assets/breeze-dark/audio.png");
const BREEZE_VIDEO: &[u8] = include_bytes!("../assets/breeze-dark/video.png");
const BREEZE_CODE: &[u8] = include_bytes!("../assets/breeze-dark/code.png");
const BREEZE_TEXT: &[u8] = include_bytes!("../assets/breeze-dark/text.png");
const BREEZE_PDF: &[u8] = include_bytes!("../assets/breeze-dark/pdf.png");

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

/// PNG bytes for `kind`/`mode`/`ext` under icon set `set`, or `None` for
/// [`IconSetId::None`] (caller falls back to `glyphs::glyph_for`).
pub fn icon_bytes_for(kind: IconKind, mode: u32, ext: Option<&str>, set: IconSetId) -> Option<&'static [u8]> {
    if set != IconSetId::BreezeDark {
        return None;
    }
    Some(match kind {
        IconKind::Dir => BREEZE_DIR,
        IconKind::Symlink => BREEZE_SYMLINK,
        IconKind::Broken => BREEZE_BROKEN,
        IconKind::File => {
            if mode & EXEC_BITS != 0 && mode & S_IFMT != 0o140_000 {
                BREEZE_EXEC
            } else {
                match ext {
                    Some(e) if ARCHIVE_EXTS.contains(&e) => BREEZE_ARCHIVE,
                    Some(e) if IMAGE_EXTS.contains(&e) => BREEZE_IMAGE,
                    Some(e) if AUDIO_EXTS.contains(&e) => BREEZE_AUDIO,
                    Some(e) if VIDEO_EXTS.contains(&e) => BREEZE_VIDEO,
                    Some(e) if CODE_EXTS.contains(&e) => BREEZE_CODE,
                    Some(e) if TEXT_EXTS.contains(&e) => BREEZE_TEXT,
                    Some("pdf") => BREEZE_PDF,
                    _ => BREEZE_FILE,
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_set_returns_nothing() {
        assert_eq!(icon_bytes_for(IconKind::Dir, 0, None, IconSetId::None), None);
    }

    #[test]
    fn breeze_dark_covers_every_kind() {
        assert_eq!(icon_bytes_for(IconKind::Dir, 0, None, IconSetId::BreezeDark), Some(BREEZE_DIR));
        assert_eq!(icon_bytes_for(IconKind::Symlink, 0, None, IconSetId::BreezeDark), Some(BREEZE_SYMLINK));
        assert_eq!(icon_bytes_for(IconKind::Broken, 0, None, IconSetId::BreezeDark), Some(BREEZE_BROKEN));
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, None, IconSetId::BreezeDark),
            Some(BREEZE_FILE)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_755, None, IconSetId::BreezeDark),
            Some(BREEZE_EXEC)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, Some("rs"), IconSetId::BreezeDark),
            Some(BREEZE_CODE)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, Some("png"), IconSetId::BreezeDark),
            Some(BREEZE_IMAGE)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, Some("pdf"), IconSetId::BreezeDark),
            Some(BREEZE_PDF)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, Some("zip"), IconSetId::BreezeDark),
            Some(BREEZE_ARCHIVE)
        );
    }
}
```

- [ ] **Step 4: Export the module**

`crates/iconview/src/lib.rs`, add:
```rust
pub mod icon_set;
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p opusdm-iconview icon_set::`
Expected: 2 passed. If it fails to compile because the PNG files are missing, re-run Step 2's script.

- [ ] **Step 6: Commit**

```bash
git add crates/iconview/assets crates/iconview/src/icon_set.rs crates/iconview/src/lib.rs crates/iconview/Cargo.toml
git commit -m "feat(iconview): bundle Breeze Dark icon subset and icon_bytes_for lookup"
```

---

## Task 7: `IconField`/`IconItem` render real icons when a theme's icon set is active

**Files:**
- Modify: `crates/iconview/src/item.rs`
- Modify: `crates/iconview/src/field.rs`
- Test: `crates/iconview/src/item.rs` (inline), `crates/iconview/src/field.rs` (inline)

**Interfaces:**
- Consumes: `icon_set::icon_bytes_for` (Task 6), `IconEntry` (existing, has `.ext: Option<String>`, `.mode`, `.kind`).
- Produces: `IconItem::new(entry: &IconEntry, icon_size: i32, icon_set: IconSetId) -> IconItem`; `IconField::set_icon_set(&self, set: IconSetId)`.

- [ ] **Step 1: `IconItem::new` takes an `IconSetId` and prefers a real icon**

In `crates/iconview/src/item.rs`, change the constructor signature and body. Replace:
```rust
    pub fn new(entry: &IconEntry, icon_size: i32) -> IconItem {
```
with:
```rust
    pub fn new(entry: &IconEntry, icon_size: i32, icon_set: opusdm_ipc::IconSetId) -> IconItem {
```

After the existing `let icon = gtk::Label::new(...)` and `let thumb = gtk::Image::new();` block (unchanged), add, right before `let name_label = ...`:

```rust
        if let Some(bytes) =
            crate::icon_set::icon_bytes_for(entry.kind, entry.mode, entry.ext.as_deref(), icon_set)
        {
            let gbytes = glib::Bytes::from_static(bytes);
            if let Ok(texture) = gtk::gdk::Texture::from_bytes(&gbytes) {
                thumb.set_paintable(Some(&texture));
                thumb.set_pixel_size(icon_size);
                thumb.set_visible(true);
                icon.set_visible(false);
            }
        }
```

Add `use gtk::glib;` if not already imported in this file (check the existing `use` block - `glib` is already used elsewhere in the crate via `gtk::glib`, confirm with `grep -n "^use" crates/iconview/src/item.rs`).

Note: a themed icon set here occupies the same `thumb` `gtk::Image` slot a file-content thumbnail (photo preview) later replaces via `set_thumbnail` - that is intended, content thumbnails should win over a generic per-type icon. `clear_thumbnail` (called when the thumbnail-size bucket changes) reverts to the plain glyph rather than the themed icon in this rare resize path; that is a known, acceptable limitation, not a regression to fix here (YAGNI - resizing across the 128px thumbnail-bucket boundary is rare and self-corrects on the next `set_entries`).

- [ ] **Step 2: Update the one call site inside `item.rs` itself (none - `new` is only called externally) and add a unit test**

Add to `crates/iconview/src/item.rs`'s existing `#[cfg(test)]` module (create one if absent - check with `grep -n "#\[cfg(test)\]" crates/iconview/src/item.rs`; this crate's other files each have one, `item.rs` may not):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{IconEntry, IconKind};
    use std::path::PathBuf;

    fn entry(name: &str, kind: IconKind) -> IconEntry {
        IconEntry { name: name.into(), path: PathBuf::from(name), kind, mode: 0o100_644, ext: IconEntry::ext_of(name) }
    }

    #[test]
    fn breeze_dark_shows_a_real_icon_not_the_glyph() {
        gtk::init().ok();
        let item = IconItem::new(&entry("a.png", IconKind::File), 32, opusdm_ipc::IconSetId::BreezeDark);
        assert!(item.inner.thumb.is_visible());
        assert!(!item.inner.icon.is_visible());
    }

    #[test]
    fn none_set_keeps_the_glyph() {
        gtk::init().ok();
        let item = IconItem::new(&entry("a.png", IconKind::File), 32, opusdm_ipc::IconSetId::None);
        assert!(!item.inner.thumb.is_visible());
        assert!(item.inner.icon.is_visible());
    }
}
```

If `gtk::init()` is not the pattern this crate's other GTK-touching tests use (some GTK test suites need a display and skip in headless CI), check `crates/iconview/src/field.rs`'s existing tests first with `grep -n "gtk::init\|#\[test\]" crates/iconview/src/field.rs` and match whatever pattern (or skip-if-headless guard) is already established there, rather than introducing a new one.

- [ ] **Step 3: `IconField::set_icon_set` and threading `icon_set` through `set_entries`**

In `crates/iconview/src/field.rs`, add a field to `Inner`:
```rust
    icon_set: Cell<opusdm_ipc::IconSetId>,
```
Initialize it to `opusdm_ipc::IconSetId::None` wherever `Inner` is constructed in `IconField::new()`.

Change the `IconItem::new(e, size)` call inside `set_entries` to:
```rust
            let it = IconItem::new(e, size, self.inner.icon_set.get());
```

Add the setter, near `set_icon_size`:
```rust
    /// Switch the icon library used for entries without a content thumbnail.
    /// Rebuilds every item.
    pub fn set_icon_set(&self, set: opusdm_ipc::IconSetId) {
        if self.inner.icon_set.get() == set {
            return;
        }
        self.inner.icon_set.set(set);
        let entries = self.inner.entries.borrow().clone();
        self.set_entries(&entries);
    }
```

- [ ] **Step 4: Add `opusdm-ipc` as a dependency where needed**

`crates/iconview/Cargo.toml` already gained `opusdm-ipc` in Task 6 Step 1 - no further change needed here.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p opusdm-iconview`
Expected: all pass, including the two new `item.rs` tests and existing `field.rs` tests (which still call `IconItem::new` only indirectly through `set_entries`, so they need no signature updates).

- [ ] **Step 6: Commit**

```bash
git add crates/iconview/src/item.rs crates/iconview/src/field.rs
git commit -m "feat(iconview): render real icons from the active icon set"
```

---

## Task 8: Lister Name column shows a real icon image

**Files:**
- Modify: `crates/lister/src/glyphs.rs`
- Modify: `crates/lister/src/columns.rs`
- Modify: `crates/lister/src/window.rs` (SettingsUpdate diffing)
- Modify: `crates/lister/src/theme.rs`
- Test: `crates/lister/src/glyphs.rs` (inline)

**Interfaces:**
- Consumes: `icon_bytes_for` (Task 6), `Settings.theme.icon_set` (Task 2), `IconKind`/`kind_of` (existing in `glyphs.rs`).
- Produces: `glyphs::icon_bytes_for_row(row: &Row, set: IconSetId) -> Option<&'static [u8]>`; `decorate_name` keeps its current signature but drops the glyph char when a real icon set is active (the image is drawn separately by the column, not embedded in text).

- [ ] **Step 1: Update `crates/lister/src/theme.rs` and `window.rs` for the new `Settings` shape**

`crates/lister/src/theme.rs` line ~7 (`let c = &settings.colors;`) becomes:
```rust
    let c = &settings.theme.colors;
```
Every `preset in opusdm_ipc::ThemePreset::ALL` loop (lines ~119, ~195) is removed - those tests exercised every named colour preset, which no longer exist. Replace each such test with a single check against `opusdm_ipc::Theme::default().colors`, e.g. (adapt to the exact surrounding test body found via `grep -n "fn " crates/lister/src/theme.rs`):
```rust
    #[test]
    fn default_theme_colors_produce_valid_css() {
        let mut settings = opusdm_ipc::Settings::default();
        settings.theme.colors = opusdm_ipc::Theme::default().colors;
        // ... existing per-test assertions on the CSS this file builds, unchanged ...
    }
```
Any `settings.colors.selected_bg = ...` becomes `settings.theme.colors.selected_bg = ...`. Any `appearance: opusdm_ipc::Appearance { file_icons: true, ... }` literal drops the `file_icons` field (removed in Task 2).

`crates/lister/src/window.rs` line ~916 (`old.file_icons != new.file_icons`) - replace with a check on the theme's icon set, since that is now what controls whether the Name column repaints its icon:
```rust
                    old.theme.icon_set != new.theme.icon_set
```
Read the surrounding function (`sed -n '900,925p' crates/lister/src/window.rs`) first to see what this condition guards (likely "does the Name column need a full rebind on SettingsUpdate") and confirm the replacement field serves the same purpose.

- [ ] **Step 2: `glyphs.rs` gains a real-icon lookup for a `Row`**

Add to `crates/lister/src/glyphs.rs`, after `kind_of`:
```rust
/// Real icon bytes for `row` under icon set `set`, or `None` (caller falls
/// back to the glyph). Extension is derived the same way `IconEntry` does.
pub fn icon_bytes_for_row(row: &Row, set: opusdm_ipc::IconSetId) -> Option<&'static [u8]> {
    let ext = opusdm_iconview::IconEntry::ext_of(&row.name);
    opusdm_iconview::icon_set::icon_bytes_for(kind_of(row), row.mode, ext.as_deref(), set)
}
```
`decorate_name` keeps embedding the Nerd Font glyph exactly as before when `icons` is true and no real icon set overrides it - the caller (Task 8 Step 3) decides whether to call `decorate_name` with `icons: false` (real icon set active, image drawn separately) or `icons: true` (no icon set, keep today's Nerd Font behaviour) plus `indicators` unchanged either way. No change to `decorate_name`'s own signature or logic.

Add a test:
```rust
    #[test]
    fn icon_bytes_for_row_is_none_without_a_real_icon_set() {
        assert_eq!(icon_bytes_for_row(&row("a.rs"), opusdm_ipc::IconSetId::None), None);
    }

    #[test]
    fn icon_bytes_for_row_matches_the_extension() {
        assert!(icon_bytes_for_row(&row("a.rs"), opusdm_ipc::IconSetId::BreezeDark).is_some());
    }
```

- [ ] **Step 3: `columns.rs` Name column cell becomes an image+label box when a real icon set is active**

The factory's `connect_setup` currently creates a plain `gtk::Label` as the cell child for every column (around line 60-113, ending `item.set_child(Some(&label));`). For `ColKind::Name` specifically, build a horizontal `gtk::Box` containing a `gtk::Image` and a `gtk::Label` instead, so a real icon and the text can coexist without concatenating the icon into the string.

Read the full `connect_setup` closure first (`sed -n '1,115p' crates/lister/src/columns.rs`) to see how `label` is currently captured for the click gesture, then change the `ColKind::Name` branch of setup to:
```rust
                if matches!(kind, ColKind::Name) {
                    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                    let icon = gtk::Image::new();
                    icon.set_pixel_size(16);
                    icon.set_visible(false);
                    let label = gtk::Label::new(None);
                    label.set_xalign(0.0);
                    cell.append(&icon);
                    cell.append(&label);
                    // ... existing click-gesture wiring, now attached to `label` inside `cell` exactly as before ...
                    item.set_child(Some(&cell));
                } else {
                    // ... existing plain-Label setup, unchanged ...
                    item.set_child(Some(&label));
                }
```
(This plan gives the shape of the change; the exact click-gesture code block that currently runs unconditionally must move inside the `else`-vs-`ColKind::Name` split without altering its logic - copy it verbatim into the `ColKind::Name` branch, referencing the new `label` inside `cell`.)

In `connect_bind`, the `ColKind::Name` arm currently does:
```rust
            ColKind::Name => {
                let (icons, indicators) = ui_getter_bind()
                    .map(|ui| {
                        let a = &ui.settings.borrow().appearance;
                        (a.file_icons, a.type_indicators)
                    })
                    .unwrap_or((false, false));
                label.set_xalign(0.0);
                label.set_text(&crate::glyphs::decorate_name(&row, icons, indicators));
            }
```
Replace it with logic that reads the theme's icon set, sets the image when a real icon is available, and falls back to the glyph-in-text behaviour otherwise:
```rust
            ColKind::Name => {
                let cell = item.child().and_downcast::<gtk::Box>().unwrap();
                let icon_widget = cell.first_child().and_downcast::<gtk::Image>().unwrap();
                let label = cell.last_child().and_downcast::<gtk::Label>().unwrap();
                let (icon_set, indicators) = ui_getter_bind()
                    .map(|ui| {
                        let s = ui.settings.borrow();
                        (s.theme.icon_set, s.appearance.type_indicators)
                    })
                    .unwrap_or((opusdm_ipc::IconSetId::None, false));
                label.remove_css_class("row-dir");
                label.remove_css_class("row-file");
                label.add_css_class(if row.is_dir { "row-dir" } else { "row-file" });
                apply_selected_class(&label, item.is_selected());
                label.set_xalign(0.0);
                if let Some(bytes) = crate::glyphs::icon_bytes_for_row(&row, icon_set) {
                    let gbytes = gtk::glib::Bytes::from_static(bytes);
                    if let Ok(texture) = gtk::gdk::Texture::from_bytes(&gbytes) {
                        icon_widget.set_paintable(Some(&texture));
                        icon_widget.set_visible(true);
                    }
                    label.set_text(&crate::glyphs::decorate_name(&row, false, indicators));
                } else {
                    icon_widget.set_visible(false);
                    label.set_text(&crate::glyphs::decorate_name(&row, false, indicators));
                }
                return;
            }
```
The generic pre-match block at the top of `connect_bind` (`label.remove_css_class(...)`/`add_css_class(...)`/`apply_selected_class(...)`, currently run once for every column before the `match kind` in the existing code) must be skipped for `ColKind::Name` since the code above now does it on the correct widget (`label` inside `cell`, not a bare `label` downcast that would panic on a `Box` child) - restructure the `match` so the `ColKind::Name` arm runs standalone (as sketched, ending in `return`) before the shared pre-match block executes for the other arms, or guard the shared block with `if !matches!(kind, ColKind::Name) { ... }`. Read the current full `connect_bind` body (`sed -n '117,166p' crates/lister/src/columns.rs`) and restructure conservatively - the goal is: other columns behave exactly as before, `ColKind::Name` handles its own CSS classes and never calls `.and_downcast::<gtk::Label>()` on the `Box` child.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p opusdm-lister`
Expected: all pass, including the two new `glyphs.rs` tests.

- [ ] **Step 5: Manual GTK smoke check**

Run: `cargo run -p opusdm-hub` (or however this workspace normally launches the hub + a Lister - check `CHANGES.md`/README for the exact dev-run command), open a Lister, confirm the Name column still renders correctly with no icon set (today's behaviour unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/lister/src/glyphs.rs crates/lister/src/columns.rs crates/lister/src/window.rs crates/lister/src/theme.rs
git commit -m "feat(lister): render real icons in the Name column when a theme icon set is active"
```

---

## Task 9: `opusdm-desktop-config` exposes a library for reuse

**Files:**
- Create: `crates/desktop-config/src/lib.rs`
- Modify: `crates/desktop-config/src/main.rs`
- Modify: `crates/desktop-config/src/ui.rs` (visibility only)
- Modify: `crates/desktop-config/src/model.rs` (visibility + `Background`-only load/apply, since `DesktopSettings` no longer carries it)

**Interfaces:**
- Produces: `opusdm_desktop_config::ui::{BackgroundSection, ColorRow}` (both made `pub`), `opusdm_desktop_config::model::{hex_to_rgba, rgba_to_hex}` (already `pub fn`, now reachable from outside the crate).

- [ ] **Step 1: `model.rs` no longer loads/writes `Background` as part of `DesktopSettings`**

`opus-desktop-config` still edits icon_size/grid/desktop_dir via `DesktopSettings` (Task 3 left those fields in place) but no longer edits the background - that moved to `opusdm-theme` (Task 10). Remove `BackgroundSection` usage and the background wiring from `crates/desktop-config/src/main.rs`'s `build_window` (the `bg` section and its `outer.append(&bg.root)` / separator), since `opus-desktop-config` no longer edits background. Leave `GridSection`/`FolderSection` untouched.

Remove `apply_writes_the_file_when_no_hub_is_running`'s assertion on background if it exists (check: it currently only asserts `icon_size`, per the file read earlier - no change needed there).

`crates/desktop-config/src/ui.rs` keeps `BackgroundSection` and `ColorRow` defined (they are needed by `opusdm-theme`), but they are no longer instantiated from `crates/desktop-config/src/main.rs`. Make both `pub`:
```rust
pub struct BackgroundSection { ... }   // already pub
struct ColorRow { ... }                 // change to: pub(crate) -> pub, and pub its `new`/`hex` methods
```
Change `struct ColorRow` to `pub struct ColorRow`, its fields stay private, its `impl` block's `fn new` and `fn hex` become `pub fn new`/`pub fn hex`.

- [ ] **Step 2: Add `src/lib.rs`**

```rust
//! Library surface of `opus-desktop-config`, reused by `opusdm-theme` for the
//! background editor and its colour-entry widgets.

pub mod model;
pub mod ui;
```

- [ ] **Step 3: `main.rs` uses the library modules instead of local `mod` declarations**

Change the top of `crates/desktop-config/src/main.rs` from:
```rust
mod model;
mod ui;
```
to:
```rust
use opusdm_desktop_config::{model, ui};
```
Remove the `bg`/`BackgroundSection` construction and its two `outer.append` calls (background editing moved to `opusdm-theme`); `model::load()`/`model::apply()` keep editing `DesktopSettings` (icon_size/grid/desktop_dir only, per Task 3).

- [ ] **Step 4: Cargo.toml gains a library target**

`crates/desktop-config/Cargo.toml` - a `src/lib.rs` alongside `src/main.rs` is picked up automatically by Cargo as the crate's library target (default path `src/lib.rs`) without any manifest change, as long as `[[bin]]` still points at `src/main.rs`. No edit needed here beyond what already exists, but verify: `cargo build -p opusdm-desktop-config` must produce both a lib and the `opus-desktop-config` binary.

- [ ] **Step 5: Run the tests and build**

Run: `cargo build -p opusdm-desktop-config && cargo test -p opusdm-desktop-config`
Expected: builds clean, existing tests (`hex_roundtrips_through_rgba`, `bad_hex_is_opaque_black`, `apply_writes_the_file_when_no_hub_is_running`) still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/desktop-config/src/lib.rs crates/desktop-config/src/main.rs crates/desktop-config/src/ui.rs crates/desktop-config/src/model.rs
git commit -m "refactor(desktop-config): expose ui/model as a library for opusdm-theme reuse; drop background editing"
```

---

## Task 10: `opusdm-theme` app

**Files:**
- Create: `crates/theme-config/Cargo.toml`
- Create: `crates/theme-config/src/main.rs`
- Create: `crates/theme-config/src/model.rs`
- Create: `crates/theme-config/src/ui.rs`
- Test: `crates/theme-config/src/model.rs` (inline)
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Consumes: `opusdm_ipc::{Theme, IconSetId, ToHub}`, `opusdm_ipc::sockpath::{hub_sock_path, probe_live_hub}`, `opusdm_ipc::frame::send`, `opusdm_desktop_config::ui::{BackgroundSection, ColorRow}` (Task 9), `opusdm_desktop_config::model::{hex_to_rgba, rgba_to_hex}` (Task 9).
- Produces: binary `opusdm-theme`.

- [ ] **Step 1: Workspace member**

`Cargo.toml` (root), add `"crates/theme-config"` to `members`.

- [ ] **Step 2: `Cargo.toml`**

```toml
[package]
name = "opusdm-theme-config"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "opusdm-theme"
path = "src/main.rs"

[dependencies]
opusdm-ipc = { path = "../ipc" }
opusdm-desktop-config = { path = "../desktop-config" }
gtk4.workspace = true
serde_json.workspace = true
dirs.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 3: `model.rs` - load/apply the whole `Theme`**

```rust
//! Load the current theme for the config window, and apply edits: send to a
//! running hub over the socket, or write settings.json's `theme` field
//! directly when no hub is up.

use opusdm_ipc::{Settings, Theme};
use std::path::PathBuf;

/// Path to `~/.config/opusdm/settings.json`. Same resolution as the hub.
pub fn config_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("settings.json");
    path
}

/// Load the current `Settings` (defaults if missing/unparseable), return its
/// `theme`.
pub fn load() -> Theme {
    let path = config_path();
    let parsed = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<Settings>(&c).ok());
    parsed.unwrap_or_default().theme
}

/// What [`apply`] did.
#[derive(Debug)]
pub enum ApplyOutcome {
    SentToHub,
    WroteFile,
    Failed(String),
}

/// Send `theme` to a running hub, or persist it into settings.json (creating
/// a fresh default `Settings` if none exists yet) if none is running.
pub fn apply(theme: &Theme) -> ApplyOutcome {
    let sock = opusdm_ipc::sockpath::hub_sock_path();
    if opusdm_ipc::sockpath::probe_live_hub(&sock) {
        match send_to_hub(&sock, theme) {
            Ok(()) => return ApplyOutcome::SentToHub,
            Err(e) => eprintln!("opusdm-theme: hub send failed, writing file: {e}"),
        }
    }
    match write_file(theme) {
        Ok(()) => ApplyOutcome::WroteFile,
        Err(e) => ApplyOutcome::Failed(e.to_string()),
    }
}

fn send_to_hub(sock: &std::path::Path, theme: &Theme) -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(sock)?;
    opusdm_ipc::frame::send(&mut stream, &opusdm_ipc::ToHub::SetTheme(theme.clone()))
}

fn write_file(theme: &Theme) -> std::io::Result<()> {
    let path = config_path();
    let mut settings: Settings = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    settings.theme = theme.clone();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&settings)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_writes_the_theme_field_when_no_hub_is_running() {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        std::env::set_var("OPUSDM_HUB_SOCK", td.path().join("definitely-absent.sock"));
        let mut t = Theme::default();
        t.icon_set = opusdm_ipc::IconSetId::BreezeDark;
        match apply(&t) {
            ApplyOutcome::WroteFile => {}
            other => panic!("expected WroteFile, got {other:?}"),
        }
        let written = std::fs::read_to_string(td.path().join("opusdm").join("settings.json")).unwrap();
        assert!(written.contains("breeze-dark"));
    }

    #[test]
    fn apply_preserves_other_settings_fields_when_writing_the_file() {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        std::env::set_var("OPUSDM_HUB_SOCK", td.path().join("definitely-absent.sock"));
        let mut existing = Settings::default();
        existing.appearance.monospace_list = true;
        std::fs::create_dir_all(td.path().join("opusdm")).unwrap();
        std::fs::write(
            td.path().join("opusdm").join("settings.json"),
            serde_json::to_string(&existing).unwrap(),
        )
        .unwrap();

        apply(&Theme::default());

        let written = std::fs::read_to_string(td.path().join("opusdm").join("settings.json")).unwrap();
        let back: Settings = serde_json::from_str(&written).unwrap();
        assert!(back.appearance.monospace_list, "unrelated settings must survive a theme-only write");
    }
}
```

Check `opusdm_ipc::sockpath::hub_sock_path` actually honours `OPUSDM_HUB_SOCK` (the existing `desktop-config` test relies on this same env var - confirm with `grep -n "OPUSDM_HUB_SOCK" crates/ipc/src/sockpath.rs`) before assuming the test env vars above take effect.

- [ ] **Step 4: `ui.rs` - the icon-set picker**

```rust
//! The ICON SET section: a dropdown listing every `IconSetId`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::IconSetId;

/// The ICON SET section.
pub struct IconSetSection {
    pub root: gtk::Box,
    dropdown: gtk::DropDown,
}

impl IconSetSection {
    pub fn new(current: IconSetId) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("ICON SET"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let labels: Vec<&str> = IconSetId::ALL.iter().map(|s| s.label()).collect();
        let dropdown = gtk::DropDown::from_strings(&labels);
        let selected = IconSetId::ALL.iter().position(|s| *s == current).unwrap_or(0);
        dropdown.set_selected(selected as u32);
        root.append(&dropdown);

        Self { root, dropdown }
    }

    /// The currently selected icon set.
    pub fn value(&self) -> IconSetId {
        IconSetId::ALL[self.dropdown.selected() as usize]
    }
}
```

- [ ] **Step 5: `main.rs` - window wiring `IconSetSection` + reused `BackgroundSection`/`ColorRow` + colour rows**

```rust
//! `opusdm-theme`: edit the OpusDM theme - icon set, Lister colours, and
//! desktop background - and apply it live to the running desktop.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_desktop_config::ui::BackgroundSection;
use std::rc::Rc;

mod model;
mod ui;

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.Theme").build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &gtk::Application) {
    let theme = model::load();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Theme")
        .default_width(480)
        .default_height(680)
        .build();

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    outer.set_margin_top(12);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);

    let icon_set = Rc::new(ui::IconSetSection::new(theme.icon_set));
    outer.append(&icon_set.root);
    outer.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let colors = build_color_section(&theme.colors);
    outer.append(&colors.root);
    outer.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let bg = Rc::new(BackgroundSection::new(&window, &theme.background));
    outer.append(&bg.root);

    let note = gtk::Label::new(None);
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_visible(false);
    outer.append(&note);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    buttons.append(&cancel);
    buttons.append(&apply);
    outer.append(&buttons);

    window.set_child(Some(&outer));

    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let icon_set = icon_set.clone();
        let colors = colors.clone();
        let bg = bg.clone();
        let note = note.clone();
        apply.connect_clicked(move |_| {
            let theme = opusdm_ipc::Theme {
                icon_set: icon_set.value(),
                colors: colors.read(),
                background: bg.read(),
            };
            match model::apply(&theme) {
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
    }

    window.present();
}
```

`BackgroundSection` needs a `read() -> opusdm_ipc::Background` accessor for this to compile - check whether it already has one (`grep -n "fn read" crates/desktop-config/src/ui.rs`); `opus-desktop-config`'s own `main.rs` Apply/Save handlers must already read it back somehow, so reuse that exact existing method rather than adding a new one.

`build_color_section` builds the 5 `ColorRow`-pairs (SOURCE/DEST/DIRECTORY/FILE/SELECTION) the way `crates/hub/src/toolbar.rs`'s old `add_color_section` did (removed in Task 5) - reimplement it here in `crates/theme-config/src/ui.rs` using the reused `opusdm_desktop_config::ui::ColorRow`, not the deleted hub version:

```rust
use opusdm_desktop_config::ui::ColorRow;

/// One "label" + bg/fg colour row pair, e.g. SOURCE.
pub struct ColorSection {
    pub root: gtk::Box,
    bg: ColorRow,
    fg: ColorRow,
}

/// All ten colour rows, grouped into their five sections.
pub struct ColorsSection {
    pub root: gtk::Box,
    source: ColorSection,
    dest: ColorSection,
    dir: ColorSection,
    file: ColorSection,
    selected: ColorSection,
}

fn color_section(window: &gtk::ApplicationWindow, title: &str, bg_hex: &str, fg_hex: &str) -> ColorSection {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    root.append(&heading);
    let bg = ColorRow::new(window, "Background:", bg_hex);
    let fg = ColorRow::new(window, "Text:", fg_hex);
    root.append(&bg.root);
    root.append(&fg.root);
    ColorSection { root, bg, fg }
}

impl ColorsSection {
    pub fn new(window: &gtk::ApplicationWindow, c: &opusdm_ipc::ColorSettings) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("COLOURS"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let source = color_section(window, "SOURCE", &c.source_bg, &c.source_fg);
        let dest = color_section(window, "DEST", &c.dest_bg, &c.dest_fg);
        let dir = color_section(window, "DIRECTORY", &c.dir_bg, &c.dir_fg);
        let file = color_section(window, "FILE", &c.file_bg, &c.file_fg);
        let selected = color_section(window, "SELECTION", &c.selected_bg, &c.selected_fg);

        for s in [&source, &dest, &dir, &file, &selected] {
            root.append(&s.root);
        }

        Self { root, source, dest, dir, file, selected }
    }

    pub fn read(&self) -> opusdm_ipc::ColorSettings {
        opusdm_ipc::ColorSettings {
            source_bg: self.source.bg.hex(),
            source_fg: self.source.fg.hex(),
            dest_bg: self.dest.bg.hex(),
            dest_fg: self.dest.fg.hex(),
            dir_bg: self.dir.bg.hex(),
            dir_fg: self.dir.fg.hex(),
            file_bg: self.file.bg.hex(),
            file_fg: self.file.fg.hex(),
            selected_bg: self.selected.bg.hex(),
            selected_fg: self.selected.fg.hex(),
        }
    }
}
```

Update `main.rs`'s `build_color_section(&theme.colors)` call to `ui::ColorsSection::new(&window, &theme.colors)`, and `colors.read()` stays the same method name (`ColorsSection::read`). Fix the sketch in Step 5 accordingly: `let colors = Rc::new(ui::ColorsSection::new(&window, &theme.colors));`.

- [ ] **Step 6: Run the tests and build**

Run: `cargo build -p opusdm-theme-config && cargo test -p opusdm-theme-config`
Expected: builds clean, both `model.rs` tests pass.

- [ ] **Step 7: Manual smoke check**

Run: `cargo run -p opusdm-theme-config`. Confirm the window shows Icon Set / Colours / Background sections, Cancel closes it, Apply with no hub running reports "saved, effective on next login" and writes `~/.config/opusdm/settings.json`'s `theme` field.

- [ ] **Step 8: Commit**

```bash
git add crates/theme-config Cargo.toml
git commit -m "feat: add opusdm-theme app for icon set, colours, and background"
```

---

## Task 11: Full workspace build and end-to-end manual check

**Files:** none (verification only)

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: clean build, no warnings about unused `ThemePreset`/`PRESET_CUSTOM`/`file_icons`/`preset` (all removed).

- [ ] **Step 2: Full workspace tests**

Run: `cargo test --workspace`
Expected: all pass.

- [ ] **Step 3: Manual end-to-end check**

1. Remove any existing `~/.config/opusdm/settings.json` and `desktop.json` (or point `XDG_CONFIG_HOME` at a scratch dir) to start from a clean `Theme::default()`.
2. Launch the hub (`cargo run -p opusdm-hub`, or the project's normal dev-run command).
3. Open a Lister, confirm it shows with default (no icon set) appearance.
4. Right-click the desktop -> Desktop -> Theme, or `cargo run -p opusdm-theme-config` directly.
5. Set Icon Set to "Breeze Dark", change a colour, change the background to a solid colour different from default, click Apply.
6. Confirm: the note says "Applied to the running desktop"; the open Lister's Name column now shows icon images and the new colours without restarting; the desktop background changes immediately.
7. Close and reopen the Lister; confirm it comes up already themed (via `Welcome`'s `settings`).
8. Quit the hub, relaunch it; confirm the theme persisted (`~/.config/opusdm/settings.json` has the `theme` object).

- [ ] **Step 4: Commit** (only if Step 3 uncovered fixes)

```bash
git add -A
git commit -m "fix: address issues found in end-to-end theme verification"
```
