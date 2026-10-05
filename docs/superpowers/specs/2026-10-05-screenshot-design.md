# Screenshot tool (`opusdm-shot`)

## Goal

A hotkey bound to `exec:opusdm-shot` freezes the screen, lets the user drag a
rectangle, opens a small editor with the cropped image (lines, arrows, text with
ready-made styles) and saves it as PNG, JPG or WebP, copies it to the clipboard
or saves it in one click.

Target: X11 (Wayland is out of scope, see `docs/WAYLAND-PORT.md`).

## Flow

hotkey -> `capture` (primary monitor of the root window to RGBA) -> `overlay` (full-screen selection)
-> crop -> `editor` -> Save dialog / Copy / Quick save.

Esc in the overlay quits with no output. A selection under 4x4 px is ignored.

## Crate layout (`crates/shot`, binary `opusdm-shot`)

| Module | Responsibility |
|---|---|
| `capture.rs` | `x11rb` `GetImage` of the primary monitor area into an RGBA image |
| `selection.rs` | Rectangle geometry: normalise, clamp, minimum size |
| `annotations.rs` | Model of lines, arrows and texts, styles, hit-test, undo/redo |
| `render.rs` | Draws the annotations on an image (preview and export share it) |
| `export.rs` | PNG / JPG / WebP encoding, quality, unique file names |
| `overlay.rs` | GTK full-screen window with the selection |
| `editor.rs` | GTK editor window: canvas, tool bar, Save, Copy, Quick save |

Everything but `overlay.rs` and `editor.rs` is GUI free and unit tested.

## Editor

- Tools: Select/Move, Line, Arrow, Text. Undo / Redo (Ctrl+Z, Ctrl+Shift+Z),
  Delete removes the selected annotation.
- Line and arrow are drawn by dragging; Shift locks the angle to 15 degrees.
- Text: click, type on the canvas, Enter adds a line, Esc or a click outside
  commits.
- Annotations stay objects until export, so they remain editable.
- Default style: red, 4 px, text 24 px. A palette of 6 colours (red, yellow,
  green, blue, black, white) and 3 widths (2, 4, 8 px; text 16, 24, 40 px).
  Changing them affects the new annotations and the selected one. No config
  file: the defaults are constants.

## Output

- PNG lossless. JPG quality 1-100 (default 90), alpha flattened on white. WebP
  quality 1-100 (default 90), lossy through libwebp, with a lossless switch.
- Save dialog: `gtk::FileDialog` with a format filter; the extension picks the
  format. The format, quality slider (JPG and WebP) and lossless switch (WebP)
  live in the bottom bar of the editor, since a file dialog cannot hold them; a
  missing or unknown extension gets the one of the bar format appended.
- Quick save: `~/Pictures/Screenshots/shot-YYYYMMDD-HHMMSS.png`; the folder is
  created, an existing name gets a numeric suffix, never overwritten.
- Copy: the rendered image goes to the clipboard as a texture. The window stays
  open: without a clipboard manager the content would vanish with the process.
- After a successful save the window closes; after an error it stays open with
  the error (path and cause) in a bar.

## Errors

Fail fast with context, never silently: capture failure prints `[shot]` plus the
cause and exits non zero; write and encode failures are shown in the editor and
the image is kept.

## Tests

Unit tests for `selection`, `annotations`, `render` (known pixels) and `export`
(round trip of the three formats, unique names, alpha on JPG). `overlay` and
`editor` are checked by hand under Xephyr.

## Integration

Binary added to `scripts/build-release.sh`; usage section in `docs/usage.md`;
entry in `CHANGES.md`.

## Out of scope

Wayland, other monitors than the primary one, GIF and video, blur and pixelate, rectangles and ellipses, upload,
single window capture, config file for the styles.
