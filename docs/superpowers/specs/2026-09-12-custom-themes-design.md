# Custom themes: design spec

Date: 2026-09-12
Status: approved for planning

## Problem

OpusDM currently ships exactly one selectable icon set (`IconSetId::None` /
`BreezeDark`, a closed enum) plus a `Theme` bundle (icon set + `ColorSettings`
+ `Background`) edited by `opusdm-theme` and applied atomically via
`ToHub::SetTheme`. There is no way for a user to install or author their own
theme (icons, colours, background, desktop grid metrics) as a self-contained,
shareable unit.

## Goal

Let a user **import a theme archive** (`.zip`, `.tar.gz`/`.tgz`, `.tar.bz2`)
through `opusdm-theme`, which extracts it into
`~/.config/opusdm/themes/<name>/` (name taken from the archive's own
`manifest.json`), and have it show up as a selectable theme, applying its
icons, colours, appearance/behavior switches, desktop grid metrics, and
background in one shot - the same way picking `BreezeDark` works today, but
data-driven instead of compiled in.

## Non-goals

- Live-reloading a theme while applied (editing files in a theme dir after
  selecting it does not affect the running desktop, except icons - see
  "Apply is an import" below).
- Theme marketplace / download / install UI. This spec only covers loading
  themes already present on disk.
- Per-window or per-Lister theme overrides. One theme applies desktop-wide,
  same as today.
- Slideshow/rotating backgrounds. A theme's `backgrounds/` folder may hold
  more than one image; `theme.json` names exactly one as active.
- Changing `DesktopSettings::desktop_dir` (an environment/user fact, not a
  look-and-feel setting) - themes never touch it.

## On-disk theme format

```
~/.config/opusdm/themes/<slug>/
  manifest.json   # identity only: label (required), author, version (optional)
  theme.json      # data: colors, appearance, behavior, desktop metrics, background
  preview.png     # shown in opusdm-theme's picker list (optional; absent = no thumbnail)
  icons/          # 0-12 PNGs: dir.png file.png symlink.png broken.png exec.png
                  # archive.png image.png audio.png video.png code.png text.png pdf.png
  backgrounds/    # 0+ images the theme ships, referenced by filename from theme.json
```

`<slug>` (the directory name, equal to `manifest.json`'s `name`) is the
theme's stable id, used the same way `"none"` / `"breeze-dark"` are used
today. Users never create this directory by hand in the normal flow - see
"Theme import" below; a hand-placed directory following this exact layout is
still discovered and works, but is not the supported install path.

### `manifest.json`

```json
{
  "name": "my-custom-theme",
  "label": "My Custom Theme",
  "author": "optional",
  "version": "optional"
}
```

Scan-only metadata. `name` is the stable slug (also the installed directory
name) - required, non-empty, `[a-z0-9-]+` only (no `.`, `/`, or path
separators). `label` is the display name shown in the picker - required,
non-empty. `name` and `label` are independent: renaming/retranslating
`label` never changes `name`, so an already-applied `icon_set: "custom:
<name>"` in `settings.json` stays valid across theme updates. A dir whose
`manifest.json` is missing, unparseable, or fails either validation is
skipped during discovery (logged, not fatal) - this can only happen for a
theme placed by hand, since the importer (below) validates `name` before
ever writing the directory.

### `theme.json`

All fields optional (`#[serde(default)]`, same pattern as `Settings`), so a
theme can supply only icons and nothing else:

```json
{
  "colors": { ...same shape as ColorSettings... },
  "appearance": { ...same shape as Appearance... },
  "behavior": { ...same shape as Behavior... },
  "desktop": { "icon_size": 64, "grid_spacing": 96, "grid_gap": 4 },
  "background": {
    "mode": "image",
    "solid": "#2e3436",
    "gradient_start": "#2e3436",
    "gradient_end": "#555753",
    "gradient_angle": 180,
    "image": "wallpaper.jpg",
    "image_fit": "fill"
  }
}
```

`desktop` is a new small struct (`ThemeDesktopMetrics { icon_size,
grid_spacing, grid_gap }`, all `Option<i32>`) - a subset of
`DesktopSettings`, deliberately excluding `desktop_dir`.

`background.image` is a **filename relative to `backgrounds/`** inside the
theme dir, not an absolute path - this is what makes a theme portable/
shareable. It is resolved to an absolute path only at apply time (see
below). Everything else in this `background` shape matches
`opusdm_ipc::desktop::Background` field-for-field.

## Code changes

### `opusdm-ipc`

- `IconSetId` gains a third variant: `Custom(String)` (the slug).
  - `id()`: `IconSetId::Custom(slug)` -> `format!("custom:{slug}")`.
  - `from_id()`: parses the `"custom:"` prefix back into `Custom(slug)`
    unconditionally (no validation that the dir still exists - a stale
    reference just fails to find icons/theme.json later, degrading to
    glyphs, same as a deleted `BreezeDark`-style asset would).
  - `ALL`/`label()` stay as they are for the two built-ins; `Custom` is not
    part of `ALL` since it is not a fixed compile-time list. `opusdm-theme`'s
    picker builds its full list as `IconSetId::ALL` plus the live scan
    (below).
- `Theme` struct is unchanged in shape (`icon_set`, `colors`, `background`).

### `opusdm-iconview` (icon byte lookup - used by Lister, hub desktop grid, `IconField`)

- `icon_set.rs`: `icon_bytes_for` return type changes from
  `Option<&'static [u8]>` to `Option<Cow<'static, [u8]>>`.
  - `None` -> `None` (unchanged).
  - `BreezeDark` -> `Some(Cow::Borrowed(...))` (unchanged bytes, wrapped).
  - `Custom(slug)` -> on-demand, memoized load: a
    `OnceLock<Mutex<HashMap<String, CustomIcons>>>` process-local cache.
    Cache miss reads `~/.config/opusdm/themes/<slug>/icons/*.png` that
    exist (missing files simply absent from the map). Returns
    `Some(Cow::Owned(bytes.clone()))` per present icon, `None` for an
    absent one (caller falls back to the glyph, exactly like `IconSetId::
    None` does for every icon today).
  - This module does not parse `theme.json` or `manifest.json` at all -
    icons only. Every caller of `icon_bytes_for` already treats `None` as
    "fall back to `glyphs::glyph_for`", so no call site changes needed
    beyond the `Cow` type (a couple of `.as_ref()` / dereferences).

### New: theme discovery (small module, likely `opusdm-ipc::themes` or a new
`opusdm-theme-data` crate - implementation plan decides; logically it is
IPC-adjacent shared data, not GTK, so it should NOT live in `opusdm-theme`
which is a GTK binary)

- `themes_dir() -> PathBuf` - `~/.config/opusdm/themes`, same
  `dirs::config_dir()` resolution already duplicated in `hub/settings.rs`,
  `desktop-config/model.rs`, `theme-config/model.rs`.
- `scan_themes() -> Vec<CustomThemeSummary>` - lists subdirectories, parses
  each `manifest.json`, skips invalid ones, returns `{ slug, label,
  preview_path: Option<PathBuf> }` sorted by label. Used only by
  `opusdm-theme`'s picker UI.
- `load_theme_data(slug: &str) -> Option<CustomThemeData>` - parses
  `theme.json` for one slug into `{ colors: Option<ColorSettings>,
  appearance: Option<Appearance>, behavior: Option<Behavior>, desktop:
  Option<ThemeDesktopMetrics>, background: Option<ThemeBackground> }`, where
  `ThemeBackground` mirrors `Background` but with `image: Option<String>`
  (relative filename) instead of `image_path: Option<PathBuf>`. Used only by
  `opusdm-theme` at apply time.

### `opusdm-theme` (the GTK app)

- `IconSetSection` picker: today iterates `IconSetId::ALL`. Extend to also
  list `scan_themes()` results, each rendered as `Custom(slug)` with its
  `label` and (if present) `preview.png` thumbnail next to the row.
- **Apply flow** (`model.rs` or wherever `SetTheme`/`SetDesktopSettings` are
  currently sent) changes for a `Custom(slug)` selection only:
  1. `load_theme_data(slug)`.
  2. Build the `Theme` to send: `icon_set = Custom(slug)`, `colors =
     data.colors.unwrap_or_default()` (or: unwrap_or(current colors) - see
     open question below), `background` built from `data.background` with
     `image` resolved to `themes_dir().join(slug).join("backgrounds").join(image)`
     when `mode == Image` and a filename was given.
  3. Merge `data.appearance`/`data.behavior` into the current `Settings` the
     same way, and `data.desktop` fields into the current `DesktopSettings`
     (only overwriting fields the theme actually set).
  4. Send `SetTheme` (existing) and, if any desktop metric changed,
     `SetDesktopSettings` (existing) - two IPC messages, both already
     idempotent/atomic individually; no new IPC message type needed.
  5. Same `SentToHub` / `WroteFile` / `Failed` fallback as today when no hub
     is running (falls back to editing `settings.json` / `desktop.json`
     directly, same as the existing `theme-config` and `desktop-config`
     `model.rs::apply` functions already do).
- Selecting `None`/`BreezeDark` keeps working exactly as today (no theme dir
  involved).

## Theme import (archive)

`opusdm-theme` gains an "Import theme..." action (file chooser filtered to
`.zip`, `.tar.gz`/`.tgz`, `.tar.bz2`). New dependencies: `zip` for zip
archives, `tar` + `flate2` (gzip) + `bzip2` for the tarballs. Format is
picked from the file extension.

Import flow, all-or-nothing:

1. Extract the archive into a fresh `tempfile::tempdir()`, never directly
   into `themes/`. While extracting, reject any entry whose name contains a
   `..` component or is an absolute path (zip-slip / tar-slip guard) -
   abort the whole import on the first such entry.
2. Require `manifest.json` at the extracted root (flat layout - the
   archive's top-level entries are the theme's contents, not wrapped in one
   more folder). Missing -> abort with "not a theme archive".
3. Parse and validate `manifest.json` (`name` non-empty and
   `[a-z0-9-]+`-only, `label` non-empty). Invalid -> abort with the specific
   validation error.
4. On success: if `~/.config/opusdm/themes/<name>/` already exists, remove
   it first (overwrite/reinstall semantics - re-importing the same theme's
   archive, e.g. after an update, replaces it). Move the temp dir's contents
   into `~/.config/opusdm/themes/<name>/`.
5. On any failure at steps 1-3: delete the temp dir, show the error, and
   leave `themes/` untouched - no partial theme is ever installed.

This is the only supported way a theme reaches `themes/`; manual placement
(dragging a folder in) still works because discovery only cares about the
final on-disk layout, not how it got there.

## Apply is an import, not a live link

Once applied, the running desktop's `settings.json`/`desktop.json` hold a
plain `Custom(slug)` `icon_set` plus concrete copied-in colour/appearance/
behavior/metric values - not a reference to "theme X's settings". Editing or
deleting the theme directory afterwards:
- **Icons**: re-resolved from disk on next lookup (per-process cache), so
  editing `icons/*.png` is picked up after restarting the Lister/hub;
  deleting the theme dir makes those icons silently fall back to glyphs.
- **Colours/appearance/behavior/metrics/background**: already copied into
  `settings.json`/`desktop.json` at apply time, so these are unaffected by
  later changes to the theme dir. Re-applying the theme re-imports current
  values.

This keeps the hot render path (icon lookup) as the only thing that reads
the theme dir live, and keeps `Settings`/`DesktopSettings` self-contained on
disk, matching how `BreezeDark` already behaves (compiled-in, no external
dependency once chosen).

## Error handling

- Missing/unreadable `~/.config/opusdm/themes/` - `scan_themes()` returns
  empty, no error surfaced (directory may simply not exist yet).
- A theme subdir with a bad/missing `manifest.json` - skipped during scan,
  `eprintln!` a warning, never crashes discovery.
- A theme subdir with a bad/missing `theme.json` - `load_theme_data`
  returns defaults for every field (same `#[serde(default)]` tolerance
  `Settings` already has for a hand-edited `settings.json`).
- A referenced `backgrounds/<file>` that does not exist - `Background`
  ends up with an `image_path` pointing at a nonexistent file; existing
  `background_css`/rendering already treats an unreadable image path as
  "fall back to solid colour" (see `image_mode_without_a_path_falls_back_to_solid`-style handling) - no new fallback logic needed here, just confirm the existing image-path-unreadable path is exercised, not only image-path-`None`.
- A `Custom(slug)` `icon_set` whose theme dir no longer exists - every icon
  lookup returns `None`, i.e. every entry falls back to its glyph. No crash,
  no error dialog; this is the same degrade path a partially-populated
  `icons/` folder already takes per-icon.
- Theme import failures (corrupt archive, missing/invalid `manifest.json`,
  a path-traversal entry) - reported to the user as a dialog with the
  specific reason; `themes/` is left exactly as it was (see "Theme import"
  above). An unsupported extension is rejected before any extraction is
  attempted.

## Testing

- `opusdm-ipc`: `IconSetId::id()`/`from_id()` round-trip for `Custom("foo")`
  including the `"custom:"` prefix; unaffected existing tests
  (`icon_set_ids_are_unique_and_round_trip` etc.) keep passing unmodified
  since `ALL` is unchanged.
- `opusdm-iconview`: extend `icon_set.rs` tests with a `Custom` case using a
  `tempfile::tempdir()` theme dir - present icon returns bytes, absent icon
  returns `None`, nonexistent theme dir returns `None` for everything.
- New discovery module: unit tests for `scan_themes()` (valid dir, missing
  manifest, empty label, several themes sorted by label) and
  `load_theme_data()` (full theme.json, partial theme.json, missing file ->
  all-defaults), all using `tempfile::tempdir()` + `XDG_CONFIG_HOME`
  override, following the existing pattern in
  `desktop-config/model.rs::apply_writes_the_file_when_no_hub_is_running`.
- `opusdm-theme`: at minimum, a test that building the `Theme`+
  `DesktopSettings` to send from a `CustomThemeData` merges correctly
  against a starting `Settings`/`DesktopSettings` (theme-supplied fields
  win, absent fields keep the current value) - this merge function should
  be a plain data function, testable without GTK.
- Theme importer: unit tests (plain data/fs functions, no GTK) covering:
  valid zip and both tarball formats import correctly; a zip-slip entry
  (`../../evil`) aborts and leaves `themes/` untouched; missing
  `manifest.json` aborts; invalid `name` (empty, uppercase, contains `/`)
  aborts; importing over an existing `<name>` dir replaces it; an
  unsupported extension is rejected before touching the filesystem. All via
  `tempfile::tempdir()` + `XDG_CONFIG_HOME` override, building tiny archives
  in-memory with the `zip`/`tar` crates' writer APIs.

## Open questions for the implementation plan

1. **New crate or module?** Discovery/parsing code (`scan_themes`/
   `load_theme_data`) needs to be reachable from `opusdm-theme` only (not
   `opusdm-iconview`, which stays icon-bytes-only, and not the hub, which
   never reads theme dirs) - a module inside `opusdm-ipc` fits (it already
   owns `Theme`/`IconSetId`/`DesktopSettings`, `serde_json` already a
   dependency): `opusdm-ipc::themes`. The **importer** (archive extraction,
   pulling in `zip`/`tar`/`flate2`/`bzip2`) should NOT live in `opusdm-ipc`
   though - those are heavier, GTK-app-only dependencies every IPC consumer
   (hub, every Lister process) would otherwise link for no reason. Recommend
   the importer as its own module inside `opusdm-theme` (the only binary
   that needs it), reusing `opusdm-ipc::themes`' manifest-parsing types.
   Confirm crate boundary during planning.
2. **Colours/appearance/behavior merge default** - when a theme's
   `theme.json` omits `colors` entirely, should applying it *reset* colours
   to `ColorSettings::default()`, or *keep the user's current* colours
   untouched? Spec above assumes "keep current, only overwrite fields the
   theme explicitly sets" (partial import), consistent with "allow partial,
   missing icons fall back" already agreed for icons. Implementation plan
   should confirm this reading is what gets built for colors/appearance/
   behavior/desktop metrics too, since it's the same partial-override
   principle applied to four more structs.
