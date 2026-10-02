# Theme as the single source of the look - design

Date: 2026-10-02. Follows F-35 (full themes).

## Goal

Stop editing the same values in two places. Today the Settings window edits
Lister colours, appearance and the desktop background, and the Theme Editor
edits the same values inside a theme. Applying a theme merges it over the
user's values, so the two copies drift apart and "Save theme..." exists only
to capture that drift.

After this change every value has exactly one editor:

- **The theme** (edited only in the Theme Editor) owns the look.
- **The preferences** (edited only in the Settings window) own everything
  that is personal to the user or to the machine.

## Decisions taken with the user

- Icon grid metrics (`icon_size`, `grid_spacing`, `grid_gap`) are a
  preference. They stay in the Settings window and are no longer part of a
  theme.
- Light / dark (`appearance.theme_mode`) is a preference. It stays in the
  Settings window and is ignored when found in a theme.
- The desktop background (wallpaper) is a preference. It stays in the
  Settings window, and "Set as background" from the Lister keeps changing it.
  A theme can bundle wallpaper pictures; the Settings window gets a
  "From theme..." button that picks one of the active theme's pictures.
- Existing customisations are migrated automatically into a new theme called
  "My Theme" the first time the Settings window opens.

## Ownership

| Value | Owner | Edited in |
|---|---|---|
| Lister colours (`theme.colors`) | theme | Theme Editor, Colours |
| `appearance.type_indicators`, `monospace_list`, `theme_window` | theme | Theme Editor, Colours |
| Fonts (`fonts`) | theme | Theme Editor, Fonts |
| Lister / requester pictures (`surfaces`) | theme | Theme Editor, Pictures |
| Icons, sounds, wallpaper pictures | theme | Theme Editor |
| Active theme (`theme.icon_set`) | preference | Settings, Desktop |
| `appearance.theme_mode` | preference | Settings, Desktop |
| Desktop background (`theme.background`) | preference | Settings, Desktop |
| Icon grid, desktop folder, panel | preference | Settings, Desktop |
| Dock, menu | preference | Settings, Dock / Menu |
| Behaviour (`behavior`) | preference | Settings, Lister |

`Settings.theme.background` keeps its place in the struct (IPC payload and
`settings.json` stay compatible); its doc comment says it is a preference.

## Resolving a theme

`resolve_look(slug, prefs: &Settings) -> Settings` (replaces
`merge_custom_theme`) returns `prefs` with the theme-owned fields replaced by
what the theme sets, and **defaults** where the theme does not set them. It is
no longer a partial merge: the same theme always produces the same look,
whatever the user had before. `theme_mode`, `behavior` and `background` are
always copied from `prefs`. `DesktopSettings` is never touched by a theme.

`theme.json` loses its `behavior`, `desktop` and `background` sections.
Old files that carry them still parse (serde ignores unknown keys); the values
are ignored.

## Where the resolved values live

`settings.json` keeps the full `Settings`, look values included: it is the
runtime cache the hub loads at start and sends to every Lister. Only two
places write it with new look values, both through `resolve_look`:

- Apply in the Settings window (resolves the selected theme).
- Save & Apply in the Theme Editor.

The hub is unchanged. Saving the active theme in the editor without applying
leaves the cache as it was until the next apply, as today.

## Wallpapers in a theme

A theme may carry pictures in a new `wallpapers/` folder. The Theme Editor
gets a WALLPAPERS asset list (Add / Remove, like fonts and sounds) in the
Pictures tab, replacing the DESKTOP BACKGROUND section. Save, Duplicate,
Export and Import carry `wallpapers/`; Import keeps only picture files in it.

`opusdm_ipc::themes::theme_wallpapers(slug) -> Vec<PathBuf>` lists the
pictures in `wallpapers/`, sorted, plus the picture an old `theme.json`
`background.image` names in `backgrounds/` (legacy themes keep offering their
wallpaper). When the writer saves a theme from a source that has such a legacy
picture, it copies it into `wallpapers/`, so editing an old theme never loses
it.

The Settings window's BACKGROUND section gains a "From theme..." button next
to "Browse...". It opens a small dialog with thumbnails of
`theme_wallpapers(<theme selected in the picker>)`; choosing one switches the
mode to Image and sets the path (absolute, inside the theme folder). An empty
list shows a short note instead.

## Settings window

- Desktop tab: Light / dark, THEME (picker plus Edit..., Import...,
  Export...), BACKGROUND, ICON GRID, DESKTOP FOLDER, PANEL.
  "Save theme..." is removed (Duplicate in the Theme Editor covers it).
- Lister tab: BEHAVIOUR only. PRESET, COLOURS and APPEARANCE move out.
- Apply: preferences from the widgets, look from `resolve_look(selected)`.

## Theme Editor

- Colours tab: no Light / dark selector.
- Pictures tab: WALLPAPERS list, LISTER and REQUESTER pictures.
- `ThemeDraft` loses `behavior`, `desktop` and `background`.
- The default theme (`breeze-dark`) is read only: Save, Save & Apply and
  file changes are refused with "The default theme cannot be changed:
  duplicate it first". Save as copy and Duplicate still work.
- The "Set by this theme" checkboxes stay; unchecked now means "use the
  default value" instead of "keep the user's value".

## Migration

`Settings` gains `look_version: u32` (`#[serde(default)]`, so old files read
0). `CURRENT_LOOK_VERSION = 1`.

At start, before building the window, `opusdm-settings` runs
`migrate_look(settings, desktop)` when `look_version < 1`:

1. `resolved = resolve_look(active, &settings)`.
2. If the theme-owned fields of `settings` differ from `resolved`, write a new
   theme "My Theme" (slug `my-theme`, or `my-theme-2`, ... when taken) from
   the current values, carrying the active theme's icons, fonts and sounds,
   and make it the active theme.
3. Set `look_version = 1` and persist through `apply_full` (hub if running,
   files otherwise).

Until the Settings window is opened nothing changes: the hub keeps using the
cached values in `settings.json`, which are exactly the user's values.

## Out of scope

- Moving `background` out of `Settings.theme` into its own field.
- Live reload when the active theme is saved without Apply.
- Hub-side resolution of the theme at start.

## Acceptance

1. With custom colours in `settings.json`, opening the Settings window
   creates "My Theme" with those colours, selects it, and the Listers look
   unchanged.
2. The Settings window has no colour, appearance or preset controls.
3. Picking a theme and Apply sets exactly that theme's look; a theme that sets
   no colours gives the default colours.
4. Light / dark, icon grid and wallpaper survive switching theme.
5. "From theme..." lists the active theme's wallpapers and sets one.
6. Editing `breeze-dark` in the Theme Editor is refused; Duplicate works.
