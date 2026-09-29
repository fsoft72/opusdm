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
