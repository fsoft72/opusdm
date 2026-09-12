# Theme support: icon set + colors + desktop background, `opusdm-theme`

Date: 2026-09-12
Status: approved for planning

## Goal

Full theming for OpusDM. A theme bundles three things a user picks together:

1. **Icon library** - which icon set the Lister and desktop use for files and
   folders. Ships with one set, Breeze Dark (KDE), bundled in-repo.
2. **Colors** - the Lister's colour scheme (source/dest/dir/file/selected
   colours). Same 10 fields as today's `ColorSettings`.
3. **Desktop background** - solid / gradient / image, same as today's
   `Background`.

A new standalone app, `opusdm-theme`, edits all three and talks to the hub
over the existing IPC socket. Pressing "Apply" applies the whole bundle to
the running desktop immediately (Listers repaint, desktop repaints), the same
apply-live-or-write-file-if-no-hub pattern `opus-desktop-config` already uses.

## Non-goals

- No backward compatibility. This replaces the old colours/background
  storage outright - no migration code, no dual-read of old shapes. There is
  no existing user base to preserve state for.
- No saved/named custom themes list in v1. The user edits icon set, colours,
  and background directly (same editing model `ColorSettings`/`Background`
  already have); YAGNI until asked.
- No new icon sets beyond Breeze Dark in v1. `IconSetId` is an enum with room
  to grow, same pattern `ThemePreset` used.
- No change to desktop icon grid sizing/spacing/folder settings
  (`DesktopSettings.icon_size`, `grid_spacing`, `grid_gap`, `desktop_dir`) -
  those stay in `opus-desktop-config`, untouched.
- No full Breeze Dark icon set (thousands of files) - only the subset needed
  to cover the categories `iconview`'s existing extension table already
  classifies (~15-20 icons: folder, file, image, archive, text, executable,
  audio, video, code, pdf, link, socket, pipe, block device).

## Current state (as read)

- `crates/ipc/src/theme.rs` defines `ThemePreset` (6 built-in colour presets,
  `id()`/`label()`/`colors()`/`from_id()`). This file is rewritten, not kept.
- `crates/ipc/src/proto.rs`:
  - `ColorSettings` - the 10 hex-string colour fields. **Kept as-is**,
    relocated to live inside the new `Theme` struct.
  - `Appearance` - has `preset: String` (last-applied `ThemePreset` id or
    `"custom"`) plus unrelated switches (`file_icons`, `type_indicators`,
    `monospace_list`, `theme_window`). `preset` and `file_icons` are replaced
    by theme fields (see below); the other three switches are untouched.
  - `Settings { colors, appearance, behavior }` - wire format, on-disk
    shape, and hub in-memory state all in one struct.
  - `ToHub` / `ToLister` enums, framed over a Unix socket
    (`crates/ipc/src/frame.rs`, `sockpath.rs`).
- `crates/ipc/src/desktop.rs`: `DesktopSettings { background: Background,
  icon_size, grid_spacing, grid_gap, desktop_dir }`, persisted to
  `~/.config/opusdm/desktop.json`, owned by the hub. `Background` /
  `BackgroundMode` / `ImageFit` are the solid/gradient/image model.
- `crates/desktop-config` (`opus-desktop-config`): GTK app, exact template
  for `opusdm-theme`. `model.rs` has `load()` (read file, sanitize),
  `apply()` (send `SetDesktopSettings` to hub if live, else write file),
  `ApplyOutcome`. `main.rs` builds sections, wires Cancel/Apply/Save buttons.
- `crates/iconview/src/glyphs.rs`: `glyph_for(kind, mode, name)` maps an
  entry to a Nerd Font `char` via `IconKind` + extension table
  (`EXT_GLYPHS`). Pure, GTK-free. Used by both `crates/lister/src/glyphs.rs`
  and the desktop icon grid (via `iconview`).
- The hub (`crates/hub/src/settings.rs`, `server.rs`) is the sole writer of
  `settings.json` and `desktop.json`, and the sole broadcaster of
  `ToLister::SettingsUpdate`. `SetDesktopSettings` is handled by the hub:
  sanitize, persist, apply live (repaint desktop), no reply.

## Design

### 1. New `Theme` type replaces `ThemePreset`

`crates/ipc/src/theme.rs` is rewritten:

```rust
/// Which bundled icon library the Lister and desktop use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconSetId {
    /// No icon images: Nerd Font glyphs (today's behaviour) or plain text.
    None,
    /// Breeze Dark (KDE), bundled subset.
    BreezeDark,
}

impl IconSetId {
    pub const ALL: [IconSetId; 2] = [IconSetId::None, IconSetId::BreezeDark];
    pub fn id(self) -> &'static str { ... }       // "none" / "breeze-dark"
    pub fn label(self) -> &'static str { ... }     // "None (text glyphs)" / "Breeze Dark"
    pub fn from_id(id: &str) -> Option<IconSetId> { ... }
}

/// A theme bundle: icon set + colours + desktop background. What
/// `opusdm-theme` edits and applies as one unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub icon_set: IconSetId,
    pub colors: crate::proto::ColorSettings,
    pub background: crate::desktop::Background,
}

impl Default for Theme {
    fn default() -> Self {
        Theme { icon_set: IconSetId::None, colors: ColorSettings::default(), background: Background::default() }
    }
}
```

No `preset`/named-theme id field - v1 has no saved theme list, so there is
nothing to name. `PRESET_CUSTOM` and the whole "is this preset or custom"
tracking goes away with it.

The 6 built-in colour combinations `ThemePreset` used to hardcode
(AmigaWorkbench, DopusClassic, Cyberpunk, PhosphorGreen, PhosphorAmber) are
dropped - the new colour editor is direct hex editing only, matching the
"no named presets in v1" decision. (If missed: flag before implementation,
since this is a visible feature removal, not just a rename.)

### 2. `Settings` and `DesktopSettings` restructured

`crates/ipc/src/proto.rs`:

- `Appearance` loses `preset` and `file_icons`. Keeps `type_indicators`,
  `monospace_list`, `theme_window`.
- `Settings` loses `colors: ColorSettings`; gains `theme: Theme`.
  ```rust
  pub struct Settings { pub theme: Theme, pub appearance: Appearance, pub behavior: Behavior }
  ```

`crates/ipc/src/desktop.rs`:

- `DesktopSettings` loses `background: Background`. Keeps `icon_size,
  grid_spacing, grid_gap, desktop_dir`.
- `Background`/`BackgroundMode`/`ImageFit`/`background_css` stay in this
  file (still the desktop-painting model), just no longer nested under
  `DesktopSettings` - nested under `Theme` instead.

On-disk shapes change accordingly: `settings.json` gains a `"theme"` object
and drops `"colors"` + `appearance.preset`/`file_icons`; `desktop.json` drops
`"background"`. No `#[serde(default)]` migration shim - a field simply
resets to `Theme::default()` / `DesktopSettings::default()`'s background if
absent, which is the existing `#[serde(default)]` behavior on `Settings`
and `DesktopSettings`, not new code.

### 3. IPC: one atomic `SetTheme` message

`ToHub` (proto.rs) gains:

```rust
/// Sent by opusdm-theme: replace the hub's theme (icon set, colours,
/// background). The hub sanitises, persists to both settings.json and
/// desktop.json, and applies live. No reply.
SetTheme(crate::theme::Theme),
```

One message, not three, so the hub applies icon set + colours + background
as a single unit - no window where a Lister sees new colours but the old
icon set, or a partial apply if one write failed. Hub handling (in
`crates/hub/src/settings.rs` / wherever `SetDesktopSettings` is currently
handled, alongside it):

1. Sanitize `Theme` (background sanitisation already exists on
   `Background`/`DesktopSettings::sanitized`; extend/reuse for `Theme`).
2. Update in-memory `Settings.theme`, persist `settings.json`.
3. Update in-memory `DesktopSettings` (background field lives in `Theme`
   now, so the hub copies `theme.background` into its `DesktopSettings`
   view for the desktop painter), persist `desktop.json`.
4. Broadcast `ToLister::SettingsUpdate(Settings)` to every connected Lister.
5. Repaint the desktop window live (background + icon set), same code path
   `SetDesktopSettings` uses today for background.

`Welcome` (`ToLister`) already carries `settings: Settings`, so a freshly
connecting Lister gets the current theme for free - no new message needed
there.

### 4. Icon rendering: `iconview` gets a real-icon lookup

`crates/iconview/src/glyphs.rs` gains a sibling function next to
`glyph_for`:

```rust
/// Path to this entry's icon image under the given icon set, or `None` for
/// `IconSetId::None` (caller falls back to `glyph_for`).
pub fn icon_path_for(kind: IconKind, mode: /* same as glyph_for */, name: &str, set: IconSetId) -> Option<&'static Path>
```

Reuses the exact same `IconKind`/extension classification `glyph_for`
already computes - one classification, two lookup tables (char vs path), no
duplicated file-type logic. Bundled assets live at
`crates/iconview/assets/breeze-dark/*.svg`, embedded with `rust-embed` (or
`include_dir!`, matching whatever embedding approach - if none used yet in
this codebase, `rust-embed` is the simpler/more common choice) so the
binary is self-contained, no runtime file dependency.

Both call sites switch on `icon_set != IconSetId::None`:

- `crates/lister/src/glyphs.rs` - render a `gtk::Image`/texture from
  `icon_path_for` instead of the glyph char, when set.
- Desktop icon grid (via `iconview`) - same swap, one shared code path
  since both already route through `iconview`.

### 5. `opusdm-theme` app

New crate `crates/theme-config` (binary `opusdm-theme`), structured exactly
like `crates/desktop-config`:

- `main.rs` - `gtk::Application`, one window, three sections (icon set
  picker, colour editor, background editor - background editor is the
  existing `BackgroundSection` widget from `desktop-config`, reused or
  moved to a shared location since it's needed unchanged), Cancel/Apply/Save
  buttons.
- `model.rs`:
  - `load() -> Theme` - read `settings.json` (theme) + `desktop.json`
    (background) is wrong now that background lives in `Theme` -
    **load reads `settings.json`'s `theme` field only**, since after the
    restructure that's where background lives too.
  - `apply(&Theme) -> ApplyOutcome` - send `SetTheme` to hub if live
    (`probe_live_hub`), else write `settings.json`'s `theme` field directly
    (read-modify-write the existing file, or write a fresh default
    `Settings` if none exists).
  - Colour editor reuses the same `rgba_to_hex`/`hex_to_rgba` helpers
    `desktop-config` has (move to a shared location, e.g. `opusdm-ipc` or a
    small shared crate, to avoid duplication - both apps need them now).

### Testing

- `crates/ipc`: roundtrip tests for `SetTheme` (frame ser/deser), same
  pattern as existing `roundtrip_tohub`/`every_tohub_variant_roundtrips`.
  `Theme::default()` sanity test (colours are valid hex, matching the
  existing `ColorSettings` test pattern minus the per-preset loop, since
  presets are gone).
- `crates/iconview`: `icon_path_for` returns `None` for `IconSetId::None`
  and `Some` for every `IconKind` under `BreezeDark` (no missing mapping),
  mirroring the existing "every preset defines every colour" exhaustiveness
  test style.
- `crates/theme-config`: `apply()` writes-file-when-no-hub test, same shape
  as `desktop-config`'s `apply_writes_the_file_when_no_hub_is_running`.
- Manual: launch hub, launch `opusdm-theme`, switch icon set + colour +
  background, Apply, confirm Lister and desktop repaint immediately without
  restart.
