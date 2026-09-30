# SetAsBackground (F-37) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `SetBackground` internal command and a Lister context menu entry that install an image as the desktop background, applied live and persisted.

**Architecture:** `SetBackground` is a new `InternalCmd` handled by the hub function executor like `ChangeDir`. It validates the path and fit, then emits `FnEvent::SetBackground`. The hub server applies it by cloning `current_settings`, editing `theme.background`, and running the same code as `ToHub::SetSettings` (extracted into `Hub::apply_settings`), which persists and emits `HubEvent::ThemeChanged` so the GTK side redraws the CSS. The Lister menu entry runs an ad-hoc one-step function through `ToHub::RunFunction`.

**Tech Stack:** Rust workspace (crates `ipc`, `hub`, `lister`), GTK4, serde. Tests with `cargo test`.

**Spec:** `docs/superpowers/specs/2026-09-30-set-as-background-design.md`

## Global Constraints

- Background lives in `Settings.theme.background` (`opusdm_ipc::Background`), never in `DesktopSettings`.
- Accepted image extensions: png, jpg, jpeg, webp, bmp, gif (case-insensitive).
- Accepted `fit` values: `fill`, `fit`, `center`, `tile` (case-insensitive); empty or absent keeps the current fit.
- Command label "Set Background"; params `path` (required, `PathTemplate`) and `fit` (optional, `Text`).
- Context menu entry text: "Set as Desktop Background".
- Error messages start with `Set Background:` and include the offending value.
- English only in code and comments; never use the long dash character; constants in UPPER_CASE; every function gets a doc comment; text files end with an empty line.
- Commit after each task with a meaningful message. No AI attribution lines. Never use `--no-verify`. Do not push.
- Run `cargo test -p <crate>` for the touched crate; run `cargo build --all` and `cargo clippy --all` at the end.

## Review Focus

- File name containing `{f}` (or any placeholder) chosen from the context menu: it would be expanded as a placeholder. The Lister refuses it with a clear message (Task 4); the executor keeps unknown `{x}` text literal (Task 2 test).
- Symlink named `wall.png` pointing at a real image is accepted; a dangling symlink fails with a message (Task 2 tests).
- Upper-case extension `WALL.PNG` is accepted (Task 1 test).
- Path with spaces or non-ASCII characters ends up percent-encoded in the CSS URL (Task 1 test).
- A failing step (wrong extension, bad fit) leaves the background untouched: no `SetBackground` event (Task 2 tests).

---

### Task 1: ipc helpers and the `SetBackground` command

**Files:**
- Modify: `crates/ipc/src/desktop.rs` (add `ImageFit::parse`, `BACKGROUND_IMAGE_EXTS`, `is_background_image`, tests)
- Modify: `crates/ipc/src/function.rs` (variant, label, schema, tests)
- Modify: `crates/ipc/src/lib.rs:43-47` (re-export the new items)
- Modify: `crates/lister/src/function_editor.rs:371-382` (`INTERNAL_CMDS`)
- Modify: `crates/hub/src/functions.rs` (temporary `unreachable!` arm so the workspace compiles; replaced in Task 2)

**Interfaces:**
- Produces:
  - `opusdm_ipc::desktop::ImageFit::parse(text: &str) -> Option<ImageFit>`
  - `opusdm_ipc::desktop::BACKGROUND_IMAGE_EXTS: [&str; 6]`
  - `opusdm_ipc::desktop::is_background_image(path: &std::path::Path) -> bool`
  - `InternalCmd::SetBackground` with schema keys `path` and `fit`
  - all three helpers re-exported at the crate root as `opusdm_ipc::{ImageFit, is_background_image, BACKGROUND_IMAGE_EXTS}` (`ImageFit` already is)

- [ ] **Step 1: Write the failing tests in `crates/ipc/src/desktop.rs`**

Append inside the existing `mod tests`:

```rust
    #[test]
    fn image_fit_parse_accepts_the_four_names_case_insensitively() {
        assert_eq!(ImageFit::parse("fill"), Some(ImageFit::Fill));
        assert_eq!(ImageFit::parse(" FIT "), Some(ImageFit::Fit));
        assert_eq!(ImageFit::parse("Center"), Some(ImageFit::Center));
        assert_eq!(ImageFit::parse("tile"), Some(ImageFit::Tile));
        assert_eq!(ImageFit::parse("stretch"), None);
        assert_eq!(ImageFit::parse(""), None);
    }

    #[test]
    fn is_background_image_checks_the_extension_case_insensitively() {
        for name in ["a.png", "a.JPG", "a.jpeg", "a.WebP", "a.bmp", "a.gif"] {
            assert!(is_background_image(std::path::Path::new(name)), "{name}");
        }
        for name in ["a.txt", "a.svg", "png", "a."] {
            assert!(!is_background_image(std::path::Path::new(name)), "{name}");
        }
    }

    #[test]
    fn background_css_percent_encodes_spaces_in_the_image_path() {
        let bg = Background {
            mode: BackgroundMode::Image,
            image_path: Some(PathBuf::from("/tmp/my wall.png")),
            ..Background::default()
        };
        assert!(background_css(&bg).contains("my%20wall.png"));
    }
```

- [ ] **Step 2: Write the failing tests in `crates/ipc/src/function.rs`**

Append inside the existing `mod tests`:

```rust
    #[test]
    fn set_background_schema_has_a_required_path_and_an_optional_fit() {
        let params = schema::params_for(InternalCmd::SetBackground);
        assert_eq!(params.len(), 2);
        assert_eq!((params[0].key, params[0].required), ("path", true));
        assert_eq!((params[1].key, params[1].required), ("fit", false));
        assert_eq!(InternalCmd::SetBackground.label(), "Set Background");
    }

    #[test]
    fn set_background_step_survives_a_json_roundtrip() {
        let mut args = BTreeMap::new();
        args.insert("path".to_string(), "{f}".to_string());
        let step = FnStep::Internal { cmd: InternalCmd::SetBackground, args: InternalArgs(args) };
        let json = serde_json::to_string(&step).unwrap();
        assert_eq!(serde_json::from_str::<FnStep>(&json).unwrap(), step);
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p opusdm-ipc 2>&1 | tail -20`
Expected: compile errors (`no variant SetBackground`, `no function parse`).

- [ ] **Step 4: Implement in `crates/ipc/src/desktop.rs`**

Directly after the `ImageFit` enum add:

```rust
impl ImageFit {
    /// Parse `fill` / `fit` / `center` / `tile`, ignoring case and
    /// surrounding whitespace. Anything else is `None`.
    pub fn parse(text: &str) -> Option<ImageFit> {
        match text.trim().to_ascii_lowercase().as_str() {
            "fill" => Some(ImageFit::Fill),
            "fit" => Some(ImageFit::Fit),
            "center" => Some(ImageFit::Center),
            "tile" => Some(ImageFit::Tile),
            _ => None,
        }
    }
}

/// File extensions (lower case) accepted as a desktop background image.
pub const BACKGROUND_IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];

/// Whether `path` has one of the [`BACKGROUND_IMAGE_EXTS`] extensions,
/// ignoring case. Looks at the name only, never at the file content.
pub fn is_background_image(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| BACKGROUND_IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}
```

- [ ] **Step 5: Implement in `crates/ipc/src/function.rs`**

Add the variant after `ChangeDir` in `InternalCmd`:

```rust
    /// Install an image as the desktop background.
    SetBackground,
```

Add to `label()`:

```rust
            InternalCmd::SetBackground => "Set Background",
```

Add to `params_for()` after the `ChangeDir` arm:

```rust
            InternalCmd::SetBackground => &[
                ParamSpec {
                    key: "path",
                    label: "Image file (png, jpg, jpeg, webp, bmp, gif)",
                    required: true,
                    kind: ParamKind::PathTemplate,
                },
                ParamSpec {
                    key: "fit",
                    label: "Fit: fill, fit, center or tile (empty = keep current)",
                    required: false,
                    kind: ParamKind::Text,
                },
            ],
```

- [ ] **Step 6: Re-export and register in the editor**

In `crates/ipc/src/lib.rs` extend the `pub use desktop::{...}` list with `is_background_image, BACKGROUND_IMAGE_EXTS,`.

In `crates/lister/src/function_editor.rs` append `InternalCmd::SetBackground,` after `InternalCmd::ChangeDir,` in `INTERNAL_CMDS`.

In `crates/hub/src/functions.rs`, inside `run_internal`'s `match cmd`, add next to the `ChangeDir` arm:

```rust
        InternalCmd::SetBackground => unreachable!("SetBackground is handled by run_set_background"),
```

Also add the dispatch arm in `run_steps` now, calling a function that Task 2 fills in. To keep this task compiling, add a stub right after `run_change_dir`:

```rust
/// Run a `SetBackground` step. Filled in by the next task.
fn run_set_background(
    _run_id: u64,
    _step_idx: usize,
    _args: &InternalArgs,
    _source: &SelectionCtx,
    _dest: Option<&SelectionCtx>,
    _events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    Err("Set Background: not implemented".to_string())
}
```

and in `run_steps` add before the generic `FnStep::Internal { cmd, args }` arm:

```rust
            FnStep::Internal { cmd: InternalCmd::SetBackground, args } => {
                run_set_background(run_id, step_idx, args, &source, dest, events_tx)
            }
```

- [ ] **Step 7: Run tests and build**

Run: `cargo test -p opusdm-ipc 2>&1 | tail -8 && cargo build --all 2>&1 | tail -5`
Expected: ipc tests PASS, workspace builds. If another exhaustive `match` on `InternalCmd` fails to compile, add the arm there (grep `InternalCmd::ChangeDir` shows every site).

- [ ] **Step 8: Commit**

```bash
git add crates/ipc crates/lister/src/function_editor.rs crates/hub/src/functions.rs
git commit -m "feat(functions): SetBackground command schema and image helpers"
```

---

### Task 2: Executor step

**Files:**
- Modify: `crates/hub/src/functions.rs` (`FnEvent::SetBackground`, `expand_home`, `run_set_background`, refactor `run_change_dir`, tests)

**Interfaces:**
- Consumes: `ImageFit::parse`, `is_background_image` (Task 1); `expand_text`, `expand_err_msg`, `SelectionCtx`.
- Produces: `FnEvent::SetBackground { path: PathBuf, fit: Option<ImageFit> }`, where `path` is canonical.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `mod tests` of `crates/hub/src/functions.rs`:

```rust
    fn set_background_steps(path: &str, fit: Option<&str>) -> Vec<FnStep> {
        let mut args = BTreeMap::new();
        args.insert("path".to_string(), path.to_string());
        if let Some(fit) = fit {
            args.insert("fit".to_string(), fit.to_string());
        }
        vec![FnStep::Internal { cmd: InternalCmd::SetBackground, args: InternalArgs(args) }]
    }

    fn has_set_background(events: &[FnEvent]) -> bool {
        events.iter().any(|e| matches!(e, FnEvent::SetBackground { .. }))
    }

    #[test]
    fn set_background_resolves_a_relative_path_and_parses_the_fit() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("wall.PNG"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("wall.PNG", Some("Tile")), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        let want = std::fs::canonicalize(td.path().join("wall.PNG")).unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            FnEvent::SetBackground { path, fit: Some(opusdm_ipc::ImageFit::Tile) } if *path == want
        )));
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn set_background_without_fit_keeps_the_current_one() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("wall.png"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("wall.png", None), &src, None, &tx);
        drop(tx);
        assert!(drain(rx).iter().any(|e| matches!(e, FnEvent::SetBackground { fit: None, .. })));
    }

    #[test]
    fn set_background_rejects_a_non_image_and_changes_nothing() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("notes.txt"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("notes.txt", None), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(!has_set_background(&events));
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("notes.txt")));
    }

    #[test]
    fn set_background_rejects_a_directory_and_a_missing_file() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir(td.path().join("dir.png")).unwrap();
        for name in ["dir.png", "missing.png"] {
            let (tx, rx) = channel();
            let src = ctx(&[], td.path().to_str().unwrap());
            run_steps(1, &set_background_steps(name, None), &src, None, &tx);
            drop(tx);
            let events = drain(rx);
            assert!(!has_set_background(&events), "{name}");
            assert!(matches!(events.last(), Some(FnEvent::Failed { .. })), "{name}");
        }
    }

    #[test]
    fn set_background_rejects_an_unknown_fit_and_changes_nothing() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("wall.png"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("wall.png", Some("stretch")), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(!has_set_background(&events));
        assert!(matches!(events.last(), Some(FnEvent::Failed { error, .. }) if error.contains("stretch")));
    }

    #[test]
    fn set_background_follows_a_symlink_and_fails_on_a_dangling_one() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("real.dat"), b"x").unwrap();
        std::os::unix::fs::symlink(td.path().join("real.dat"), td.path().join("link.png")).unwrap();
        std::os::unix::fs::symlink(td.path().join("gone.dat"), td.path().join("dangling.png")).unwrap();
        let src = ctx(&[], td.path().to_str().unwrap());

        let (tx, rx) = channel();
        run_steps(1, &set_background_steps("link.png", None), &src, None, &tx);
        drop(tx);
        assert!(has_set_background(&drain(rx)), "extension is checked on the name the user picked");

        let (tx, rx) = channel();
        run_steps(1, &set_background_steps("dangling.png", None), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(!has_set_background(&events));
        assert!(matches!(events.last(), Some(FnEvent::Failed { .. })));
    }

    #[test]
    fn set_background_keeps_unknown_braces_in_the_path_literal() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a{x}.png"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("a{x}.png", None), &src, None, &tx);
        drop(tx);
        assert!(has_set_background(&drain(rx)));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-hub set_background 2>&1 | tail -20`
Expected: compile error (`no variant SetBackground` in `FnEvent`).

- [ ] **Step 3: Implement**

Add to `FnEvent`, after `ChangeDir`:

```rust
    /// A `SetBackground` step validated `path` (canonical): the hub must
    /// install it as the desktop background. `fit` is `None` to keep the
    /// current one.
    SetBackground { path: PathBuf, fit: Option<opusdm_ipc::ImageFit> },
```

Add the shared helper before `run_change_dir`, and use it there:

```rust
/// Turn the user-typed `text` into a path: a leading `~` becomes `$HOME`,
/// anything else is returned as is (the caller joins it to the cwd).
/// `label` names the command in the error message.
fn expand_home(text: &str, label: &str) -> Result<PathBuf, String> {
    match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME").ok_or_else(|| format!("{label}: HOME is not set"))?;
            Ok(PathBuf::from(home).join(rest.trim_start_matches('/')))
        }
        _ => Ok(PathBuf::from(text)),
    }
}
```

In `run_change_dir` replace the `let expanded = match text.strip_prefix('~') { ... };` block with:

```rust
    let expanded = expand_home(text, "Change Directory")?;
```

Replace the Task 1 stub `run_set_background` with:

```rust
/// Run a `SetBackground` step: expand the path template, resolve `~` and
/// relative paths against the SOURCE cwd, require an existing regular file
/// with an image extension (checked on the name the user picked, so a
/// symlink `wall.png` is fine), parse the optional `fit`, and ask the hub to
/// install the image. Nothing is emitted when any check fails.
fn run_set_background(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let text = expand_text(args.get("path").unwrap_or(""), source, dest).map_err(expand_err_msg)?;
    let text = text.trim();
    if text.is_empty() {
        return Err("Set Background: path is empty".to_string());
    }
    let picked = source.cwd.join(expand_home(text, "Set Background")?);
    if !opusdm_ipc::is_background_image(&picked) {
        return Err(format!(
            "Set Background: \"{}\" is not a supported image (png, jpg, jpeg, webp, bmp, gif)",
            picked.display()
        ));
    }
    let fit = match args.get("fit").map(str::trim).filter(|f| !f.is_empty()) {
        Some(text) => Some(opusdm_ipc::ImageFit::parse(text).ok_or_else(|| {
            format!("Set Background: unknown fit \"{text}\" (use fill, fit, center or tile)")
        })?),
        None => None,
    };
    let target = std::fs::canonicalize(&picked)
        .map_err(|e| format!("Set Background: cannot open {}: {e}", picked.display()))?;
    if !target.is_file() {
        return Err(format!("Set Background: {} is not a file", target.display()));
    }
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("background {}", target.display()) }));
    let _ = events_tx.send((run_id, FnEvent::SetBackground { path: target, fit }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p opusdm-hub functions 2>&1 | tail -15`
Expected: all `set_background_*` tests and the existing `changedir_*` tests PASS. The hub crate will fail to compile at `server.rs` (non-exhaustive `match ev`) until Task 3; if so run only after Task 3, or add the temporary arm `crate::functions::FnEvent::SetBackground { .. } => return,` to the lister-message `match` in `function_event`.

- [ ] **Step 5: Commit**

```bash
git add crates/hub/src/functions.rs crates/hub/src/server.rs
git commit -m "feat(functions): execute the SetBackground step"
```

---

### Task 3: Hub applies the background

**Files:**
- Modify: `crates/hub/src/server.rs` (`apply_settings`, `apply_background_image`, `function_event`, `SetSettings` arm, test)

**Interfaces:**
- Consumes: `FnEvent::SetBackground` (Task 2).
- Produces: `Hub::apply_settings(&mut self, new_settings: opusdm_ipc::Settings)` and `Hub::apply_background_image(&mut self, path: PathBuf, fit: Option<opusdm_ipc::ImageFit>)`.

- [ ] **Step 1: Write the failing test**

Append inside `mod tests` of `crates/hub/src/server.rs`, next to `set_settings_persists_and_emits_a_changed_event`:

```rust
    #[test]
    fn set_background_step_applies_and_persists_the_image() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let img_dir = tempfile::tempdir().unwrap();
        let img = img_dir.path().join("wall.png");
        std::fs::write(&img, b"not really a png").unwrap();

        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut c = connect(&path);
        send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut c).unwrap().unwrap();

        let mut args = std::collections::BTreeMap::new();
        args.insert("path".to_string(), img.to_string_lossy().into_owned());
        args.insert("fit".to_string(), "tile".to_string());
        let steps = vec![opusdm_ipc::function::FnStep::Internal {
            cmd: opusdm_ipc::function::InternalCmd::SetBackground,
            args: opusdm_ipc::function::InternalArgs(args),
        }];
        send(&mut c, &ToHub::RunFunction { steps }).unwrap();
        let q: ToLister = recv(&mut c).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection));
        send(&mut c, &ToHub::SelectionReply { paths: vec![], cwd: img_dir.path().to_path_buf() }).unwrap();

        let mut got = None;
        while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(1000)) {
            if let HubEvent::ThemeChanged(t) = ev {
                got = Some(t);
                break;
            }
        }
        let theme = got.expect("ThemeChanged not emitted");
        assert_eq!(theme.background.mode, opusdm_ipc::BackgroundMode::Image);
        assert_eq!(theme.background.image_path, Some(std::fs::canonicalize(&img).unwrap()));
        assert_eq!(theme.background.image_fit, opusdm_ipc::ImageFit::Tile);
        wait_for_file(&cfg.path().join("opusdm").join("settings.json"));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p opusdm-hub set_background_step_applies 2>&1 | tail -20`
Expected: FAIL (timeout "ThemeChanged not emitted", or a compile error if the Task 2 note applied).

- [ ] **Step 3: Extract `apply_settings` and add `apply_background_image`**

Replace the `ToHub::SetSettings(mut new_settings) => { ... }` arm body in `lister_msg` with:

```rust
            ToHub::SetSettings(new_settings) => self.apply_settings(new_settings),
```

Add, next to `function_event` in the `impl Hub` block:

```rust
    /// Adopt `new_settings`: sanitize the background, persist to
    /// `settings.json`, push the change to every registered Lister and to
    /// the GTK side. Shared by `ToHub::SetSettings` and the `SetBackground`
    /// function step.
    fn apply_settings(&mut self, mut new_settings: opusdm_ipc::Settings) {
        new_settings.theme.background = new_settings.theme.background.sanitized();
        self.current_settings = new_settings;
        let _ = self.persist_tx.send(PersistMsg::Settings(self.current_settings.clone()));
        for c in self.conns.values_mut().filter(|c| c.registered) {
            let _ = c.tx.send(ToLister::SettingsUpdate(self.current_settings.clone()).into());
        }
        let _ = self.evt_tx.send(HubEvent::ThemeChanged(self.current_settings.theme.clone()));
        let _ = self.evt_tx.send(HubEvent::AppearanceChanged(self.current_settings.appearance.clone()));
    }

    /// Install `path` as the desktop background image. `fit` overrides the
    /// current fit when `Some`; `None` keeps it.
    fn apply_background_image(&mut self, path: PathBuf, fit: Option<opusdm_ipc::ImageFit>) {
        let mut settings = self.current_settings.clone();
        let background = &mut settings.theme.background;
        background.mode = opusdm_ipc::BackgroundMode::Image;
        background.image_path = Some(path);
        if let Some(fit) = fit {
            background.image_fit = fit;
        }
        self.apply_settings(settings);
    }
```

- [ ] **Step 4: Handle the event in `function_event`**

Directly after the `if terminal { ... }` block (before `if requester == HUB_REQUESTER`), add:

```rust
        // A background change applies for every requester, hub-originated
        // runs (script events) included; the requester still gets the
        // step's Output and StepDone lines from the executor.
        if let crate::functions::FnEvent::SetBackground { path, fit } = &ev {
            self.apply_background_image(path.clone(), *fit);
            return;
        }
```

In the `let msg = match ev { ... }` below, make sure the temporary arm from Task 2 is present, with an accurate comment:

```rust
            // Applied above, before this match.
            crate::functions::FnEvent::SetBackground { .. } => return,
```

- [ ] **Step 5: Run the hub tests**

Run: `cargo test -p opusdm-hub 2>&1 | tail -15`
Expected: new test PASS, existing `set_settings_persists_and_emits_a_changed_event` still PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/hub/src/server.rs
git commit -m "feat(hub): apply SetBackground steps to the desktop background"
```

---

### Task 4: Lister context menu entry

**Files:**
- Modify: `crates/lister/src/context_menu.rs` (new entry after the Quick Look block)

**Interfaces:**
- Consumes: `opusdm_ipc::is_background_image`, `InternalCmd::SetBackground` (Task 1), `ToHub::RunFunction`, `crate::actions::flash`.
- Produces: none.

- [ ] **Step 1: Add the entry**

In `show_context_menu`, after the `// 3. Quick Look` block (the block that builds the "Quick Look (Preview)" button and appends it), add:

```rust
    // Set as Desktop Background (image files only). The path goes in as a
    // literal template, so a name holding a `{...}` placeholder is refused
    // instead of being expanded.
    if !row.is_dir && opusdm_ipc::is_background_image(&target_path) {
        let ui = ui.clone();
        let pop = popover.clone();
        let target_path = target_path.clone();
        let btn = menu_button("Set as Desktop Background");
        btn.connect_clicked(move |_| {
            pop.popdown();
            let text = target_path.to_string_lossy().into_owned();
            if text.contains('{') {
                crate::actions::flash(&ui, "cannot set a background from a path containing '{'");
                return;
            }
            let mut args = std::collections::BTreeMap::new();
            args.insert("path".to_string(), text);
            let steps = vec![opusdm_ipc::function::FnStep::Internal {
                cmd: opusdm_ipc::function::InternalCmd::SetBackground,
                args: opusdm_ipc::function::InternalArgs(args),
            }];
            let _ = ui.hub.client.send(&opusdm_ipc::ToHub::RunFunction { steps });
        });
        menu_box.append(&btn);
    }
```

If `Row` has no `is_dir` field, use the field that marks directories in `crates/lister/src/model.rs` (grep `pub struct Row`); the `is_background_image` check alone is enough when a directory named `x.png` is rare, but the hub rejects it anyway ("is not a file").

- [ ] **Step 2: Build and lint**

Run: `cargo build -p opusdm-lister 2>&1 | tail -10 && cargo clippy -p opusdm-lister 2>&1 | tail -10`
Expected: no errors or new warnings.

- [ ] **Step 3: Manual check**

Run: `./run-x11.sh` (see the script header). In a Lister open a folder with a PNG, right-click it, choose "Set as Desktop Background": the desktop changes live. Restart the hub: the image is still there. Right-click a `.txt`: the entry is absent.

- [ ] **Step 4: Commit**

```bash
git add crates/lister/src/context_menu.rs
git commit -m "feat(lister): Set as Desktop Background context menu entry"
```

---

### Task 5: Documentation and final verification

**Files:**
- Modify: `CHANGES.md` (new `## 2026-09-30` section at the top)
- Modify: `TODO.md` (F-37 line)
- Modify: `FEATURES.md` (F-37 spec)
- Modify: `docs/usage.md` (internal command list, if it lists `Change Directory`)

- [ ] **Step 1: CHANGES.md**

Insert above `## 2026-09-29`:

```markdown
## 2026-09-30

- Functions: new internal command "Set Background" (`InternalCmd::SetBackground`, required `path` with placeholders, `~` and cwd-relative paths, optional `fit` = fill / fit / center / tile, empty keeps the current fit). The step checks the file exists and has a png / jpg / jpeg / webp / bmp / gif extension, then the hub sets `theme.background` to that image and applies it through the same path as `ToHub::SetSettings` (now `Hub::apply_settings`): persisted to `settings.json`, live redraw. Hub-originated runs (script events) apply it too. New helpers `ImageFit::parse` and `is_background_image` in `opusdm_ipc::desktop`.
- Lister: right-click an image file, "Set as Desktop Background" (refused for paths containing `{`).
```

- [ ] **Step 2: TODO.md**

Change line 66 to `- [x] F-37 SetAsBackground command (palette use needs a saved function with `SetBackground {f}`; the filetype action waits for F-06)`. Leave item 6 of "Suggested order" unchecked: F-33, F-34 and F-38 are still open.

- [ ] **Step 3: FEATURES.md**

In the F-37 spec paragraph, replace "palette command + filetype action ... updating `DesktopSettings` (already supports image + fit mode)" with: internal command `SetBackground` (params `path`, optional `fit`) updating `Settings.theme.background`, plus a Lister context menu entry on image files; the filetype action arrives with F-06.

- [ ] **Step 4: docs/usage.md**

Run: `grep -n "Change Directory" docs/usage.md`. If found, add a matching "Set Background" line next to it; if not found, skip.

- [ ] **Step 5: Full verification**

Run: `cargo build --all 2>&1 | tail -3 && cargo test --all 2>&1 | grep -E "^test result|FAILED|failed" && cargo clippy --all 2>&1 | tail -5`
Expected: build OK, every `test result: ok`, no clippy errors.

- [ ] **Step 6: Commit**

```bash
git add CHANGES.md TODO.md FEATURES.md docs/usage.md docs/superpowers/specs/2026-09-30-set-as-background-design.md docs/superpowers/plans/2026-09-30-set-as-background.md
git commit -m "docs: SetBackground (F-37) changes, roadmap and usage"
```
