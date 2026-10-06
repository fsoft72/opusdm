# Input configuration (mouse, touchpad, keyboard)

Date: 2026-10-06

## Goal

A new tool, `opusdm-input-config`, to set mouse, touchpad and keyboard
options on the X11 session. Every change takes effect **immediately**
while the window is open. **Save** stores the values; closing the window
in any other way restores the values that were active when it opened.
The hub reapplies the saved values at start-up and when a device is
plugged in.

Scope decided with the user:

- Mouse: pointer speed, acceleration profile, left handed, natural scrolling.
- Touchpad: tap to click, natural scrolling, speed, disable while typing.
- Keyboard: ordered layout list with variants, layout switch key, repeat
  delay and rate.
- Persistence: the hub applies the saved file (no autostart script).
- Out of scope: double click time (not an X setting, toolkits read
  `gtk-double-click-time`), per-device settings, Wayland.

## Structure

New crate `crates/input-config` (`opusdm-input-config`), library plus binary.
The hub depends on the library.

Library modules:

- `model`: `InputSettings { mouse: MouseSettings, touchpad: TouchpadSettings, keyboard: KeyboardSettings }`, serde, with defaults. Unknown or missing fields use defaults so old files keep loading.
- `store`: load and save `~/.config/opusdm/input.json` (`_at(path)` variants for tests). A malformed file is reported, never silently replaced.
- `backend`: trait `Backend` (apply one change, read the current state) and `XBackend`, which runs `xinput`, `setxkbmap` and `xset` without a shell (argument vectors). A `FakeBackend` for tests records the commands.
- `devices`: parsers for `xinput list`, `xinput list-props` and `setxkbmap -query` / `xset q`; classify a pointer as mouse or touchpad (a touchpad has the libinput `Tapping Enabled` property).
- `layouts`: parse `/usr/share/X11/xkb/rules/evdev.lst` into layouts, variants and switch options.
- `session`: pure state machine, see below.
- `ui` (binary only): the GTK window.

## Property mapping

| Setting | Command |
|---|---|
| speed | `xinput set-prop <dev> "libinput Accel Speed" v` (-1..1) |
| profile | `"libinput Accel Profile Enabled" a, f` (adaptive, flat) |
| left handed | `"libinput Left Handed Enabled" 0/1` |
| natural scroll | `"libinput Natural Scrolling Enabled" 0/1` |
| tap to click | `"libinput Tapping Enabled" 0/1` |
| disable while typing | `"libinput Disable While Typing Enabled" 0/1` |
| layouts | `setxkbmap -layout a,b -variant x,y -option ""` then `-option <switch>` |
| repeat | `xset r rate <delay> <rate>` |

The mouse group applies to every pointer that is not a touchpad, the
touchpad group to every touchpad. A property a device does not expose is
skipped (not an error); other failures are reported.

## Live apply and revert

`Session` holds `snapshot` (what was effective when the window opened,
read from the system through `Backend::read_current`), `current` and the
saved flag.

- `change(new)`: computes the differences against `current`, applies only those through the backend, updates `current`. Sliders are debounced (about 30 ms) in the UI, not in `Session`.
- `commit()`: writes `current` through `store` and marks it saved.
- `revert()`: applies `snapshot` (only the differences from `current`). Called on window close, Esc and Cancel when not saved.
- Closing after Save does not revert.
- Known limit: if the process is killed, nothing is restored.

## Hub integration

At start-up the hub loads `input.json` (if it exists) and applies it. It
then polls the pointer device list every few seconds and reapplies when the
set of devices changes (hot plug). A missing file or a missing tool is
logged and never stops the hub.

## Errors

- A missing tool disables the controls of its group and shows the reason.
- A failing command is shown in a status bar in the window with the command and its stderr.
- Under Wayland (no X display) the window shows a message and nothing is applied.

## Launch

The binary, plus an internal command to open it and an entry in the system
menu. The exact menu and command wiring follows the existing patterns
(see how `opusdm-settings` is started) and is fixed in the plan.

## Testing

- Unit tests: all parsers (with captured sample output), model defaults and old-file loading, command generation and property mapping, `Session` (change, revert, commit, revert after commit, no-op change), layout list parsing.
- Hub: applying a saved file through a `FakeBackend`, device set change detection.
- By hand under Xephyr with openbox: live change of speed and layout, Save, close without Save restores, hub start applies the file.

## Files touched

New crate and workspace member, hub (start-up apply and poll), ipc (internal
command, if used), `build-release.sh` and `run-x11.sh` (ship the binary),
`CHANGES.md`, `docs/usage.md`.
