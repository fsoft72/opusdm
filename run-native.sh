#!/usr/bin/env bash
#
# Launch OpusDM's own Wayland compositor (opusdm-compositor) nested in a
# window of the current desktop and run opusdm-hub inside it. The sibling of
# run-wayland.sh (nested labwc) and run-x11.sh: a sandbox for testing the
# native compositor without touching the real session.
#
# Stage 3 of the compositor: windows, popups, layer shell, move / resize,
# maximize, fullscreen, desktops, minimize, tasklist, pager, global hotkeys,
# window menu, the window manager built-ins (switcher, close, desktops,
# focus...), snapping, opacity, server side title bars, screen copy
# (opusdm-shot), clipboard and drag and drop. No XWayland or DRM session yet.
#
# The sandbox never writes the live configuration: XDG_CONFIG_HOME is a temp
# folder whose `opusdm` is a COPY of ~/.config/opusdm (every other entry of
# ~/.config is a symlink), removed at exit. In the copy every global hotkey
# gets `Super+` (the host desktop keeps its own combos: Ctrl+Shift+space in
# the sandbox is Ctrl+Shift+Super+space). The built-ins of the compositor
# that the host Openbox grabs get Ctrl+Super+<key> (Alt+Tab is Ctrl+Super+Tab,
# Alt+F4 is Ctrl+Super+F4...) and the window menu is Super+Alt+space
# (OPUSDM_COMPOSITOR_CONFIG, a temp compositor.json). Both tables used are
# printed at start.
#
# Usage:
#   ./run-native.sh [--debug] [resolution]
#
# Examples:
#   ./run-native.sh               # 1920x1080
#   ./run-native.sh 1280x800
#   ./run-native.sh --debug       # debug log in /ramdisk/opusdm.log
#   NATIVE_LOG=/tmp/hub.log ./run-native.sh --debug   # debug log elsewhere
#
# Always on a private D-Bus session bus (the script runs itself again under
# dbus-run-session): no need to start it under one.

set -euo pipefail

# A private D-Bus session bus, always: on the live bus the sandbox hub would
# take org.freedesktop.FileManager1, the notification and SNI names from the
# live hub. The script runs itself again under dbus-run-session.
# OPUSDM_SANDBOX_BUS (the marker) names a file this outer run created and
# the inner run writes its pid to. dbus-run-session dies on TERM without
# passing it on, so the outer run stays: it passes TERM / INT to the inner
# run (whose trap ends the compositor's process group) and exits with the
# status dbus-run-session hands back (the inner run's).
if [ -z "${OPUSDM_SANDBOX_BUS:-}" ] || [ ! -f "$OPUSDM_SANDBOX_BUS" ]; then
    command -v dbus-run-session >/dev/null 2>&1 || { echo "dbus-run-session not found (package: dbus-daemon or dbus-bin): the sandbox needs a private D-Bus session"; exit 1; }
    BUS_PID_FILE="$(mktemp /tmp/opusdm-native-bus-XXXXXX)"
    trap 'rm -f "$BUS_PID_FILE"' EXIT
    # Passes a TERM to the inner run; before it has written its pid, to
    # the whole process group of dbus-run-session (nothing started yet).
    _forward_term() {
        local pid
        pid="$(cat "$BUS_PID_FILE" 2>/dev/null || true)"
        if [ -n "$pid" ]; then
            kill -TERM "$pid" 2>/dev/null || true
        elif [ -n "$BUS_PID" ]; then
            kill -TERM -- "-$BUS_PID" 2>/dev/null || true
        fi
    }
    # Own process group: a Ctrl+C reaches only this run, which passes it on.
    # The trap comes first (the forwarder copes with an empty BUS_PID); HUP
    # too: closing the terminal reaches only this run, the sandbox has a
    # group of its own.
    BUS_PID=""
    trap _forward_term TERM INT HUP
    set -m
    OPUSDM_SANDBOX_BUS="$BUS_PID_FILE" dbus-run-session -- bash "$0" "$@" &
    BUS_PID=$!
    set +m
    STATUS=0
    wait "$BUS_PID" || STATUS=$?
    # A trapped signal ends `wait` early: wait again until the inner run is gone.
    while kill -0 "$BUS_PID" 2>/dev/null; do
        STATUS=0
        wait "$BUS_PID" || STATUS=$?
    done
    exit "$STATUS"
fi
echo "$$" >"$OPUSDM_SANDBOX_BUS"
unset OPUSDM_SANDBOX_BUS

DEBUG=0
ARGS=()
for a in "$@"; do
    case "$a" in
        --debug) DEBUG=1 ;;
        *) ARGS+=("$a") ;;
    esac
done
set -- ${ARGS[@]+"${ARGS[@]}"}

# Configuration (overridable via args or environment)
RESOLUTION="${1:-${NATIVE_RES:-1920x1080}}"
DEBUG_LOG="${NATIVE_LOG:-/ramdisk/opusdm.log}"   # hub output with --debug

# Variables of a running OpusDM session that would point the sandbox at the
# live one: the base picom config (the hub would rewrite the user's
# picom.conf and signal the live picom) and the IPC socket overrides.
# Keep in sync with `child_env` in crates/compositor/src/child.rs (a unit
# test there compares the two lists).
LIVE_SESSION_VARS=(OPUSDM_PICOM_BASE OPUSDM_HUB_SOCK OPUSDM_DOCK_SOCK OPUSDM_APPLETS_SOCK OPUSDM_LABWC_RC)

# Exit conditions up front
command -v cargo >/dev/null 2>&1 || { echo "cargo not found (install Rust: https://rustup.rs)"; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq not found (package: jq): the sandbox needs it to write its own hotkey table"; exit 1; }
[[ "$RESOLUTION" =~ ^[0-9]+x[0-9]+$ ]] || { echo "Bad resolution: $RESOLUTION (expected WxH, e.g. 1280x800)"; exit 1; }
if [ "$DEBUG" = "1" ] && [ ! -d "$(dirname "$DEBUG_LOG")" ]; then
    echo "Debug log folder not found: $(dirname "$DEBUG_LOG") (set NATIVE_LOG to a file in an existing folder)"
    exit 1
fi

# Nothing started from here on (compositor, hub, tools) inherits them.
unset "${LIVE_SESSION_VARS[@]}"

# Build the workspace before entering the sandbox
echo "Building opusdm (release)"
cargo build --release --workspace

BIN_DIR="$(pwd)/target/release"
HUB_BIN="$BIN_DIR/opusdm-hub"
COMPOSITOR_BIN="$BIN_DIR/opusdm-compositor"
[ -x "$HUB_BIN" ] || { echo "opusdm-hub not built at $HUB_BIN"; exit 1; }
[ -x "$COMPOSITOR_BIN" ] || { echo "opusdm-compositor not built at $COMPOSITOR_BIN"; exit 1; }

# Where the live configuration is read from (never written).
LIVE_CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"

# Built-in bindings of the sandbox compositor.json, as "name key" pairs: the
# host Openbox grabs every default of them (Alt+Tab, Alt+F4, Ctrl+Alt+Left,
# Super+F1, Super+d, Alt+space...), so each one gets Ctrl+Super (plus Shift or
# Alt where two would clash). Openbox matches the exact modifiers, so these
# combos reach the nested window. `quit` (Ctrl+Alt+BackSpace) is not grabbed
# by Openbox and keeps its default. Keep in sync with docs/WAYLAND-PORT.md.
SANDBOX_BUILTINS=(
    "switcher_next Ctrl+Super+Tab"
    "switcher_prev Ctrl+Shift+Super+Tab"
    "switcher_all Ctrl+Alt+Super+Tab"
    "close Ctrl+Super+F4"
    "desktop_left Ctrl+Super+Left"
    "desktop_right Ctrl+Super+Right"
    "desktop_1 Ctrl+Shift+Super+F1"
    "desktop_2 Ctrl+Shift+Super+F2"
    "desktop_3 Ctrl+Shift+Super+F3"
    "desktop_4 Ctrl+Shift+Super+F4"
    "send_left Ctrl+Shift+Super+Left"
    "send_right Ctrl+Shift+Super+Right"
    "show_desktop Ctrl+Super+d"
    "lower Ctrl+Super+Escape"
    "focus_left Ctrl+Alt+Super+Left"
    "focus_right Ctrl+Alt+Super+Right"
    "focus_up Ctrl+Alt+Super+Up"
    "focus_down Ctrl+Alt+Super+Down"
    "window_menu Super+Alt+space"
)

# Temp folders, all removed by the EXIT trap. The trap is set before the
# first mktemp and copes with empty names.
CONFIG_DIR=""
HUB_RUNTIME_DIR=""
PRIVATE_CONFIG_HOME=""
trap 'rm -rf ${CONFIG_DIR:+"$CONFIG_DIR"} ${HUB_RUNTIME_DIR:+"$HUB_RUNTIME_DIR"} ${PRIVATE_CONFIG_HOME:+"$PRIVATE_CONFIG_HOME"}' EXIT

# Fills the folder $1 as a private XDG_CONFIG_HOME: `opusdm` is a copy of
# the live one (an empty folder when there is none), every other entry of
# the live config home is a symlink to it. The copy follows symlinks (a
# symlinked opusdm folder, symlinked files inside it): a link left in the
# copy would let the sandbox write the live file, so any link left (a
# dangling one cannot be copied either) ends the script with a message.
_make_private_config() {
    local dest="$1" entry name links
    if [ -d "$LIVE_CONFIG_HOME/opusdm" ]; then
        cp -RL --preserve=mode,timestamps "$LIVE_CONFIG_HOME/opusdm" "$dest/opusdm" \
            || { echo "Cannot copy $LIVE_CONFIG_HOME/opusdm (a dangling symlink in it?): the sandbox needs a full copy" >&2; exit 1; }
        [ ! -L "$dest/opusdm" ] || { echo "The copy $dest/opusdm is a symlink: the sandbox would write the live configuration" >&2; exit 1; }
        links="$(find "$dest/opusdm" -type l)"
        [ -z "$links" ] || { echo "The copy of the configuration still holds symlinks, the sandbox would write through them:" >&2; echo "$links" >&2; exit 1; }
    else
        echo "No $LIVE_CONFIG_HOME/opusdm: the sandbox hub starts from an empty configuration" >&2
        mkdir "$dest/opusdm"
    fi
    for entry in "$LIVE_CONFIG_HOME"/* "$LIVE_CONFIG_HOME"/.[!.]*; do
        [ -e "$entry" ] || [ -L "$entry" ] || continue
        name="$(basename "$entry")"
        [ "$name" = "opusdm" ] && continue
        ln -s "$entry" "$dest/$name"
    done
}

# Rewrites the hotkeys.json of the copy in $1 into the sandbox table: every
# global entry without Super gets `Super+` before its key name (the canonical
# order Ctrl, Alt, Shift, Super, key). Entries already using Super are kept
# and listed on stderr: the host may take them. Prints the table used.
_make_sandbox_hotkeys() {
    local file="$1/hotkeys.json" taken
    if [ ! -f "$file" ]; then
        echo "No hotkeys.json in the configuration: the hub writes its defaults (the real combos, not Super+)" >&2
        return 0
    fi
    taken="$(jq -r '.[] | select(.global == true) | select(.key | split("+") | .[:-1] | map(ascii_downcase) | any(. == "super")) | "  \(.key)  \(.function)"' "$file")" \
        || { echo "jq failed on $file (not valid JSON or not an array of hotkeys)" >&2; return 1; }
    jq 'map(if .global == true and ((.key | split("+") | .[:-1] | map(ascii_downcase) | any(. == "super")) | not)
            then .key |= (split("+") | .[:-1] + ["Super"] + .[-1:] | join("+"))
            else . end)' "$file" >"$file.new" \
        || { echo "jq failed writing the sandbox hotkey table from $file (not valid JSON or not an array of hotkeys)" >&2; return 1; }
    mv "$file.new" "$file"
    if [ -n "$taken" ]; then
        echo "These global hotkeys already use Super and may be taken by the host:" >&2
        echo "$taken" >&2
    fi
    echo "Sandbox global hotkeys (Super+ added; real combos are not bound here):"
    jq -r '.[] | select(.global == true) | "  \(.key)  \(.function)"' "$file"
}

# Writes the sandbox compositor.json to $1 from SANDBOX_BUILTINS and prints
# the table used.
_make_sandbox_compositor_config() {
    local file="$1" entry
    printf '%s\n' "${SANDBOX_BUILTINS[@]}" \
        | jq -R -s 'split("\n") | map(select(length > 0) | split(" ") | {(.[0]): .[1]}) | add' >"$file" \
        || { echo "jq failed writing the sandbox compositor.json $file" >&2; return 1; }
    echo "Sandbox built-in bindings (compositor.json; the host Openbox grabs the defaults):"
    for entry in "${SANDBOX_BUILTINS[@]}"; do
        printf '  %-24s %s\n' "${entry#* }" "${entry%% *}"
    done
    echo "  Ctrl+Alt+BackSpace       quit (default, not grabbed by Openbox)"
}

# Warns on stderr for every sandbox global hotkey (in the hotkeys.json $1)
# whose key equals a built-in of the compositor.json $2: the built-in wins,
# so that hotkey does nothing in the sandbox. Modifiers are compared in any
# order and case. Nothing to compare without a hotkeys.json.
_warn_hotkey_clashes() {
    local hotkeys="$1" builtins="$2" clashes
    [ -f "$hotkeys" ] || return 0
    clashes="$(jq -r --slurpfile b "$builtins" '
        def norm: split("+") | (.[:-1] | map(ascii_downcase) | sort) + .[-1:] | join("+");
        ($b[0] | to_entries | map({key: (.value | norm), value: .key}) | from_entries) as $taken
        | .[] | select(.global == true) | select($taken[.key | norm] != null)
        | "  \(.key)  \(.function) (the built-in \($taken[.key | norm]) wins)"' "$hotkeys")" \
        || { echo "jq failed comparing $hotkeys with $builtins" >&2; return 1; }
    [ -z "$clashes" ] && return 0
    echo "These sandbox global hotkeys use a key of a sandbox built-in and do nothing here:" >&2
    echo "$clashes" >&2
}

# A config directory of its own holding the autostart script the compositor
# runs once its socket is up, and the sandbox compositor.json.
CONFIG_DIR="$(mktemp -d /tmp/opusdm-native-XXXXXX)"
# The hub gets its own runtime dir so a stale socket never blocks the
# sandbox; the compositor keeps the real one (its socket lives there).
HUB_RUNTIME_DIR="$(mktemp -d /tmp/opusdm-native-run-XXXXXX)"
# The private configuration. The compositor (it only reads: desktop.json for
# the number of desktops, the theme) and the hub (with every tool it starts)
# both get it, so nothing in the sandbox writes the live ~/.config/opusdm.
PRIVATE_CONFIG_HOME="$(mktemp -d /tmp/opusdm-cfg-XXXXXX)"
_make_private_config "$PRIVATE_CONFIG_HOME"
_make_sandbox_hotkeys "$PRIVATE_CONFIG_HOME/opusdm"
export XDG_CONFIG_HOME="$PRIVATE_CONFIG_HOME"
echo "Configuration: a copy in $PRIVATE_CONFIG_HOME/opusdm (removed at exit)"

_make_sandbox_compositor_config "$CONFIG_DIR/compositor.json"
_warn_hotkey_clashes "$PRIVATE_CONFIG_HOME/opusdm/hotkeys.json" "$CONFIG_DIR/compositor.json"
export OPUSDM_COMPOSITOR_CONFIG="$CONFIG_DIR/compositor.json"

HUB_OUT="/dev/stderr"
[ "$DEBUG" = "1" ] && HUB_OUT="$DEBUG_LOG" && echo "Debug log: $HUB_OUT"

{
    echo '# WAYLAND_DISPLAY must be a full path once XDG_RUNTIME_DIR changes.'
    echo 'export WAYLAND_DISPLAY="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"'
    echo "export XDG_RUNTIME_DIR=\"$HUB_RUNTIME_DIR\""
    echo 'unset GDK_BACKEND'
    # Tools started by the hub (opusdm-lister, opusdm-run...) are found by name
    echo "export PATH=\"$BIN_DIR:\$PATH\""
    [ "$DEBUG" = "1" ] && echo 'export OPUSDM_DEBUG=1'
    # A sandbox never starts the user's Startup Apps a second time (as run-x11.sh)
    echo "\"$HUB_BIN\" --no-startup-apps >\"$HUB_OUT\" 2>&1 &"
} >"$CONFIG_DIR/autostart"

[ "$DEBUG" = "1" ] && export OPUSDM_DEBUG=1

echo "Starting opusdm-compositor ($RESOLUTION)"
# Not exec: the shell stays to run the EXIT trap that removes the temp dirs.
# The compositor runs in the background so a TERM / INT to this script is
# passed on to it (it quits, and its clients, the hub first, lose their
# display) before the trap cleans up; its exit code is this script's.
# `set -m` gives it a process group of its own, which the hub and every
# tool it starts inherit.
COMPOSITOR_PID=""
trap '[ -n "$COMPOSITOR_PID" ] && kill -TERM "$COMPOSITOR_PID" 2>/dev/null; true' TERM INT HUP
set -m
"$COMPOSITOR_BIN" --nested --size "$RESOLUTION" --startup "sh $CONFIG_DIR/autostart" &
COMPOSITOR_PID=$!
set +m
STATUS=0
wait "$COMPOSITOR_PID" || STATUS=$?
# A trapped signal ends `wait` early: wait again until the compositor is gone.
while kill -0 "$COMPOSITOR_PID" 2>/dev/null; do
    STATUS=0
    wait "$COMPOSITOR_PID" || STATUS=$?
done
# Tools that outlive the hub (opusdm-keyring-watch is not a Wayland client
# and does not quit with it) are still in the compositor's process group.
kill -TERM -- "-$COMPOSITOR_PID" 2>/dev/null || true
exit "$STATUS"
