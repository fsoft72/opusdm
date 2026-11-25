# Task 6 report

## What
`_end_popup_grabs_for` in crates/compositor/src/shell/layer.rs: when a layer becomes the exclusive keyboard layer (new or on a later commit), the roots of the active keyboard/pointer grabs (grab start focus) other than the layer itself go through `State::dismiss_popups_and_grabs` (popup_done, grabs unset, debug_log!), then the layer takes the keyboard. No grab: unchanged.

## TDD
RED: `env -u DISPLAY -u WAYLAND_DISPLAY cargo test -p opusdm-compositor --test headless an_exclusive_layer_mapped_over_an_open_menu` -> `assertion failed: c.popup_done(&menu)`, 1 failed.
GREEN: after the change, full crate: 76 unit, 49 headless, 15 ipc pass; clippy -D warnings clean; headless loop 5x all 49 pass.

## Files
crates/compositor/src/shell/layer.rs, crates/compositor/tests/headless.rs, CHANGES.md

## Self-review
Ownership unchanged (no Weak/Rc changes). Own-menu grab of the exclusive layer kept (stage 1 test green).

## Concerns
No test for "becomes exclusive on a later commit" (same code path). Pointer grab end not asserted. Not run in nested/live session.

## Fix after review
- layer.rs: `_end_popup_grabs_for` reads the root from the keyboard grab only (pointer branch and dedup removed), so a ClickGrab / MoveGrab is never ended.
- New test `a_layer_that_becomes_exclusive_over_an_open_menu_closes_it` + harness `set_layer_keyboard` (tests/common/mod.rs).
- Commands: `env -u DISPLAY -u WAYLAND_DISPLAY cargo test -p opusdm-compositor` -> 76 unit, 50 headless, 15 ipc ok; clippy -D warnings clean; headless loop 5x: 50 passed each.
- Not verified: pointer-grab case (no pointer injection), nested/live.
