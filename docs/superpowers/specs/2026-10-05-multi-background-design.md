# Multiple desktop backgrounds

## Goal

The desktop shows a list of backgrounds (images, videos, flat colours,
gradients) that rotate on a global timer with a cross-fade. Images and
videos can be mixed. Settings gets a "Desktop Background" tab that owns the
list; the old "Background" section leaves the "Desktop" tab.

## Decisions (agreed with the user)

- Order is sequential, with a global "Shuffle" option.
- The interval is global, in seconds. The fade is global, in milliseconds
  (default 1000).
- A video shorter than the interval loops until the interval expires. A
  longer one is cut when the interval expires. Videos are always silent.
- A single usable entry never rotates and never fades.
- An entry that cannot be shown (missing file, video that fails to play) is
  skipped with a log line. With no usable entry left the desktop paints the
  default solid colour.
- The "Add from theme..." button of the old section stays: it adds the theme
  wallpapers to the list.
- `SetBackground` (function step and Lister context menu) replaces the whole
  list with the one picture, keeping the interval / fade / shuffle values.
- Existing settings migrate on load: the old single background becomes a
  list of one entry.

## Data model (`opusdm-ipc`, `desktop.rs`)

`Background` (still `Settings.theme.background`):

| field           | type                   | default |
|-----------------|------------------------|---------|
| `entries`       | `Vec<BackgroundEntry>` | one `Solid` entry `#2e3436` |
| `interval_secs` | `u32`                  | 300, clamped to 5..=86400 |
| `fade_ms`       | `u32`                  | 1000, clamped to 0..=10000 |
| `shuffle`       | `bool`                 | false |

`BackgroundEntry` is tagged with `kind` (snake case): `solid { color }`,
`gradient { start, end, angle }`, `image { path, fit }`, `video { path, fit }`.
A video honours `fill` and `fit`; `center` and `tile` behave as `fill`.

Deserialisation accepts the legacy flat shape (`mode`, `solid`,
`gradient_*`, `image_path`, `image_fit`) and turns it into one entry.
`sanitized()` clamps the interval, the fade and every gradient angle.

`background_css(&BackgroundEntry)` keeps producing the CSS declarations
(images, gradients, colours). A video page paints black behind its picture.

New helpers: `BACKGROUND_VIDEO_EXTS`, `is_background_video`,
`is_background_media`.

## Hub

- `opusdm_hub::bg_sequence::Sequence` (pure, tested): sequential or shuffled
  order over entry indexes, skipping unusable ones, reshuffling at each wrap
  without repeating the entry that just played.
- `background.rs` (GTK): a thread-local controller. Every desktop window
  registers a `gtk::Stack` (cross-fade) with one page per entry. Colour,
  gradient and image pages are boxes styled by a per-entry CSS class in the
  single desktop stylesheet; video pages are `gtk::Picture`s fed by an
  `opusdm_media::Engine` (no audio, looping) whose paintable is shared by
  every monitor.
- A glib timer fires after `interval_secs`; the next entry is shown with the
  fade, the outgoing video engine is dropped once the fade ends. The
  effective fade never exceeds half the interval.
- A settings change identical to the current one is ignored, so unrelated
  theme changes do not restart the rotation.

## Settings (`opusdm-settings`)

New tab "Desktop Background": the list (one row per entry with a preview,
a label, a fit selector for pictures and videos, move up / down and remove),
the buttons "Add file...", "Add color", "Add gradient", "Add from theme...",
and the "Interval (s)", "Fade (ms)" and "Shuffle" controls. Colour and
gradient rows edit their colours in place.

## Tests

Unit tests for the migration, the serde shape, the clamping, the CSS per
entry and the sequence logic. A GTK CSS parse test for every entry kind
(skipped without a display).
