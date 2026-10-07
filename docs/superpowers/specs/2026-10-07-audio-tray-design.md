# Audio tray (volume and devices in the panel)

Date: 2026-10-07

## Goal

Something like the KDE "Audio Volume" applet. A panel icon opens a popover
to control the volume and mute of the output and the input, and to choose
the default output and input device.

Scope decided with the user:

- Volume and mute of the output (speakers, headphones) and of the input
  (microphone), plus the choice of the default device for each.
- Backend: the **PulseAudio protocol** through `libpulse`. It works with
  PulseAudio and with PipeWire (`pipewire-pulse`). Real time events, no
  polling.
- Position: the right side of the primary bar, next to the device notifier
  icon and before the CPU / network monitor (`widgets::sysmon`).
- Out of scope for this version: per application volume, profiles and
  ports (for example headphones against speakers on the same card), wheel
  and middle click on the icon, volume above 100%, Wayland specifics.

## Why libpulse

`pactl subscribe` plus `pactl --format=json` would add no crate, but it
starts a process for every action and depends on the output format.
Native PipeWire needs `libpipewire-dev`, which is not installed, and the
Rust binding is heavy. `libpulse` 17 and its headers are already on the
system, and the PulseAudio protocol is served by both sound servers.

## Structure

- New crate `crates/audio` (`opusdm-audio`), **library only, no GTK**.
- New panel widget `crates/panel/src/widgets/audio.rs`, a pure row model
  `crates/panel/src/audio_model.rs` and a controller
  `crates/panel/src/audio_center.rs` (same split as `devices_model.rs`,
  `devices_center.rs` and `widgets/devices.rs`).
- No new binary: the panel lives inside the hub process.

### `opusdm-audio` modules

- `model`: `AudioDevice { index, name, description, kind (Output | Input), volume_percent, muted, is_default }` and `AudioEvent { Added(AudioDevice), Removed { kind, index }, Changed(AudioDevice) }`. Pure data.
- `filter`: `is_listed(&RawSource) -> bool`. Sources whose name ends in `.monitor` (the listening side of an output) are not inputs and are never listed.
- `volume`: pure conversion between a percentage and a PulseAudio volume (100% is `PA_VOLUME_NORM`, 0 dB). The percentage is clamped to `0..=100`. The average of the channel volumes gives `volume_percent`; setting a volume applies one value to every channel, so a balance set elsewhere is flattened (stated limitation).
- `backend`: trait `Backend` with `list()`, `set_volume(kind, index, percent)`, `set_mute(kind, index, muted)`, `set_default(kind, name)` and `watch(sender)`. Implemented by `PulseBackend` and by `FakeBackend` for tests, so the widget logic never needs a server.
- `pulse`: `PulseBackend` over `libpulse-binding` with a **threaded mainloop** on its own thread. It subscribes to sink, source and server events and sends `AudioEvent`s through an `async_channel` (the same pattern as `opusdm-devices`). Every request goes through that thread; the GTK main loop never calls libpulse.
- `error`: `AudioError` with the failing operation and the server message, mapped to a short readable text (`ConnectionFailed`, `NoSuchDevice`, `Rejected`).

### Panel widget

`widgets::audio::build(center)` returns a `gtk::MenuButton` with a speaker
icon drawn with cairo (like the other panel icons, so it does not depend
on the icon theme). The symbol follows the default output: muted, low,
medium, high. The tooltip shows the percentage, or "Muted".

The popover holds two sections, **Output** and **Input**, each with:

- A list of devices (check mark on the default one). A click makes a device the default.
- A slider `0..=100` and a mute toggle for the default device of the section.
- The section is hidden when it has no device.

The button is hidden while the server is not reachable.

Operations are sent to the backend thread and the answers come back through
`async_channel`. The slider sends a value on each change; the center keeps
only the last pending value per device so a fast drag does not queue up
requests.

Placement in `lib.rs`: in `right`, after the device button and before
`widgets::sysmon::build()`. Only the primary bar gets it, as the tray and
the device button do.

## Error handling

- No server or connection lost: the widget stays hidden and one line is
  logged (`eprintln!` as the rest of the panel does). It retries every 30
  seconds, so starting the sound server later works.
- A failed action (device gone, request rejected) is shown as a one line
  message at the top of the popover for 5 seconds. Nothing is swallowed
  silently and nothing panics.
- A device that disappears mid-operation (unplugged headphones) is removed
  by the `Removed` event and the failed call is ignored.
- While the slider is being dragged, incoming `Changed` events for that
  device do not move it.

## Testing

- `volume`: percentage to PulseAudio volume and back, clamping, the average over uneven channels.
- `filter`: monitor sources are dropped, real inputs and outputs are kept.
- `backend`: set volume, mute and default with the `FakeBackend`, and the events they produce.
- Panel: the row model (default device, mute state, icon level, hidden section, events added, removed and changed, ignoring a `Changed` while dragging) is a pure struct tested without GTK, as `devices_model.rs` does.
- By hand under the real session: slider, mute, switching the default output and input, plugging and unplugging headphones, restarting `pipewire-pulse` while the panel runs. Whatever is not checked by hand will be stated in `CHANGES.md`.
