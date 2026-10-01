# Full themes (F-35) - design

Date: 2026-10-01. Roadmap item: F-35 in `TODO.md` / `FEATURES.md`.

## Goal

Extend the existing custom theme system so that a theme is a complete,
self-contained, shareable package: colours, fonts, background pictures for
the desktop, the Lister and the requesters, icons, and a container for
sounds. Users can build a theme from their current settings (Save), pack it
(Export) and install it (Import, already existing).

Acceptance (from FEATURES.md): export the current settings as a theme zip,
wipe the config, import it, colours and background are restored.

## Decisions taken with the user

- **Sounds**: container only. `sounds/` is copied by Save, Export and
  Import but nothing plays it until F-36. No `sounds` key is written to
  `theme.json` (the folder is copied as it is; an unknown `sounds` key in an
  existing `theme.json` is ignored).
- **Fonts**: family and size for the file list and for the UI, plus optional
  `.ttf` / `.otf` files shipped in the theme, registered at runtime for the
  OpusDM processes only (never installed system-wide).
- **Backgrounds**: one image for the Lister file list, one for the
  requesters / dialogs, each with a fit mode. The desktop background already
  exists and is reused.
- **Theme Editor**: a dedicated window that edits an installed theme's
  folder (colours, appearance, fonts, pictures, icons, sounds, manifest).
  Preview is "Save & Apply" to the live desktop, no mock preview.
- **Format**: no new format. The existing theme folder grows; it stays a
  partial override merged over the user's current settings.

## Theme layout

```
~/.config/opusdm/themes/<slug>/
  manifest.json     name (slug), label, author, version
  theme.json        values and file names only, never absolute paths
  preview.png       optional
  icons/            existing
  backgrounds/      existing, now also lister and requester images
  fonts/            new: .ttf / .otf
  sounds/           new: opaque container (carried, never played)
```

An exported theme is a `.zip` of that folder named `<slug>.zip`.

## 1. Data model (`opusdm-ipc`)

New optional `theme.json` sections (every field optional, old themes keep
loading):

```json
"fonts":    { "list": {"family": "...", "size": 11},
              "ui":   {"family": "...", "size": 10} },
"surfaces": { "lister":    {"image": "lister.png",    "fit": "tile"},
              "requester": {"image": "requester.png", "fit": "center"} },
```

- `CustomThemeData` gains `fonts` and `surfaces`.
- `Settings` gains `fonts` and `surfaces` with `#[serde(default)]`, so an old
  `settings.json` still parses (covered by a test, as for earlier fields).
- Image file names are relative to the theme's `backgrounds/` and are
  resolved to absolute paths only when applying (same rule as
  `ThemeBackground`).
- Applying a theme overwrites a settings field only where the theme sets it
  (existing `merge_custom_theme` behaviour, extended to the new sections).
  The merge is per section: a `fonts` value replaces the whole `Fonts` value,
  so a theme that sets only `list` resets `ui` to the default, like colours.
  `surfaces` merge per surface (Lister, requester).

## 2. Save, Export, Import (`opusdm-theme-config`)

UI: `opusdm-settings` gets "Save theme..." and "Export..." next to the
existing "Import theme...".

**Save theme** (capture the current settings):
1. Dialog asks for a name (label and slug, checked with `valid_slug`). There
   is no author field: the author is set in the editor's General tab. An
   existing slug asks for overwrite confirmation.
2. Pure function `build_theme_dir(settings, desktop, dest)` builds the theme
   in a temporary directory:
   - writes `manifest.json` and `theme.json` from the current settings;
   - copies every referenced image (desktop, lister, requester) into
     `backgrounds/` and rewrites the references as relative file names;
   - copies the font files the active theme uses into `fonts/`;
   - copies the active icon set into `icons/` so the theme is standalone.
3. On success the temporary directory is swapped into `themes/<slug>/`
   (`writer::write_theme`, shared with the Theme Editor). An existing theme
   is first renamed to `.<slug>.old` and restored if the second rename
   fails, so a failed swap keeps the old theme. On any failure nothing is
   left in `themes/` and the message names the failing file. The new theme shows up in the picker immediately.

**Export (BuildTheme)**: pick an installed theme, write `<slug>.zip` to a
chosen location. The packing code is the logic of `xtask package-themes`,
extracted into the `opusdm-themepack` crate, shared with `xtask`, so there
is a single packager.

**Import**: `import_theme_archive` keeps its behaviour (all-or-nothing,
zip-slip guard, overwrite on reimport). It additionally allows only
`.ttf` / `.otf` under `fonts/` and the known image types under
`backgrounds/`; other files there are skipped with a log line and do not fail
the import. `sounds/` is copied untouched.

## 3. Applying fonts and surfaces

Hook points already in the code: the Lister regenerates its CSS with
`generate_css(settings)` (`crates/lister/src/theme.rs`) into its
`css_provider`, and generic widgets are styled by `opusdm_ui::chrome_css`.

**Fonts**
- New `fonts` module in `opusdm-ui`: `register_theme_fonts(slug)` registers
  each file in `themes/<slug>/fonts/` with fontconfig
  (`FcConfigAppFontAddFile`) for the current process only. A registry
  remembers accepted and rejected files, so a file is added once and a bad
  one is logged once. The Lister (when it receives settings) and the
  Settings window (at startup, for the active theme) call it; the Hub renders
  no themed text and does not. The Theme Editor's Fonts tab only edits the
  family, size and files: it does not register fonts itself, so a font added
  there reaches the Lister when the theme is applied and the Settings window
  at its next start.
- `generate_css` emits `font-family` / `font-size` for the file list from
  `settings.fonts.list`; `chrome_css` does the same for the UI from
  `settings.fonts.ui`. Empty fields emit nothing (system default).
- `monospace_list` stays; `fonts.list` wins when set.
- An unknown family falls back through GTK, not an error. An unreadable font
  file is logged and skipped.

**Surfaces**
- `surfaces.lister.image` becomes a rule on `columnview > listview` with
  `background-image: url(...)`. `surfaces.requester.image` becomes a rule on
  requester windows. Requesters need a shared CSS class; the old
  `requester` class was removed as dead, so it is reintroduced at the single
  construction point of requesters (to be located during implementation).
- Fit mapping reuses the desktop `ImageFit` enum, so the fits are the four
  existing ones (`fill`, `fit`, `center`, `tile`; there is no `stretch`):
  `tile` = `background-repeat: repeat`; `center` = `no-repeat` plus
  `background-position: center`; `fill` / `fit` = `background-size: cover` /
  `contain`.
- With a Lister picture the file rows are drawn at 60 percent opacity
  (`ROW_ALPHA_WITH_PICTURE`) so the picture shows through.
- `file_bg` stays underneath as a fallback. A missing or unreadable image is
  logged and ignored.

**Unchanged**: desktop background (`DesktopSettings`), the IPC protocol (no
new message; `Settings` just carries two more defaulted fields).

## 4. Theme Editor (`opusdm-theme-config`, new `editor/` module)

A separate window opened by an "Edit theme..." button in `opusdm-settings`.
It edits the folder of an installed theme, not the desktop settings. The
Settings window itself has no font or picture widgets: the editor is where
they are edited.

**Layout**: a list of installed themes on the left (New, Duplicate, Delete),
tabs on the right:
- **General**: label, author, version, `preview.png`.
- **Colours**: the existing colour widgets and preset picker.
- **Appearance** (in the same tab as Colours): monospace list, type
  indicators, `theme_window`, dark / light. Behaviour and desktop metrics are
  not edited; they are carried through unchanged.
- **Fonts**: family and size for the list and the UI, plus the files in
  `fonts/` with Add and Remove.
- **Pictures**: desktop, Lister and requester picture each with a file
  chooser, fit and thumbnail.
- **Icons**: the theme's icons with "Replace..." per icon.
- **Sounds**: the files in `sounds/` with Add and Remove (container only).

**Partial override**: every section has a "Set by this theme" checkbox. When
off, that field is not written to `theme.json`, so applying the theme leaves
the user's own value (the format already supports partial themes).

**Data flow**:
- Open loads a `ThemeDraft`: manifest, `CustomThemeData` and a list of pending
  file operations (add / remove / replace for fonts, pictures, icons, sounds).
- Widgets read and write the draft only. Nothing on disk changes until Save.
- **Save** builds a staging copy of the theme folder, applies the pending file
  operations, writes `manifest.json` and `theme.json`, then swaps it in (same
  staging-and-swap as Save theme, shared code; the failure and
  save-over-self rules of section 2 apply unchanged).
- **Revert** reloads the draft from disk. Closing with unsaved changes asks
  first.
- **Save & Apply** saves, then applies the theme to the running desktop
  through the existing `apply_full` path. This is the preview.
- The default `breeze-dark` theme is editable; the editor offers "Save as
  copy" for it. It is never rewritten behind the user's back
  (`ensure_default_theme` only writes when the manifest is missing).
- Validation before Save: label not empty, slug valid for New / Duplicate,
  every referenced picture exists, font and icon replacements have the
  allowed extensions (same rules as import).

## Testing

- Unit: `generate_css` and `chrome_css` with and without `fonts` / `surfaces`,
  one test per fit mode; `theme.json` parsing for old and new shapes; path
  resolution; old `settings.json` still loads.
- Unit: `build_theme_dir` (relative references, copied assets, failure leaves
  `themes/` untouched).
- Round trip with a temporary `XDG_CONFIG_HOME`: set colours and a
  background image, Save, Export, wipe the config, Import, apply, compare
  colours and background (image included) with the original.
- Unit: `ThemeDraft` load and save round trip, "Set by this theme" off
  leaves the field out of `theme.json`, pending file operations
  (add / remove / replace) end up in the saved folder, a failed Save leaves
  the original theme untouched, Duplicate and New create valid themes.
- Manual with `run-x11.sh`: real rendering of the Lister and requester
  backgrounds and of a shipped font. The GTK side cannot be claimed verified
  without this.

## Out of scope

A mock preview inside the editor, sound playback and the sound-event map (F-18, F-36), random pick of a
background from a pattern, converting foreign theme formats
(`ConvertTheme`), per-widget font overrides beyond list and UI.

## Files touched

`crates/ipc` (themes, proto, settings), `crates/ui` (fonts, chrome_css),
`crates/lister` (theme.rs), `crates/theme-config` (apply, UI, importer, new `editor/` and `draft.rs`),
`xtask` (shared packager), `CHANGES.md`, `TODO.md`, `FEATURES.md`.
