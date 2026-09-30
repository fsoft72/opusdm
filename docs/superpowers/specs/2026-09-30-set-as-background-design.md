# F-37 SetAsBackground - design

Status: approved in conversation, pending written-spec review.
Part of roadmap step 6 (Desktop features: F-37, then F-33, F-34, F-38, each
with its own spec, plan and implementation cycle).

## Goal

Let the user install any picture as the desktop background from a Lister
context menu or from a function, applying it live.

## Scope

In scope:
- New internal command `SetBackground` in the function system.
- "Set as Desktop Background" entry in the Lister context menu for image files.
- Live apply and persistence through the existing desktop settings path.

Out of scope:
- Lister and requester backgrounds (F-35).
- Filetype actions (F-06). The context menu entry is hardcoded on the image
  extension until F-06 exists.
- Seeding default functions into the user's `functions.json`.

## Existing pieces reused

- `DesktopSettings.background` (`crates/ipc/src/desktop.rs`): `mode`
  (`Solid` / `Gradient` / `Image`), `image_path`, `fit`
  (`Fill` / `Fit` / `Center` / `Tile`), rendered by `background_css`.
- `ToHub::SetDesktopSettings` handling in `crates/hub/src/server.rs`:
  sanitize, persist to `desktop.json`, redraw the CSS, broadcast
  `HubEvent::DesktopSettingsChanged`.
- `ChangeDir` step handling in `crates/hub/src/functions.rs`: path template
  expansion, `~` and cwd-relative resolution, and the `FnEvent` channel that
  the server maps to messages.

## Design

### 1. Internal command (`crates/ipc/src/function.rs`)

- Add `InternalCmd::SetBackground`, label "Set Background".
- Schema parameters:
  - `path`: required, `ParamKind::PathTemplate` (accepts `{f}`).
  - `fit`: optional, `ParamKind::Text`; accepted values `fill`, `fit`,
    `center`, `tile`, case-insensitive.
- Add it to the command list of the Function Editor
  (`crates/lister/src/function_editor.rs`).

### 2. Execution (`crates/hub/src/functions.rs`, `server.rs`)

- A `SetBackground` step resolves `path` like `ChangeDir` does.
- The step fails with a descriptive error when:
  - the resolved path is not a regular file;
  - the extension is not one of png, jpg, jpeg, webp, bmp, gif
    (case-insensitive);
  - `fit` is present and not one of the four accepted values.
- On success it sends `FnEvent::SetBackground { path, fit: Option<ImageFit> }`.
- The server handler takes the current `DesktopSettings`, sets
  `background.mode = Image`, `background.image_path = path`, and
  `background.fit` only when `fit` is `Some` (otherwise the current fit is
  kept). It then runs the same code path as `SetDesktopSettings`
  (sanitize, persist, apply live, broadcast). No new persistence code.

### 3. Lister context menu (`crates/lister/src/context_menu.rs`)

- For rows that are files with an image extension, add a
  "Set as Desktop Background" button.
- Click runs an ad-hoc function with a single `SetBackground` step whose
  `path` is the row path, through the existing function-run channel. The
  exact call site is confirmed while writing the implementation plan.
- Failures surface through the normal function error reporting.

### 4. Palette

The Command Palette lists saved functions, so there is no separate palette
command. A user gets a palette entry by saving a function containing
`SetBackground {f}`. Nothing is added to `functions.json` automatically.

## Error handling

Fail fast with messages that include the offending value: for example
`Set Background: "notes.txt" is not a supported image (png, jpg, jpeg, webp,
bmp, gif)` or `Set Background: unknown fit "stretch" (use fill, fit, center
or tile)`. A failed step leaves the current background untouched.

## Testing

Unit tests, next to the existing ones in `crates/hub/src/functions.rs` and
`crates/ipc`:
- relative and `~` paths resolve, like `ChangeDir`;
- non-file path, non-image extension and invalid `fit` each produce an error;
- `fit` omitted keeps the current fit, `fit` given overrides it;
- the schema lists `path` as required and `fit` as optional, and the command
  survives a JSON roundtrip.

Manual acceptance: right-click a PNG in a Lister, choose "Set as Desktop
Background", the desktop background changes live and survives a hub restart.

## Documentation

Update `CHANGES.md`, `TODO.md` (F-37) and the F-37 section of `FEATURES.md`,
which currently mentions a filetype action that does not exist yet.
