# Job engine base (F-15, F-07) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox syntax.

**Goal:** Add `JobOptions` (filter and flags) plus `CopyAs`, `MoveAs`, `Duplicate` to the job engine and expose them in the Function Editor, the palette and the `recursive_filter` setting.

**Architecture:** `opusdm_ipc` holds the data model and a shared glob matcher. `opusdm_fsops` applies the filter in `scan` and the flags in the copy work pass. The hub forwards `JobOptions`. The Lister UI adds dialogs.

**Tech Stack:** Rust workspace, GTK4, walkdir, serde.

**Spec:** `docs/superpowers/specs/2026-09-29-job-engine-base-design.md`

## Global Constraints

- English only in code and comments; no long dash character.
- Every function has a doc comment; exits at the top; constants in UPPER_CASE.
- Old JSON payloads (`Op`, `RunJob`, `functions.json`) must still parse.
- `cargo test --workspace` and `cargo clippy` stay green; commit per task, no push, no attribution lines.

## Review Focus

- Move with a filter or `update`/`newer` must never delete a source file that was not copied.
- Empty glob, glob with only separators: rejected before touching files.
- `CopyAs` name count mismatch or a name containing `/`: rejected before touching files.
- `Duplicate` of `a` and `a.copy` in one run picks distinct names.

## Tasks

1. ipc: `glob` module, `JobOptions`, new `Op` variants, `RunJob.options`, `Behavior.recursive_filter` (tests: JSON compatibility, round trips, glob).
2. fsops: filtered scan, `update`/`newer`, copy-tracking for partial moves, `CopyAs`/`MoveAs`/`Duplicate`, `move_when_same`, filtered delete (tests per acceptance criterion).
3. hub: forward `JobOptions` through `HubCmd::RunJob`, `lister_run_job`, functions executor; schema params and new `InternalCmd`s.
4. lister: palette commands, naming dialog, recursive-filter pattern requester, settings toggle.
5. docs (`docs/usage.md`, `CHANGES.md`, `TODO.md`).
