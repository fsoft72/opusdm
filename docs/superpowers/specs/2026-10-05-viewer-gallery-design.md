# Image and video viewer (`opusdm-viewer`)

## Goal

`opusdm-viewer <file>` opens one picture or video in a GTK4 window and lets the
user move through the other pictures and videos of the same folder (the
gallery). Pictures can be zoomed, panned and rotated; videos play with the same
keys that move through the gallery. The Lister opens image files with it.

Target: X11 (Wayland is out of scope, see `docs/WAYLAND-PORT.md`).

## Scope of the first version

In: a single file argument, a gallery built from the folder of that file,
zoom (keys and mouse wheel), pan by dragging, fit / 100%, rotation, next /
previous, full screen, an info bar, a thumbnail strip, an automatic slideshow,
video playback (play / pause, seek, position slider), delete with confirmation.

Out (YAGNI): animated GIFs (the first frame is shown), prefetch of the next
picture, automatic EXIF orientation, a setting for the slideshow interval,
mark-for-deletion (that stays in Quick Look, F-17), audio files (`opusdm-player`
plays them), watching the folder for changes, zoom around the pointer, video
frames in the thumbnail strip, a volume control (system volume).

## Crate layout

New crate `crates/viewer`, binary `opusdm-viewer`. It depends on `opusdm-media`
(the `Engine`, used for videos), `opusdm-ui` (theme, `requester`), `opusdm-ipc`
(settings, logging), `gtk4` and `async-channel`.

Pure modules (unit tested, no display needed):

| Module | Responsibility |
|---|---|
| `args` | command line: exactly one file, usage text on error |
| `kind` | `MediaKind::{Image, Video}` from the extension |
| `gallery` | `Gallery`: the sorted file list and the current index; `build` from a file, natural sort, `next` / `prev` with wrap, `jump_to`, `remove_current` |
| `zoom` | `Zoom`: `Fit` or a scale, step 1.25, limits 5% to 1600%, rotation in 90 degree steps, fit scale for a view size |
| `keys` | `Action` and `map(key, kind)`: the same key means different things for a picture and a video |
| `slideshow` | on / off state and when it advances |
| `delete` | `FileRemover` trait and the trash, then permanent delete, sequence |

GTK modules:

| Module | Responsibility |
|---|---|
| `loader` | decodes a picture on a worker thread into `RawImage` (plain bytes, so it crosses threads); builds the `gdk::Texture` for a rotation on the main thread |
| `image_view` | `ScrolledWindow` with a `gtk::Picture`, wheel zoom, drag pan |
| `video_view` | the `Engine` paintable and the shared control bar (`opusdm-media-ui`: play / pause, slider, time, volume, mute) |
| `strip` | the thumbnail strip, one worker thread making thumbnails |
| `window` | owns the state, wires keys, full screen, slideshow, delete |
| `main` | arguments, theme, `gtk::Application` (one process per window, like the player) |

## Gallery

`Gallery::build(file)` lists the folder of `file`, keeps the regular files whose
extension is a known picture or video (case-insensitive, hidden files left out),
always includes `file` itself, and sorts by natural order (`img2` before
`img10`, case-insensitive, ties broken by the plain name). The list is built once;
the folder is not watched. A file that disappears later is reported like any
loading error and the user moves on.

Pictures: png, jpg, jpeg, gif, webp, svg, bmp, ico, tiff (the Lister list, plus
what the installed gdk-pixbuf loaders read). Videos: the extensions of the
default *Video file* type.

## Keys

The key controller is on the window in the capture phase, so no focused widget
takes the arrows or the space bar.

| Key | Picture | Video |
|---|---|---|
| `+` `-` (also keypad), wheel | zoom | - |
| `0` / `1` | fit / 100% | - |
| `Left` / `Right` | previous / next | seek -5 s / +5 s |
| `Space` | next | play / pause |
| `Up` / `Down`, `m` | - | volume, mute |
| `n` / `p` | next / previous | next / previous |
| `r` / `R` | rotate right / left | - |
| `s` | slideshow on / off | slideshow on / off |
| `f`, double click | full screen | full screen |
| `Delete` | delete the file | delete the file |
| `Esc` | leave full screen, else close | same |
| `q` | close | close |

## Pictures

- `loader::load_raw` runs on a worker thread (`gdk_pixbuf::Pixbuf::from_file`)
  and sends back `RawImage { width, height, rowstride, has_alpha, bytes }`.
  A generation counter drops the answer of a file the user already left.
- `RawImage::texture(rotation)` builds the pixbuf on the main thread, rotates it
  with `rotate_simple` and makes the texture.
- `Fit`: the picture uses `ContentFit::ScaleDown` (shrinks to the window, never
  enlarges). Zoom: the picture gets an explicit size request and the scrolled
  window shows scroll bars. When smaller than the window it is centred.
- Zooming from `Fit` starts from the scale the fit currently has.
- Every new file starts in `Fit` with rotation 0.

## Videos

One `Engine` is created the first time a video is shown and reused
(`open` + `play`); it is stopped when the user moves to a picture. Videos start
playing at once. An `Event::Error` is shown in the stage, with the package
hint of `MediaError`, and navigation keeps working.

## Slideshow

`s` toggles it. Every time a picture is shown with the slideshow on, a one shot
timer of 4 seconds (`SLIDESHOW_SECS`) advances to the next file (wrapping). With
a video shown the slideshow waits for `Event::Eos` and then advances.

## Thumbnail strip and info bar

The strip has one button per file. Thumbnails (96 px) are requested for the
files around the current one (12 each side), made by one worker thread
(`Pixbuf::from_file_at_scale`), and shown when ready. Videos and failed files
show a text placeholder. The current file is highlighted and scrolled to the
centre. A click jumps to that file.

The info bar shows `name - 3/12 - 1920x1080 - 125%` (`Fit` while fitting;
`video` instead of the size for videos). In full screen the strip, the info bar
and the video bar are hidden.

## Delete

`Delete` asks "Move `name` to the trash?" (requester, Trash / Cancel). Trash uses
`gio::File::trash`. When the trash is not available the user is asked a second
time ("Cannot trash: ... Delete permanently?") and `remove_file` is used. A
failure of both is shown in a requester and the gallery is left alone. On success
the file leaves the gallery, the next file (or the new last one) is shown, and
the window closes when the gallery is empty.

## Errors

- Bad command line, missing file or not a regular file: message on stderr
  (`err_log!`), exit 1.
- A picture or video that cannot be loaded: the message appears in the stage
  (`Cannot load name: reason`); no modal requester, so a folder with many
  damaged files stays usable.
- No `unwrap` on I/O; every failure is logged with `err_log!("viewer", ...)`.

## Integration

- Workspace member, shipped by `scripts/build-release.sh`.
- `filetypes::default_filetypes` gains an *Image file* type (double click
  `exec:opusdm-viewer {f}`). Existing `filetypes.json` files are never touched.
- Docs: `docs/usage.md` section 3.28, `CHANGES.md`, `TODO.md`.

## Tests

Pure modules: argument parsing, extension to kind, gallery (natural order, filter,
start index, wrap, single file, removal at the start, middle and end, removing
the last item), zoom (steps, limits, fit, rotation, rotated fit), keys for
pictures and videos, slideshow rules, delete sequence with a fake remover, time
and slider maths, and a check that every extension of the default *Video file*
type is a video for the viewer. `RawImage` conversion is tested with a generated
PNG. The windows are checked by hand under Xephyr (large picture, damaged file,
mixed folder, delete, full screen, slideshow); that check is not automated.

## Shared code with the player

`timefmt` (time text, slider mapping, seek offset) lives in `opusdm-media`. The
control bar (`Controls`, `Handlers`, the cairo `icons`) lives in the new library
crate `crates/media-ui` (`opusdm-media-ui`), used by `opusdm-player` and
`opusdm-viewer`. The Loop button is shown only when the handlers have a loop
callback (`Handlers::looping` is an `Option`): the player has one, the viewer
does not.
