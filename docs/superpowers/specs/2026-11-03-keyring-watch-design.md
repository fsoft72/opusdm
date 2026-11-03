# Keyring watch: design

Date: 2026-11-03. Status: waiting for the owner's approval.

## Intent and why

The owner once found that "the programs could not find their passwords any
more" and had no idea why: the keyring (gnome-keyring, collection `login`)
had been locked during the session and nothing said so. OpusDM needs a small
watcher that checks the Secret Service, stays invisible while all is well, and
shows a tray icon plus a desktop notification when it is not, with the reason
in plain words and the actions to fix it, including typing the login password
to unlock at once.

## Goals

- A new small binary `opusdm-keyring-watch`, started and restarted by the hub.
- Check the Secret Service now and then (see "Check"). Healthy: no icon.
  Unhealthy: tray icon (StatusNotifierItem) + one desktop notification when
  the state changes to unhealthy. The icon goes away by itself when the
  problem is gone.
- The tray menu shows the reason on its first line and offers: **Unlock**,
  **Test**, **Restart gnome-keyring**, **Check now**.
- Typing the login password in an OpusDM window unlocks the keyring at once.
- Generic where possible: the check, the Test and the Secret Service `Unlock`
  work with any provider of `org.freedesktop.secrets`. Only the password
  window and the restart are gnome-keyring specific, in their own module, and
  are hidden when the owner of the bus name is another program.

## Non-goals

- Changing how `opusdm-network` or any other crate uses the keyring.
- Fixing the other keyrings (`Default_N`) or merging them.
- Making a Secret Service where there is none (no installing, no enabling).
- Storing the password anywhere, ever.

## Anti-goals (a failure even if it "works")

- An icon or a notification when the keyring is fine (false alarms, also at
  session start while things are still coming up).
- The password in a log, a file, an argument list, an environment variable or
  the clipboard.
- A silent early return on a failure path: every one must log with
  `err_log!` (project rule 3).
- The hub, the Lister or the global hotkeys being affected in any way by this
  process dying or hanging.

## Decisions taken by the owner

- Keyring locked mid-session is the case that matters; login is by password
  (PAM unlocks `login`), so locked means something broke during the session.
- Checks are repeated, not only at start.
- Unlock must be possible from the tray, with a password window of OpusDM.
- A desktop notification when the icon appears.
- The hub starts the process and restarts it when it dies.
- The solution must not depend on gnome-keyring for detection.

## Design (calls made by the assistant are marked [call])

**Process.** One binary, crate `crates/keyring-watch` [call], next to the
other tools. It is not part of the hub process, so a hang here cannot touch
the hub.

**Supervision.** Same pattern as `applet_host` (`crates/hub/src/applet_host.rs`):
a detached supervisor thread in the hub owns the `Child`, restarts it with a
growing delay and a restart limit, treats "already running" (restarted hub)
as a wait-and-retry, and logs every exit. Single instance through the same
`opusdm_ipc::instance` socket mechanism [call]. Started on X11 and Wayland.
The commit that adds it must answer in its body: who owns the process for the
whole run, and what happens when it is gone (project rule 2).

**Check** (every 30 s [call], first one after a 20 s grace at start [call],
plus on demand from "Check now"). Unhealthy reasons, in this order:
1. Session bus not reachable.
2. Nobody owns `org.freedesktop.secrets` (bus activation is allowed to start
   it before this is reported [call]).
3. No collection behind the alias `default` (fall back to the collection
   labelled `login` [call]).
4. That collection is locked. Apps use the alias, so the collection behind
   `default` is the one that counts; if a collection labelled `login` exists
   and is a different one, it must be unlocked too [call]. No other
   collection (`Default_N`) is ever checked, so they cannot raise an alarm.
   Unlock and Test act on the collection behind `default` [call].

A check never blocks the UI thread and has a timeout; a timeout counts as
unhealthy with its own reason. To avoid flapping, two failed checks in a row
are needed to raise the alarm [call], one good check clears it.

**Tray.** StatusNotifierItem registered with `org.kde.StatusNotifierWatcher`
(the panel runs it). The panel only hosts items today and nothing in the repo
exports one, so this is new. The panel host already reads `Menu`
(`com.canonical.dbusmenu`: layout and click events, `crates/panel/src/sni.rs`),
so the item exports a dbusmenu and the panel needs no change. Icon: a themed lock icon (`dialog-password` or
similar) [call]. The menu has a disabled first line with the reason. The entries depend on
the reason [call]: Unlock only when the collection is locked; Restart
gnome-keyring when the service is missing, locked or not answering (gnome
only); Test when the check is good or only locked; Check now always. The Test
dialog and the actions show their own result. If no
watcher is on the bus, log it and keep the desktop notification as the signal.

**Notification.** One `org.freedesktop.Notifications.Notify` when the state
goes from healthy to unhealthy (and again if the reason changes), replacing
the previous one; text: the reason and "Click the icon in the tray".

**Unlock (generic).** Secret Service `Unlock` on the collection; the provider
shows its own prompt.

**Unlock with password (gnome-keyring only).** A GTK window, run as a
short-lived child process of the same binary (`opusdm-keyring-watch
--ask-password`) so the watcher stays headless and the password never lives
in the long-running process [call]; it works on X11 and Wayland, and
`opusdm-keyring-watch --unlock` opens it directly, which is also the way out
when there is no tray [call]. The window (title, password
entry, Unlock and Cancel). The password goes to `gnome-keyring-daemon
--unlock` on its stdin only, in memory that is zeroed after use; wrong
password: the window says so and stays open. If no daemon runs, start it
first (`gnome-keyring-daemon --start --components=secrets`) and then unlock
[call]. Exact flags are for the builder to verify on this machine.

**Restart gnome-keyring (gnome-keyring only).** On this machine gnome-keyring
is a systemd user unit with socket activation (`gnome-keyring-daemon.service`
and `.socket`, checked 2026-11-03), so the restart is `systemctl --user
restart gnome-keyring-daemon.service`, not `--replace`, which would fight
systemd. A restarted daemon has no login password and comes back locked, so
the sequence is: ask the password in the same window, restart the unit, unlock
with the password, run the check. When the unit is not present, fall back to
`--replace` [call]. Also stop `ksecretd`, `kwalletd5`, `kwalletd6` first, like
`opusdm-session` does. Confirm before restarting [call], because apps lose
their open Secret Service session.

**Test (generic).** Create an item with a random value in the default
collection with a clearly named label, read it back, compare, delete it, show
the result (ok or the exact error) in a small dialog or notification. Never
leave the item behind, even on error.

**Provider detection.** `GetNameOwner` + `GetConnectionUnixProcessID` on
`org.freedesktop.secrets`, then `/proc/<pid>/comm`; `gnome-keyring-d` selects
the gnome module [call].

**Libraries.** `zbus` and `secret-service` are already in the workspace
(`crates/network/src/keystore.rs`). Whether to add `ksni` (SNI plus dbusmenu)
or write the SNI object by hand with `zbus` is left to the builder; adding a
dependency needs a one line justification in `CHANGES.md`.

## Constraints

- Project rules (`CLAUDE.md`): behaviour first, no silent failures, one risky
  change per commit, a regression test where possible, run the real thing.
- `pnpm`, English in code and comments, no em dash, `CHANGES.md` updated with
  what was NOT verified.
- Packaging: `build-release.sh`, `build-deb.sh` and `live-update.sh` must ship
  and stop the new binary like the others.

## Left to the builder

Crate and module layout; exact intervals; SNI library choice; the Test
dialog form; how to make the check testable (a trait over the Secret Service
with a fake, like `KeyStore`/`FakeKeyStore`); the icon name.

## How it must be verified (not assumed)

Unit tests with a fake Secret Service for the state machine (grace, two
failures, recovery, reason change). By hand, in the sandbox (`run-x11.sh`) and
then the live session: lock the keyring (D-Bus `Lock`), see icon and
notification; unlock with the password window (right and wrong password);
stop the daemon, see the reason change; Restart; Test; kill the watcher and
see the hub restart it; hub restart does not leave two watchers. Whatever is
not exercised is written as not verified in `CHANGES.md` and the final message.
