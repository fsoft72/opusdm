# Media player (`opusdm-player`)

## Goal

`opusdm-player` plays one audio or video file in a small GTK4 window: play /
pause, seek, volume, mute, full screen. The Lister opens audio and video files
with it.

The playback engine lives in its own library crate (`crates/media`), with no UI
and no dependency on the player window, so a later feature can reuse it to play a
video as the desktop background instead of the picture (see "Reuse for the
desktop background").

Target: X11 (Wayland is out of scope, see `docs/WAYLAND-PORT.md`).

## Scope of the first version

In: a single file (command line, Lister double click or Open With), play /
pause, seek slider, current and total time, volume, mute, full screen, loop
switch, cover art and tags for audio only files, errors in a requester.

Out (YAGNI): playlist, subtitles, audio track selection, playback speed,
equalizer, being the desktop background.

## Backend

GStreamer through `gstreamer-rs`, with a `playbin3` pipeline and the
`gtk4paintablesink` video sink (Ubuntu package `gstreamer1.0-gtk4`). The sink
gives a `gdk::Paintable`, shown by a `gtk::Picture`.

Build time: `libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`. Run time:
`gstreamer1.0-gtk4`, `gstreamer1.0-plugins-good`, `gstreamer1.0-plugins-bad`,
`gstreamer1.0-libav`.

## Crate layout

### `crates/media` (library `opusdm-media`, GStreamer only)

| Module | Responsibility |
|---|---|
| `engine.rs` | `Engine`: owns the `playbin3` pipeline. `new(EngineOptions)`, `open(uri)`, `play`, `pause`, `stop`, `seek(ns)`, `set_volume(f64)`, `set_muted(bool)`, `set_loop(bool)`, `position()`, `duration()`, `paintable()` |
| `event.rs` | `Event` enum sent to the UI: `State(PlayState)`, `Position(ns)`, `Duration(ns)`, `Tags(TagInfo)`, `Eos`, `Error(MediaError)` |
| `error.rs` | `MediaError`: `MissingPlugin(name)` (names the package to install), `Unsupported(uri)`, `Pipeline { message, debug }` |
| `tags.rs` | `TagInfo` (title, artist, album, cover as encoded bytes), built from a `gst::TagList` |
| `uri.rs` | Path or `file://` / `http(s)://` argument to a URI |

`EngineOptions { audio: bool, looping: bool, video_sink: VideoSink }`. `audio:
false` plays video only (no audio decode); both are for the desktop background
and the player uses the defaults (`audio: true`, `looping` from `--loop`).
`VideoSink::Discard` throws frames and audio away, so the tests run with no
display and no sound card; `paintable()` is then `None`.

`Engine::open` fails only for an invalid path or URL. A file that cannot be
opened or played arrives as `Event::Error` (one channel for those).

Rules:

- `Engine` lives on the GTK main thread. The bus is read with
  `bus.add_watch_local` and translated to `Event` values handed to a callback.
  No thread of its own and no `Mutex` in the public API.
- `Event::Position` comes from a 250 ms GLib timer that runs only while playing.
- A missing `gtk4paintablesink` makes `Engine::new` fail at once with a message
  that names `gstreamer1.0-gtk4`. No silent fallback.
- At the end of the stream the engine seeks to 0 and pauses, or keeps playing
  when `looping` is set.

### `crates/player` (binary `opusdm-player`)

| Module | Responsibility |
|---|---|
| `main.rs` | Arguments, `gtk::Application`, theme, one window per file |
| `args.rs` | Parses the command line (one file, optional `--loop`) |
| `view.rs` | Picks the video view or the audio view from the stream (`ViewKind`) |
| `timefmt.rs` | `mm:ss` / `h:mm:ss`, slider position to nanoseconds and back |
| `window.rs` | GTK window: video area, controls, full screen, Space / Left / Right / F / M keys |
| `controls.rs` | Play / pause button, seek slider, times, volume, mute, loop |
| `icons.rs` | Play, pause, volume, muted and loop icons drawn with cairo (they follow the theme text colour, like the `opusdm-shot` ones) |
| `audio_view.rs` | Cover or icon, title, artist, album for audio only files |

Everything but `window.rs`, `controls.rs` and `audio_view.rs` is GUI free and
unit tested.

## Seek

While dragging the slider the player seeks with `FLUSH | KEY_UNIT`; on release
it makes one accurate seek. `Event::Position` does not move the slider while it
is dragged.

## Errors

- `Engine::open` returns `Result<(), MediaError>`.
- The player shows an error in an OpusDM requester and logs it to the console
  with the `[player]` prefix. Only the first error is shown, because GStreamer
  follows it with a second, more technical one. A file that cannot be played exits
  with code 1.

## Integration

- File types: OpusDM has no built-in File Types entries (the user creates them),
  so `docs/usage.md` explains how to make audio and video files open with
  `opusdm-player`.
- `scripts/build-release.sh` ships the binary; `opusdm-session` and `run-x11.sh`
  put the binaries on `PATH`.
- Usage documented in `docs/usage.md`.

## Tests

- Unit: time formatting, slider mapping, view choice, argument parsing, URI
  building, tag list conversion.
- Integration (`crates/media/tests`): `videotestsrc` / `audiotestsrc` through a
  `gst::parse::launch` source: open, play, pause, seek, EOS, loop, volume. They
  skip with a clear message when GStreamer is not installed.
- By hand under Xephyr: a video, an audio file with a cover, full screen, a
  corrupt file.

## Reuse for the desktop background (later, not part of this work)

A desktop video background will build an `Engine` with
`EngineOptions { audio: false, looping: true }` and put `engine.paintable()` in a
`gtk::Picture` inside the desktop window, where the image is now drawn from
`Background` (`crates/ipc/src/desktop.rs`). Nothing in `crates/media` knows about
the player, so no change to it is expected beyond the settings plumbing.
