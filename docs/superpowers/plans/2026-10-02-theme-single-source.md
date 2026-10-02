# Theme as the Single Source of the Look - Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every look value has one editor: the theme (Theme Editor) owns colours, appearance, fonts, pictures, icons, sounds and bundled wallpapers; the Settings window owns preferences (theme choice, light / dark, wallpaper, icon grid, folder, panel, dock, menu, behaviour).

**Architecture:** `resolve_look` replaces the partial merge: a theme fully determines the look (defaults where unset), preferences are copied through. `settings.json` stays the runtime cache, written only through `resolve_look`. A one-time migration in `opusdm-settings` turns existing customisations into "My Theme". The hub is unchanged.

**Spec:** `docs/superpowers/specs/2026-10-02-theme-single-source-design.md`

## Global Constraints

- English in code and comments, never the long dash, constants `UPPER_SNAKE_CASE`, doc comment on every function, early returns at the top.
- Old `settings.json` and `theme.json` files must still load.
- `cargo test -p <crate>` after each task; never `--no-verify`; no push. Tasks 1 to 8 change shared types and only compile together, so they land in one commit.

## Tasks

- [x] **1. ipc model.** `CustomThemeData`: remove `behavior` and `desktop`; keep `background` documented as legacy, read only (`skip_serializing_if = "Option::is_none"`). `Settings.look_version: u32` plus `CURRENT_LOOK_VERSION`. `themes::WALLPAPERS_DIR`, `themes::theme_wallpapers(slug)` (sorted pictures in `wallpapers/` plus the legacy `background.image`, no duplicates). Doc comment on `Theme.background`: a preference. Tests: old `theme.json` with `behavior` / `desktop` still parses; `theme_wallpapers` lists, sorts, filters non-pictures, adds the legacy picture.
- [x] **2. resolve_look.** `apply.rs`: `resolve_look(slug, data, prefs) -> Settings` replaces `merge_custom_theme` / `MergedTheme`; `model::apply_icon_set` becomes `model::resolve_selected(icon_set, prefs) -> Settings` (desktop untouched). Tests: unset sections give defaults, `theme_mode` / `behavior` / `background` come from prefs, surfaces resolve inside the theme, unsafe names ignored.
- [x] **3. writer, draft, builder.** `ThemeDraft` loses `behavior`, `desktop`, `background`. `AssetOp::AddWallpaper` / `RemoveWallpaper` (picture extensions). `stage` carries `wallpapers/` from the source plus its legacy wallpaper; never writes `background`. `draft::queue_remove_wallpaper`. `builder::save_theme` writes only theme-owned values. `writer::is_read_only(slug)` for the default theme; `write_theme` always refuses it (the default theme is installed by `ensure_default_theme`, not by the writer). Tests for each.
- [x] **4. importer.** Prune `wallpapers/` to pictures. Test.
- [x] **5. migration.** `migrate.rs`: `migrate_look(settings) -> Result<Option<Migrated>, BuildError>` (`Migrated` holds the settings to persist and the slug created, if any), `apply::same_look(a, b)`, `free_migration_slug()`; `main::migrated_settings` persists through `apply_full`. Called from `build_window` before anything reads the settings. Tests: equal look only bumps the version, different look creates `my-theme` (then `my-theme-2`) carrying the icons, version already current does nothing.
- [x] **6. BackgroundSection "From theme...".** `desktop-config/src/ui.rs`: `connect_theme_pictures(provider)` shows the button; a dialog with thumbnails; choosing sets Image mode and the path.
- [x] **7. Settings window.** Remove "Save theme...", PRESET / COLOURS / APPEARANCE from the Lister tab; Apply uses `resolve_selected`; the wallpaper provider reads the picker's current theme; run the migration and report it in the note line.
- [x] **8. Theme Editor.** Colours tab without Light / dark; Pictures tab with a WALLPAPERS asset list instead of DESKTOP BACKGROUND; `apply_theme` uses `resolve_selected`; read-only default theme (status message, Save / Save & Apply refused).
- [x] **9. Docs.** `CHANGES.md`, `FEATURES.md`, `OpusDM-Features-Guide.md` (section 19), this plan's checkboxes.
