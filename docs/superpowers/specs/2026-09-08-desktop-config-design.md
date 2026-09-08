# Desktop configuration: `opus-desktop-config`, desktop background, per-folder `.opusdm/`

Date: 2026-09-08
Status: approved for planning

## Goal

Three related additions to OpusDM:

1. Keep every OpusDM configuration file under `~/.config/opusdm/`, and move
   the per-directory layout file into a per-folder `.opusdm/` directory so
   future folder-scoped files have a home.
2. Let the user set a desktop background: a solid colour, a two-colour
   gradient, or an image.
3. Add a standalone app `opus-desktop-config` that edits the desktop
   options: background, icon grid size and spacing, and which directory is
   shown as the desktop.

The hub's existing "Settings" dialog (colours, theme, behaviour) is left
untouched. `opus-desktop-config` covers only the desktop section.

## Non-goals

- No changes to the hub Settings dialog or to `settings.json`'s shape.
- No multi-monitor per-screen backgrounds. One background for the desktop
  window.
- No live thumbnail/preview of the background inside the config app beyond
  what the running hub already shows once "Apply" is pressed.
- Layout persistence itself (writing icon positions) is still future work
  (CHANGES.md "fase 3"); this spec only fixes *where* that file will live.

## Current state (as read)

- `crates/hub/src/settings.rs` already loads/saves
  `~/.config/opusdm/settings.json`. `Settings` (in `crates/ipc/src/proto.rs`)
  is the on-disk shape, the in-memory state, and the wire format, all in one
  struct.
- The hub owns `settings.json` and is its only writer. `server.rs` handles
  the in-process `HubCmd::UpdateSettings` (GTK -> owner loop): it saves to
  disk and broadcasts `ToLister::SettingsUpdate` to every Lister.
- The desktop is painted by the hub. `crates/hub/src/toolbar.rs`
  `install_desktop_css()` loads a `CssProvider` from a string; the desktop
  background is the hardcoded constant `DESKTOP_BG = "#2e3436"`.
- The desktop directory is resolved by
  `crates/hub/src/desktop_files.rs::desktop_dir()` from `$XDG_DESKTOP_DIR`
  (via `~/.config/user-dirs.dirs`) then `~/Desktop`. `DesktopWatch` is a
  single-directory inotify watch, rebuilt on navigation.
- The desktop icon grid is `opusdm_iconview::IconField` in `LayoutMode::Free`.
  `IconField::set_icon_size(px)` exists (default 64).
  `FreeArrange { cell, margin }` drives auto-slot pitch; `FreeArrange::new()`
  hardcodes `cell = 96.0`, `margin = 24.0`. There is no public setter for
  `cell`.
- `crates/iconview/src/scan.rs` defines `LAYOUT_FILE = ".opusdm-layout"` and
  excludes it from `read_entries`. `crates/lister/src/window.rs:741` also
  filters a row named `LAYOUT_FILE`. Layout reading/writing is not
  implemented yet.
- The hub IPC socket (`crates/ipc/src/sockpath.rs`) accepts any Unix-socket
  client; messages are the `ToHub` enum. `probe_live_hub(path)` tells a live
  hub from a stale socket file.
- Recent commit added a desktop context-menu "Desktop" submenu whose
  "Settings" item is a placeholder that only closes the menu.

## Design

### 1. Per-folder `.opusdm/` directory

`crates/iconview/src/scan.rs`:

- Replace `LAYOUT_FILE` with:
  - `pub const OPUSDM_DIR: &str = ".opusdm";`
  - `pub const LAYOUT_BASENAME: &str = "layout";`
  - `pub fn layout_path(dir: &Path) -> PathBuf` -> `dir/.opusdm/layout`.
- `read_entries` skips any entry whose name is `OPUSDM_DIR` (explicit
  `continue`, so it stays hidden even with `show_hidden = true`, matching
  the old `LAYOUT_FILE` behaviour).
- The future layout writer must `fs::create_dir_all(dir/.opusdm)` before
  writing.

`crates/lister/src/window.rs`: the row filter at line ~741 compares against
`opusdm_iconview::scan::OPUSDM_DIR` instead of `LAYOUT_FILE`.

Rationale: folder-scoped OpusDM state stays with the folder (layout follows
the directory when it is moved/copied), but collected under one dotdir
instead of loose dotfiles.

### 2. `DesktopSettings` data model

New module `crates/ipc/src/desktop.rs`, re-exported from `opusdm_ipc`
(`pub use desktop::{DesktopSettings, Background, BackgroundMode, ImageFit};`).

All structs derive `Eq` too (every field is `String` / `i32` / `u16` /
`Option<PathBuf>` / a fieldless enum), matching `Settings`. Both enums
`#[derive(Default)]` with `#[default]` on `Solid` / `Fill` so the
`#[serde(default)]` structs get a `Default`.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundMode { #[default] Solid, Gradient, Image }

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit { #[default] Fill, Fit, Center, Tile }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Background {
    pub mode: BackgroundMode,
    pub solid: String,           // hex "#rrggbb"
    pub gradient_start: String,  // hex
    pub gradient_end: String,    // hex
    pub gradient_angle: u16,     // degrees, clamped 0..=360 on load
    pub image_path: Option<PathBuf>,
    pub image_fit: ImageFit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopSettings {
    pub background: Background,
    pub icon_size: i32,          // px, clamped ICON_SIZE_RANGE on load
    pub grid_spacing: i32,       // px, clamped GRID_SPACING_RANGE on load
    pub desktop_dir: Option<PathBuf>, // None => resolve via XDG as today
}
```

Constants and clamps (mirroring `Behavior::debounce_ms`):

- `pub const ICON_SIZE_RANGE: (i32, i32) = (24, 256);`
- `pub const GRID_SPACING_RANGE: (i32, i32) = (48, 320);`
- `pub const GRADIENT_ANGLE_MAX: u16 = 360;`
- `DesktopSettings::sanitized(self) -> Self` (or accessor methods) applies
  every clamp. The hub calls this right after loading and right after
  receiving settings over IPC.

`Default` reproduces today's look exactly:

- `mode: Solid`, `solid: "#2e3436"` (current `DESKTOP_BG`),
  `gradient_start: "#2e3436"`, `gradient_end: "#555753"`, `gradient_angle: 180`,
  `image_path: None`, `image_fit: Fill`.
- `icon_size: 64`, `grid_spacing: 96`, `desktop_dir: None`.

Every field `#[serde(default)]` so a hand-written or older `desktop.json`
never fails to load; a bad file falls back to `DesktopSettings::default()`
(same policy as `settings.rs`).

Preset button values used by the config app UI (not persisted, just UI
affordances): icon size S/M/L = 48 / 64 / 96; spacing S/M/L = 72 / 96 / 140.

### 3. Config file and ownership

- New file `~/.config/opusdm/desktop.json`.
- New `crates/hub/src/desktop_settings.rs`, a near-copy of `settings.rs`:
  `config_path()` -> `~/.config/opusdm/desktop.json`, `load()` (missing or
  malformed -> `DesktopSettings::default()`, then `.sanitized()`), `save()`
  (`create_dir_all` the parent, `to_string_pretty`).
- **The hub is the only writer of `desktop.json` while it is running**,
  consistent with `settings.json`. The config app writes the file directly
  only when no hub is running.

### 4. IPC

`crates/ipc/src/proto.rs`:

- New `ToHub` variant: `SetDesktopSettings(DesktopSettings)`.
- No new `ToLister` message: Listers do not need desktop settings.

`crates/hub/src/server.rs`:

- Load `let mut current_desktop = desktop_settings::load();` alongside
  `current_settings`.
- New `HubEvent::DesktopSettingsChanged(DesktopSettings)` sent to the GTK
  side (`evt_tx`).
- Handle `OwnerMsg::FromLister(_cid, ToHub::SetDesktopSettings(ds))`:
  `current_desktop = ds.sanitized(); let _ = desktop_settings::save(&current_desktop);
   let _ = evt_tx.send(HubEvent::DesktopSettingsChanged(current_desktop.clone()));`
  (Reader connections are not Lister-specific; a non-registered connection
  may send this. No registration handshake is required for it.)

The `ToHub` reader path already deserializes arbitrary `ToHub` values from
any connection, so `opus-desktop-config` connecting like a client and
sending one frame is enough. It does not send `Register`.

### 5. Applying settings live in the hub

`crates/hub/src/toolbar.rs`:

- `install_desktop_css` becomes `install_desktop_css(bg: &Background)`. It
  builds the `.opusdm-desktop` rule from a pure helper
  `desktop_background_css(bg: &Background) -> String` and keeps the existing
  `.opusdm-desktop-label` / `.opusdm-desktop-menu` rules. Called once at
  startup with the loaded background and again on every change (replacing
  the same provider - the function already tolerates re-invocation).
- `desktop_background_css` mapping:
  - `Solid`  -> `background: {solid};`
  - `Gradient` -> `background: linear-gradient({angle}deg, {start}, {end});`
  - `Image` with a readable `image_path`:
    - common: `background-image: url("file://{percent-encoded path}");`
      `background-color: {solid};` (fallback colour showing through
      transparency / while loading)
    - `Fill`   -> `background-size: cover; background-position: center; background-repeat: no-repeat;`
    - `Fit`    -> `background-size: contain; background-position: center; background-repeat: no-repeat;`
    - `Center` -> `background-size: auto; background-position: center; background-repeat: no-repeat;`
    - `Tile`   -> `background-size: auto; background-repeat: repeat;`
  - `Image` with a missing/unreadable path -> fall back to the `Solid`
    branch on `bg.solid`.
  - Path -> URL: percent-encode everything outside an unreserved set
    (small local helper; no new dependency). Absolute paths only.
- `Widgets` gains a handle to the desktop `IconField` (a handle already
  exists for "Rearrange icons") and to `desktop_dir` (already stored).
- `apply_event` handles `DesktopSettingsChanged(ds)`:
  1. `install_desktop_css(&ds.background)`.
  2. `field.set_icon_size(ds.icon_size)` and
     `field.set_grid_spacing(ds.grid_spacing)`.
  3. Resolve the wanted directory:
     `ds.desktop_dir.filter(|p| p.is_dir()).or_else(desktop_files::desktop_dir)`.
     If it differs from the current one: drop and rebuild `DesktopWatch`,
     re-read entries with `scan::read_entries`, `field.set_entries(&..)`,
     update the stored `desktop_dir` and the drop-target dir used by the
     copy/move job wiring.
- Startup: `build_ui` loads `desktop_settings::load()`, passes
  `ds.background` to `install_desktop_css`, resolves the desktop directory
  through the same override-then-XDG chain, and applies `icon_size` /
  `grid_spacing` to the field after it is built.

`crates/iconview`:

- `FreeArrange` gains no new field (it already has `cell`). Add
  `IconField::set_grid_spacing(px: i32)`: store it in a new
  `inner.grid_spacing: Cell<i32>` (default 96), and have `rebuild_arranger`
  set `fa.cell = self.inner.grid_spacing.get() as f64` for the `Free`
  branch. Then `relayout()`. `GridArrange` (Lister) is unaffected.
- `set_icon_size` already triggers `relayout`; no change.

`crates/hub/src/toolbar.rs` context menu: the "Desktop -> Settings"
placeholder spawns `opus-desktop-config`:
`std::process::Command::new(bin).spawn()`, where `bin` is resolved next to
the current executable (`std::env::current_exe()` parent + `opus-desktop-config`)
with a fallback to the bare name on `PATH`. Failure logs to stderr and is
otherwise ignored.

### 6. `opus-desktop-config` app

New crate `crates/desktop-config`:

- `Cargo.toml`: `name = "opusdm-desktop-config"`, `[[bin]] name = "opus-desktop-config"`.
  Deps: `gtk4`, `opusdm_ipc`, `opusdm_ui`, `dirs`, `serde_json`.
- Added to `Cargo.toml` workspace `members`.

Structure (keep files small and focused):

- `main.rs`: `gtk::Application` (`org.opusdm.DesktopConfig`), builds the
  window, wires buttons.
- `model.rs`: read `desktop.json` (or `DesktopSettings::default()`); build a
  `DesktopSettings` back from the widgets; `send_or_write(ds)` -> if
  `probe_live_hub(hub_sock_path())` connect a `UnixStream`, send one framed
  `ToHub::SetDesktopSettings(ds)` (reuse `opusdm_ipc` framing), else
  `save desktop.json` directly and return a "hub not running" flag.
- `ui.rs`: the three sections.

Window sections (`opusdm_ui` helpers where they fit; a plain
`ApplicationWindow` + `Box`, not `DialogShell`, since this is a top-level
app):

- **Background**: three radio buttons (Solid / Gradient / Image) driving a
  `gtk::Stack`:
  - Solid: one `ColorDialogButton`.
  - Gradient: start `ColorDialogButton`, end `ColorDialogButton`,
    `SpinButton` 0..=360 step 1 for the angle.
  - Image: a read-only `Entry` + "Browse..." (`gtk::FileDialog.open`,
    image filter), a `DropDown` Fill/Fit/Center/Tile, and an inline
    warning `Label` (hidden unless the chosen path is missing or
    unreadable).
- **Icon grid**: two rows, each a `SpinButton` plus three small preset
  buttons (S / M / L) that set the spin value. Ranges from
  `ICON_SIZE_RANGE` / `GRID_SPACING_RANGE`.
- **Desktop folder**: read-only `Entry` showing the effective path,
  "Browse..." (`FileDialog.select_folder`), "Reset to default" (clears the
  override -> `desktop_dir = None`, entry shows the XDG-resolved path in a
  dim style).
- **Buttons row**: Cancel (close), Apply (send, keep open), Save (send,
  close). After Apply/Save with no hub running, show a transient inline
  note: "OpusDM non in esecuzione: le modifiche si applicheranno al
  prossimo avvio."

Colour widgets store/emit `#rrggbb` strings (GDK RGBA -> hex helper).

## Error handling

- Malformed `desktop.json` -> `DesktopSettings::default()` (both hub and
  app). No error dialog; matches `settings.rs`.
- All numeric fields clamped on load and after every IPC receive via
  `DesktopSettings::sanitized`.
- Background image path missing/unreadable:
  - hub CSS falls back to the solid colour;
  - app shows the inline warning and still lets the user save (the file may
    appear later).
- `desktop_dir` override that is not an existing directory: ignored by the
  hub (XDG fallback), app shows it but flags it dim/invalid.
- Hub socket absent or stale (`probe_live_hub` false): app writes
  `desktop.json` itself and reports "not running".
- IPC send failure after a positive probe: app falls back to writing the
  file and reports the same note.
- `current_exe()` failing when spawning the app from the hub: fall back to
  spawning `opus-desktop-config` from `PATH`; log and ignore on failure.

## Testing

`crates/ipc`:

- `DesktopSettings` / `Background` serde round-trip.
- Back-compat: JSON missing every optional field deserializes to defaults.
- `sanitized()` clamps icon size, grid spacing, gradient angle at both ends.

`crates/ipc` or `crates/hub` (pure helper):

- `desktop_background_css` per mode: solid, gradient (angle rendered),
  image Fill/Fit/Center/Tile (correct `background-size`/`-repeat`), image
  with `None` path -> solid fallback.
- Path -> `file://` URL percent-encoding (spaces, unicode, `#`, `%`).
- "Parse the generated CSS with the GTK CSS parser" test in the style of
  the existing theme-engine test; self-skips without a display.

`crates/iconview`:

- `scan::layout_path` shape; `read_entries` excludes `.opusdm` with and
  without `show_hidden`.
- `IconField::set_grid_spacing` changes the `Free` auto-slot pitch
  (assert item positions move to the new `cell`). `FreeArrange` with a
  custom `cell` is already covered.

`crates/hub`:

- `desktop_settings` load/save round-trip; missing file -> default;
  malformed file -> default.
- Desktop directory resolution: override wins when it is a real dir; falls
  back to XDG resolution when the override is missing or not a dir.

Manual / integration:

- Hub running: change each option in `opus-desktop-config`, press Apply,
  confirm the desktop repaints, the grid re-spaces, and a directory change
  reloads the icons and re-arms the watch.
- Hub not running: press Save, confirm `desktop.json` is written and the
  change is visible on the next hub start.
- Move a folder with a `.opusdm/` dir; confirm the Lister still hides it.

## Files touched

- `crates/ipc/src/desktop.rs` (new), `crates/ipc/src/lib.rs`,
  `crates/ipc/src/proto.rs` (`ToHub::SetDesktopSettings`).
- `crates/iconview/src/scan.rs`, `crates/iconview/src/field.rs`
  (`set_grid_spacing`), `crates/iconview/src/arrange.rs` (only if a test
  needs it).
- `crates/lister/src/window.rs` (row filter constant).
- `crates/hub/src/desktop_settings.rs` (new),
  `crates/hub/src/server.rs`, `crates/hub/src/toolbar.rs`,
  `crates/hub/src/lib.rs` (module export), `crates/hub/src/desktop_files.rs`
  (only if the override chain is factored here).
- `crates/desktop-config/` (new crate: `Cargo.toml`, `src/main.rs`,
  `src/model.rs`, `src/ui.rs`).
- `Cargo.toml` (workspace members), `Cargo.lock`.
- `CHANGES.md`.

## Open questions

None outstanding. `.opusdm/layout` location confirmed; app scope confirmed
to the desktop section only.
