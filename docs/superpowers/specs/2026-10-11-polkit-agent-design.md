# opusdm-polkit: polkit authentication agent

## Problem

Programs that need administrator rights through polkit (`pkexec`, Synaptic,
GParted, disk and network tools) fail silently in an OpusDM session: polkitd
finds no authentication agent registered for the session, so no password
dialog appears and the request is refused.

## Solution

A new binary, `opusdm-polkit`, in its own crate (`crates/polkit`). The hub
starts it once at login, next to the startup apps.

### Parts

| Module | Role |
|---|---|
| `agent.rs` | D-Bus side (system bus): serves `org.freedesktop.PolicyKit1.AuthenticationAgent` and registers it with `RegisterAuthenticationAgent` for the login session (`XDG_SESSION_ID`, logind as fallback). Only polkitd may call it. `BeginAuthentication` waits for the dialog's outcome; `CancelAuthentication` closes the dialog. |
| `identity.rs` | Turns the identities polkitd offers (`unix-user`, `unix-group`) into user names (group members expanded), the current user first. |
| `helper.rs` | The conversation with the setuid `polkit-agent-helper-1`: `helper USER`, cookie on stdin, then PAM lines (`PAM_PROMPT_ECHO_OFF`, `PAM_PROMPT_ECHO_ON`, `PAM_ERROR_MSG`, `PAM_TEXT_INFO`, `SUCCESS`, `FAILURE`), text escaped with `g_strescape`. The helper itself tells polkitd the result. |
| `dialog.rs` | One GTK window per request: icon, message, user choice (when several admins), the PAM prompt with its entry, an error line, the action details, Cancel / Authenticate. A failed password starts a new helper run and keeps the dialog open. |
| `main.rs` | GTK application `org.opusdm.PolkitAgent`: one instance per session (a second launch exits at once), kept alive with no windows. |

### Threads

zbus runs the D-Bus object on its own executor; every method call is its own
task. Requests go to the GTK main loop through an `async_channel`, each with
a reply channel the dialog answers on. Helper output is read on a thread and
sent to the GTK loop the same way.

### Placement

Wayland: overlay layer surface, centered, exclusive keyboard. X11: centered on
the primary monitor, kept above.

### Out of scope

Keyboard grab on X11, fingerprint-only PAM stacks with no prompt (they work,
the dialog just waits), a Settings switch to turn the agent off.
