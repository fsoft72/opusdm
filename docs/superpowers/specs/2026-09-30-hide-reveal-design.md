# F-38 Hide and Reveal - design

Status: decisions taken by the implementer on the user's explicit delegation
("do everything yourself without asking"); no separate approval stage.
Last item of roadmap step 6 (Desktop features).

## Goal

One action hides the whole working environment, leaving only the desktop;
another action (hotkey, menu, function) brings it back exactly as it was.

## Decisions

- Hidden means: every Lister window is minimized, the panel and the dock are
  hidden, open group windows are hidden. The desktop window (background and
  icons) stays: it is the "backdrop" and carries the way back.
- Ways in and out: three internal commands `Hide`, `Reveal` and `Toggle Hide`
  (no parameters, no selection needed), so any function, user menu entry or
  global hotkey (F-03) can drive it, plus a "Hide All" / "Reveal" button in the
  desktop context menu. The hotkey acceptance ("hotkey toggles hidden and
  revealed") is a saved function `Toggle Hide` bound to a global hotkey.
- The hub is the single owner of the state (`hidden`, not persisted; it starts
  revealed). `SetHidden` to the same value does nothing. A change is sent to
  every registered Lister (`ToLister::SetHidden`) and to the GTK side
  (`HubEvent::HiddenChanged`).
- A Lister that was already minimized by the user is left alone on hide and
  stays minimized on reveal; only windows the hub minimized are restored
  (`opusdm-lister` `hidden::next_step`, a pure function).
- A new Lister registering while hidden reveals everything: the user opened it
  to see it.
- Panel and dock windows are found by their window title
  (`opusdm_panel::WINDOW_TITLE`, `opusdm_dock::WINDOW_TITLE`).

## Out of scope

Hiding to a clock or an AppIcon (the original's other hide methods), hiding
the progress window, persisting the hidden state, a Lister-side command (the
Listers are minimized while hidden; use the hotkey or the desktop).

## Error handling

Nothing can fail at runtime beyond GTK ignoring a minimize request (a window
manager that does not support it): the window then simply stays visible.

## Testing

`hidden::next_step` (all transitions), the protocol roundtrip of `SetHidden`,
the schema of the three commands, the executor steps, and server tests: state
change reaches the Lister and the GTK event exactly once, toggle flips it, the
function steps drive it, and a Lister registering while hidden reveals. The
GTK side (`apply_hidden`, the menu button) is verified by build and clippy;
the manual acceptance needs an X11 session.
