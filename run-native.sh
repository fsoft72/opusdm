#!/usr/bin/env bash
#
# Launch OpusDM's own Wayland compositor (opusdm-compositor) nested in a
# window of the current desktop and run opusdm-hub inside it. The sibling of
# run-wayland.sh (nested labwc) and run-x11.sh: a sandbox for testing the
# native compositor without touching the real session.
#
# Stage 1 of the compositor: windows, popups, layer shell, move / resize,
# maximize, fullscreen. No tasklist, pager, global hotkeys or minimize yet.
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
        else
            kill -TERM -- "-$BUS_PID" 2>/dev/null || true
        fi
    }
    # Own process group: a Ctrl+C reaches only this run, which passes it on.
    set -m
    OPUSDM_SANDBOX_BUS="$BUS_PID_FILE" dbus-run-session -- bash "$0" "$@" &
    BUS_PID=$!
    set +m
    trap _forward_term TERM INT
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

# A config directory of its own holding the autostart script the compositor
# runs once its socket is up.
CONFIG_DIR="$(mktemp -d /tmp/opusdm-native-XXXXXX)"
# The hub gets its own runtime dir so a stale socket never blocks the
# sandbox; the compositor keeps the real one (its socket lives there).
HUB_RUNTIME_DIR="$(mktemp -d /tmp/opusdm-native-run-XXXXXX)"
trap 'rm -rf "$CONFIG_DIR" "$HUB_RUNTIME_DIR"' EXIT

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
set -m
"$COMPOSITOR_BIN" --nested --size "$RESOLUTION" --startup "sh $CONFIG_DIR/autostart" &
COMPOSITOR_PID=$!
set +m
trap 'kill -TERM "$COMPOSITOR_PID" 2>/dev/null || true' TERM INT
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
