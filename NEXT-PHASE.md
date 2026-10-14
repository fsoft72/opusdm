# NEXT-PHASE.md - deferred work

Things that were decided on purpose to leave out of the current phase, so they
are not forgotten.

## Viewer: configurable read limits (from F-16 / F-17)

The Viewer window (Read / AnsiRead / HexRead / SmartRead / Show) reads files up
to fixed caps and shows a "showing the first N" notice beyond them:

- text and ANSI: 16 MiB
- hex: 1 MiB
- Quick Look keeps its own `MAX_READ` of 256 KiB

Add a setting in the configuration (BEHAVIOUR section of Settings,
`settings.json`) to choose these sizes: `MAX_READ` for Quick Look and the text /
ANSI / hex caps of the Viewer, with a sane clamped range like the other
`Behavior` fields. Until then the values are constants in the code.

Paged reading on demand (a virtual line model, search over the whole file from
disk) is a separate, larger project and is not planned yet.

## Search: configurable limits (from F-19 / F-20)

Find / Search in Files stops at 5000 results (`MAX_RESULTS`) and reads at most
16 MiB of each file (`SEARCH_FILE_MAX`). Make both settings alongside the Viewer
read caps above.

## Button banks: not done (from F-02)

Open a bank under the mouse, the Start Menu variant (a bank shown as a popup
menu), the Lister toolbar as an editable bank, drag and drop reordering of
buttons, and saving the window position on every move (it is saved when the
window closes and before the hub quits).

## Notifications: not done (from the notification server)

`opusdm-notify` and the panel bell cover popups, history, Do not disturb and
the mute list. Left out on purpose:

- sound (the `sound-file` / `sound-name` hints, needs GStreamer or canberra);
- a Settings page for the corner and the popup time (edited in
  `~/.config/opusdm/notifications.json` for now);
- a history saved across logins;
- per-app rules beyond mute (custom time, drop, no sound);
- a slide animation when the stack moves (popups only fade in).
