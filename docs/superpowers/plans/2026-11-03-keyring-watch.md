# Keyring watch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A small `opusdm-keyring-watch` process, supervised by the hub, that shows a tray icon and a desktop notification when the Secret Service is missing or locked, with Unlock / Test / Restart / Check now.

**Architecture:** New crate `crates/keyring-watch` (lib with pure, tested logic + thin bin). The checker runs on a plain thread against a `Vault` trait (fake in tests, `secret-service` crate in production); a pure `Tracker` state machine decides when to raise or clear the alarm. The tray is a `ksni` StatusNotifierItem (spawned when unhealthy, shut down when healthy). The password window is a GTK child process of the same binary. The hub gets a supervisor thread like `applet_host`.

**Tech Stack:** Rust 2021, `secret-service` 5.2, `zbus` 5.19, `ksni` 0.3 (`async-io` + `blocking`, no tokio), `zeroize`, `gtk4`.

**Spec:** `docs/superpowers/specs/2026-11-03-keyring-watch-design.md`

## Global Constraints

- English in code and comments; no em dash anywhere; files end with an empty line; constants UPPER_CASE; every function has a doc comment; exit conditions at the top as one liners.
- Project rules: no silent early return on a failure path (`opusdm_ipc::err_log!`); the password is never in a log, file, argument list, env var or clipboard; one risky change per commit, never push; no AI attribution in commit messages; never `--no-verify`.
- Check every 30 s, first check after 20 s grace, two failed checks in a row raise the alarm, one good check clears it.
- Single instance exits with `opusdm_ipc::instance::EXIT_ALREADY_RUNNING` (3).
- `CHANGES.md` updated with what was NOT verified.

## Review Focus

- Keyring locked then unlocked: icon and notification appear once, icon goes away, no repeat notification while the reason is unchanged.
- Reason changes while unhealthy (locked then service stopped): notification again, replacing the old one.
- Check call hangs (daemon stuck): counts as unhealthy with its own reason, never blocks the tray.
- Wrong password: window stays open and says so; the password is not kept.
- Test fails halfway: the probe item is still deleted.
- Second watcher (restarted hub): exits 3, supervisor waits and retries, no double tray icon.

---

### Task 1: Crate skeleton and health state machine

**Files:**
- Create: `crates/keyring-watch/Cargo.toml`, `crates/keyring-watch/src/lib.rs`, `crates/keyring-watch/src/health.rs`, `crates/keyring-watch/src/main.rs` (prints nothing, `fn main() {}` for now)
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces in `health.rs`:
  - `pub enum Reason { NoBus, NoService, NoCollection, Locked, Timeout }` with `pub fn text(&self) -> &'static str` (plain words shown to the user, e.g. `Locked` -> "The keyring is locked: saved passwords are not available").
  - `pub enum Health { Healthy, Unhealthy(Reason) }`
  - `pub enum Event { None, Raise(Reason), Clear }`
  - `pub struct Tracker`; `Tracker::new(grace_checks: u32)`; `pub fn observe(&mut self, h: Health) -> Event`.
  - Rules: the first `grace_checks` observations never raise; two consecutive unhealthy observations raise (`Raise`); while raised, a different reason gives `Raise(new)` after one observation; same reason gives `None`; one healthy observation while raised gives `Clear`; healthy while not raised gives `None`.

- [ ] **Step 1: Write failing tests in `health.rs`**: `first_failure_does_not_raise`, `two_failures_raise_once`, `same_reason_does_not_repeat`, `reason_change_raises_again`, `one_good_clears`, `grace_checks_are_ignored` (with `Tracker::new(2)`: two unhealthy observations give `None`).
- [ ] **Step 2: Run** `cargo test -p opusdm-keyring-watch health` -> FAIL (not compiling).
- [ ] **Step 3: Implement** the types and `Tracker` above (private fields: `seen: u32`, `grace: u32`, `streak: u32`, `last: Option<Reason>`, `raised: Option<Reason>`).
- [ ] **Step 4: Run** the same command -> PASS.
- [ ] **Step 5: Commit** `git add Cargo.toml Cargo.lock crates/keyring-watch && git commit -m "keyring-watch: crate and health state machine"`.

### Task 2: Vault probe

**Files:**
- Create: `crates/keyring-watch/src/probe.rs`
- Modify: `crates/keyring-watch/src/lib.rs` (`pub mod probe;`), `Cargo.toml` of the crate (`secret-service`, `zbus`)

**Interfaces:**
- Consumes: `Health`, `Reason`.
- Produces:
  - `pub trait Vault { fn bus_ok(&self) -> bool; fn service_present(&self) -> Result<bool, String>; fn default_locked(&self) -> Result<Option<bool>, String>; fn login_locked_if_distinct(&self) -> Result<Option<bool>, String>; }` (`None` = no such collection).
  - `pub fn evaluate(v: &dyn Vault) -> Health` (order: bus, service, default collection, locked, login if distinct; any `Err` from the vault -> `Unhealthy(Timeout)`... only for a timeout error string `"timeout"`; other `Err` -> `Unhealthy(NoService)` and logged with `err_log!`).
  - `pub struct SecretServiceVault` implementing `Vault` with the `secret-service` blocking API (`EncryptionType::Dh`, alias `default`, label `login`), and `pub fn probe_with_timeout(timeout: Duration) -> Health` that runs `evaluate(&SecretServiceVault)` on a worker thread and returns `Unhealthy(Timeout)` when it does not answer in time (15 s).

- [ ] **Step 1: Write failing tests** with a `FakeVault`: `healthy_when_all_is_unlocked`, `no_service_is_reported_before_collections`, `no_default_collection`, `locked_default`, `distinct_locked_login_is_unhealthy`, `vault_error_is_logged_not_swallowed` (returns `Unhealthy`), `slow_probe_times_out` (test `probe_with_timeout` helper taking a closure).
- [ ] **Step 2: Run** `cargo test -p opusdm-keyring-watch probe` -> FAIL.
- [ ] **Step 3: Implement** `evaluate`, the timeout helper (`std::thread` + `mpsc::recv_timeout`) and `SecretServiceVault` (service presence: `zbus::blocking::fdo::DBusProxy::name_has_owner("org.freedesktop.secrets")`; bus_ok: session bus connects).
- [ ] **Step 4: Run** -> PASS. **Step 5: Commit** "keyring-watch: vault probe".

### Task 3: Self test (Test action)

**Files:** Create `crates/keyring-watch/src/selftest.rs`; modify `lib.rs`.

**Interfaces:**
- Produces: `pub trait ItemStore { fn create(&self, value: &str) -> Result<String, String>; fn read(&self, id: &str) -> Result<String, String>; fn delete(&self, id: &str) -> Result<(), String>; }`, `pub fn run(store: &dyn ItemStore, value: &str) -> Result<(), String>` (create, read, compare, delete; on any failure after create the delete is still tried and a delete failure is appended to the error), `pub struct SecretServiceStore` (item label `opusdm-keyring-watch test`, attribute `xdg:schema=org.opusdm.KeyringWatchTest`, in the collection behind `default`), `pub fn random_value() -> String` (hex of `/dev/urandom` bytes via `getrandom`).

- [ ] **Step 1: Failing tests** with a `FakeStore`: `ok_creates_reads_and_deletes`, `mismatch_still_deletes`, `read_error_still_deletes`, `delete_error_is_reported`.
- [ ] **Step 2-4:** run FAIL, implement, run PASS.
- [ ] **Step 5: Commit** "keyring-watch: self test".

### Task 4: gnome-keyring module

**Files:** Create `crates/keyring-watch/src/gnome.rs`; modify `lib.rs`, crate `Cargo.toml` (`zeroize`).

**Interfaces:**
- Produces: `pub fn is_gnome_comm(comm: &str) -> bool` (`gnome-keyring-d` and `gnome-keyring-daemon`), `pub fn owner_is_gnome() -> bool` (bus owner pid -> `/proc/<pid>/comm`), `pub fn unlock_args() -> Vec<&'static str>` (`["--unlock"]`), `pub fn restart_steps() -> Vec<Step>` with `pub enum Step { Kill(&'static str), Systemctl(&'static [&'static str]), Replace }` in order: stop `ksecretd`, `kwalletd5`, `kwalletd6` (pkill -x), then `systemctl --user restart gnome-keyring-daemon.service`, with `Replace` used only when `systemctl --user cat gnome-keyring-daemon.service` fails, `pub fn unlock_with_password(password: Zeroizing<String>) -> Result<(), String>` (spawns `gnome-keyring-daemon --unlock` with stdin piped, writes the password and `\n`, drops the pipe, waits, returns the stderr text as the error; never puts the password in args, env or logs), `pub fn restart(password: Zeroizing<String>) -> Result<(), String>`.

- [ ] **Step 1: Failing tests**: `comm_names_are_recognised`, `unlock_args_never_contain_a_password` (args are exactly `--unlock`), `restart_stops_other_wallets_first` (first three steps are the kills, in order), `unlock_with_a_fake_daemon` (a shell script in a tempdir named by the env var `OPUSDM_KEYRING_DAEMON` that copies stdin to a file: the file holds the password, the script's arguments are `--unlock`; the env var is read only in tests via `cfg(test)` helper `daemon_bin()`).
- [ ] **Step 2-4:** FAIL, implement, PASS.
- [ ] **Step 5: Verify by hand on this machine** that `printf 'x\n' | gnome-keyring-daemon --unlock` with a wrong password reports an error and does not hang (record the output in `CHANGES.md`). If it differs from what the module assumes, fix the module.
- [ ] **Step 6: Commit** "keyring-watch: gnome-keyring unlock and restart".

### Task 5: Tray, notification, password window and main

**Files:** Create `crates/keyring-watch/src/tray.rs`, `notify.rs`, `ask.rs`, `app.rs`; modify `main.rs`, crate `Cargo.toml` (`ksni = { version = "0.3", default-features = false, features = ["async-io", "blocking"] }`, `gtk4`, `opusdm-ui`, `opusdm-ipc`).

**Interfaces:**
- `notify.rs`: `pub fn send(summary: &str, body: &str, replaces_id: u32) -> Result<u32, String>` (`org.freedesktop.Notifications.Notify`, app name `OpusDM`, icon `dialog-password`).
- `tray.rs`: `pub struct KeyringTray { reason: Reason, gnome: bool, tx: Sender<Cmd> }` implementing `ksni::Tray` (id `opusdm-keyring-watch`, icon `dialog-password`, status Active, title "Keyring problem"); `pub enum Cmd { Unlock, UnlockWithPassword, Restart, Test, CheckNow }`; `pub fn menu_labels(reason: Reason, gnome: bool) -> Vec<(Cmd, &'static str)>` is pure and tested: first line (disabled, not a Cmd) is `reason.text()`; `Unlock` only for `Locked`; `Restart` only if `gnome` and reason is `NoService`, `Locked`, `Timeout`; `Test` for `Locked` and `Timeout`... exactly as the spec: Test when the check is good or only locked; `CheckNow` always.
- `ask.rs`: `pub fn run(mode: AskMode) -> i32` GTK window (entry with password visibility off, Unlock/Cancel, error label "Wrong password"), `pub enum AskMode { Unlock, Restart }`; exit code 0 ok, 1 cancelled, 2 failed.
- `app.rs`: `pub fn run() -> i32`: acquire single instance (D-Bus name `org.opusdm.KeyringWatch` with `DoNotQueue`; on failure exit `EXIT_ALREADY_RUNNING`), start the checker thread (`probe_with_timeout`, `Tracker::new(grace)` where the first check waits 20 s), react to `Event`: `Raise` -> spawn tray if not spawned (else `handle.update` the reason) + notification; `Clear` -> `handle.shutdown()`. A command thread executes `Cmd`s: `Unlock` = `Collection::unlock()`; `UnlockWithPassword` and `Restart` = spawn `current_exe --ask-password unlock|restart`; `Test` = `selftest::run`, result as notification; `CheckNow` = wake the checker.
- `main.rs`: arguments `--ask-password unlock|restart`, `--unlock` (same as `--ask-password unlock`), none = watcher. Unknown argument: print usage to stderr, exit 2.

- [ ] **Step 1: Failing tests** for `menu_labels` (per reason and per `gnome`), and for the arg parser (`parse_args(&[&str]) -> Mode`).
- [ ] **Step 2-4:** FAIL, implement all modules, PASS; `cargo build -p opusdm-keyring-watch` and `cargo clippy -p opusdm-keyring-watch` clean.
- [ ] **Step 5: Commit** "keyring-watch: tray, notification, password window".

### Task 6: Hub supervisor

**Files:** Create `crates/hub/src/keyring_host.rs`; modify `crates/hub/src/lib.rs` (`pub mod keyring_host;`), `crates/hub/src/main.rs` (call `opusdm_hub::keyring_host::start();` after `applet_host::start()`).

**Interfaces:**
- Consumes: `applet_host::{next_action, Action, MAX_RESTARTS, RESTART_WINDOW}`, `opusdm_ipc::instance::{sibling_bin_in, EXIT_ALREADY_RUNNING}`.
- Produces: `pub fn start()` (detached thread `opusdm-keyring-supervisor`, X11 and Wayland, never joined), module doc stating ownership: the thread owns the `Child`; when the hub exits the child keeps running and a restarted hub waits through `EXIT_ALREADY_RUNNING`. `fn restart_delay(n: usize) -> Duration` (1 s, 2 s, 4 s...).

- [ ] **Step 1: Failing tests** mirroring the `applet_host` process tests with `sh -c 'exit N'`: `restarts_after_a_crash`, `already_running_waits_and_retries`, `gives_up_after_too_many_restarts`, `missing_binary_ends_the_supervisor` (extract the loop as `supervise(spawn, wait, sleep)` with injected closures like `applet_host`).
- [ ] **Step 2-4:** FAIL, implement (compact loop reusing `next_action`; every exit and every give-up uses `err_log!`), PASS; `cargo test -p opusdm-hub`.
- [ ] **Step 5: Commit** with the project rule 2 answer in the body (who owns the process for the whole run, what happens when it is gone).

### Task 7: Packaging, docs, live checks

**Files:** Modify `scripts/build-release.sh` (`BINARIES` and header comment), `scripts/live-update.sh` (`PROCESSES`), `scripts/build-deb.sh` if it lists binaries, `CHANGES.md`, `docs/usage.md` (a short section).

- [ ] **Step 1:** add `opusdm-keyring-watch` next to `opusdm-notify` in those lists; `grep -n opusdm-notify scripts/*.sh` shows every place.
- [ ] **Step 2:** `cargo build --workspace` and `cargo test --workspace` and `cargo clippy --workspace` pass.
- [ ] **Step 3: Run it for real** (project rule 4), in the sandbox `run-x11.sh` when possible and otherwise in the running session: lock the collection with `busctl --user call org.freedesktop.secrets /org/freedesktop/secrets/collection/login org.freedesktop.Secret.Collection ...` or `Service.Lock`; see icon and notification; unlock with a wrong then the right password; stop the daemon; Test; kill the watcher and see the hub restart it; start a second watcher and see exit 3. Also run the Lister, hotkeys and hub checks of `CLAUDE.md` rule 4 that the change could affect (hub start).
- [ ] **Step 4:** `CHANGES.md` entry listing what was verified and what was NOT.
- [ ] **Step 5: Commit** "keyring-watch: packaging, docs and changes".
