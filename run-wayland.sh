#!/usr/bin/env bash
#
# Launch a nested labwc (or Wayfire, --wayfire) and run opusdm-hub inside it.
# The Wayland sibling of run-x11.sh: a sandbox for testing OpusDM on Wayland
# without touching the real session. labwc opens as a window of the current
# desktop, X11 or Wayland alike.
#
# Usage:
#   ./run-wayland.sh [--wayfire] [--debug] [--grid-debug] [--headless] [resolution]
#
# Examples:
#   ./run-wayland.sh              # 1920x1080
#   ./run-wayland.sh 1280x800
#   ./run-wayland.sh --debug      # debug log in /ramdisk/opusdm.log
#   ./run-wayland.sh --headless   # no window: for scripted tests (grim, wlrctl)
#   ./run-wayland.sh --wayfire    # Wayfire with wayfire.ini instead of labwc
#
# Always on a private D-Bus session bus (the script runs itself again under
# dbus-run-session): no need to start it under one.

set -euo pipefail

# A private D-Bus session bus, always: on the live bus the sandbox hub would
# take org.freedesktop.FileManager1, the notification and SNI names from the
# live hub. The script runs itself again under dbus-run-session;
# OPUSDM_SANDBOX_BUS (the marker) names a file this outer run created.
# dbus-run-session dies on TERM without passing it on, so the outer run
# stays: it passes TERM / INT / HUP to the inner run (the group of dbus-run-session before the inner run has started) and waits for it
# (labwc / Wayfire run in the foreground of the inner run, in that group, as
# a Ctrl+C in the terminal reached them before) and exits with the status
# dbus-run-session hands back.
if [ -z "${OPUSDM_SANDBOX_BUS:-}" ] || [ ! -f "$OPUSDM_SANDBOX_BUS" ]; then
    command -v dbus-run-session >/dev/null 2>&1 || { echo "dbus-run-session not found (package: dbus-daemon or dbus-bin): the sandbox needs a private D-Bus session"; exit 1; }
    BUS_MARKER="$(mktemp /tmp/opusdm-wayland-bus-XXXXXX)"
    trap 'rm -f "$BUS_MARKER"' EXIT
    # Own process group: a Ctrl+C reaches only this run, which passes it on.
    # The trap comes first (an empty BUS_PID is skipped); HUP too: closing the
    # terminal reaches only this run, as the sandbox is in a group of its own.
    # Passes a signal on as TERM to the inner run (its pid is in the marker
    # file); before that, to the whole group of dbus-run-session (nothing
    # started yet). The inner run's trap does the cleanup; this run waits for
    # it below. TERM to dbus-run-session itself would return it early.
    _forward_term() {
        local pid
        pid="$(cat "$BUS_MARKER" 2>/dev/null || true)"
        if [ -n "$pid" ]; then
            kill -TERM "$pid" 2>/dev/null || true
        elif [ -n "$BUS_PID" ]; then
            kill -TERM -- "-$BUS_PID" 2>/dev/null || true
        fi
    }
    BUS_PID=""
    trap _forward_term TERM INT HUP
    set -m
    OPUSDM_SANDBOX_BUS="$BUS_MARKER" dbus-run-session -- bash "$0" "$@" &
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
# The outer run passes its signals on to this pid
echo "$$" >"$OPUSDM_SANDBOX_BUS"
unset OPUSDM_SANDBOX_BUS

DEBUG=0
GRID_DEBUG=0
HEADLESS=0
WAYFIRE=0
ARGS=()
for a in "$@"; do
    case "$a" in
        --debug) DEBUG=1 ;;
        --grid-debug) GRID_DEBUG=1 ;;
        --headless) HEADLESS=1 ;;
        --wayfire) WAYFIRE=1 ;;
        *) ARGS+=("$a") ;;
    esac
done
set -- ${ARGS[@]+"${ARGS[@]}"}

# Configuration (overridable via args or environment)
RESOLUTION="${1:-${WAYLAND_RES:-1920x1080}}"
LABWC_CONFIG="${WAYLAND_LABWC_CONFIG:-$(pwd)/labwc-rc.xml}"   # rc.xml for labwc
WAYFIRE_CONFIG="${WAYLAND_WAYFIRE_CONFIG:-$(pwd)/wayfire.ini}" # wayfire.ini for Wayfire

# Exit conditions up front
if [ "$WAYFIRE" = "1" ]; then
    command -v wayfire >/dev/null 2>&1 || { echo "wayfire not found (package: wayfire)"; exit 1; }
    [ -f "$WAYFIRE_CONFIG" ] || { echo "Wayfire config not found: $WAYFIRE_CONFIG"; exit 1; }
else
    command -v labwc >/dev/null 2>&1 || { echo "labwc not found (package: labwc)"; exit 1; }
    [ -f "$LABWC_CONFIG" ] || { echo "labwc config not found: $LABWC_CONFIG"; exit 1; }
fi
command -v wlr-randr >/dev/null 2>&1 || { echo "wlr-randr not found (package: wlr-randr)"; exit 1; }
DEBUG_LOG="/ramdisk/opusdm.log"   # hub output with --debug
if [ "$DEBUG" = "1" ] && [ ! -d "$(dirname "$DEBUG_LOG")" ]; then
    echo "Debug log folder not found: $(dirname "$DEBUG_LOG")"
    exit 1
fi

# Variables of a running OpusDM session that would point the sandbox at the
# live one: the base picom config (the hub would rewrite the user's
# picom.conf and signal the live picom) and the IPC socket overrides.
# OPUSDM_LABWC_RC is set again below for the sandbox's own rc.xml.
LIVE_SESSION_VARS=(OPUSDM_PICOM_BASE OPUSDM_HUB_SOCK OPUSDM_DOCK_SOCK OPUSDM_APPLETS_SOCK OPUSDM_LABWC_RC)
# Nothing started from here on (labwc / Wayfire, hub, tools) inherits them.
unset "${LIVE_SESSION_VARS[@]}"

# Build the workspace before entering the sandbox
echo "Building opusdm (release)"
cargo build --release --workspace

HUB_BIN="$(pwd)/target/release/opusdm-hub"
[ -x "$HUB_BIN" ] || { echo "opusdm-hub not built at $HUB_BIN"; exit 1; }

# A config directory of its own: rc.xml plus an autostart that sizes the
# nested output and starts the hub.
CONFIG_DIR=""
HUB_RUNTIME_DIR=""
# One EXIT trap of this (inner) run removes both temp dirs, only the ones
# created below (empty variables are skipped). It is set before the first
# mktemp so a failing second one leaves nothing behind.
trap '[ -n "$CONFIG_DIR" ] && rm -rf "$CONFIG_DIR"; [ -n "$HUB_RUNTIME_DIR" ] && rm -rf "$HUB_RUNTIME_DIR"; true' EXIT
CONFIG_DIR="$(mktemp -d /tmp/opusdm-labwc-XXXXXX)"
# The hub gets its own runtime dir so a stale socket never blocks the
# sandbox; the compositor keeps the real one (its socket lives there).
HUB_RUNTIME_DIR="$(mktemp -d /tmp/opusdm-wayland-XXXXXX)"

cp "$LABWC_CONFIG" "$CONFIG_DIR/rc.xml"

HUB_OUT="/dev/stderr"
[ "$DEBUG" = "1" ] && HUB_OUT="$DEBUG_LOG" && echo "Debug log: $HUB_OUT"

{
    # labwc / Wayfire start this script in a session of its own, so the hub and
    # its tools are not in the compositor's process group: record its group
    # for the cleanup after the compositor exits.
    echo "ps -o pgid= -p \$\$ | tr -d ' ' >\"$CONFIG_DIR/autostart.pgid\""
    echo '# Size the nested output (its name depends on the host: WL-1, X11-1...)'
    echo 'OUTPUT="$(wlr-randr | awk '"'"'NR == 1 { print $1 }'"'"')"'
    echo "wlr-randr --output \"\$OUTPUT\" --custom-mode $RESOLUTION || true"
    echo '# WAYLAND_DISPLAY must be a full path once XDG_RUNTIME_DIR changes.'
    echo 'export WAYLAND_DISPLAY="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"'
    echo "export XDG_RUNTIME_DIR=\"$HUB_RUNTIME_DIR\""
    echo 'unset GDK_BACKEND'
    # The hub writes its global hotkeys into the sandbox's own rc.xml.
    echo "export OPUSDM_LABWC_RC=\"$CONFIG_DIR/rc.xml\""
    # Hotkeys such as exec:opusdm-run find the OpusDM binaries by name
    echo "export PATH=\"$(pwd)/target/release:\$PATH\""
    [ "$GRID_DEBUG" = "1" ] && echo 'export GRID_DEBUG=1'
    [ "$DEBUG" = "1" ] && echo 'export OPUSDM_DEBUG=1'
    # A sandbox never starts the user's Startup Apps a second time (as run-x11.sh)
    echo "\"$HUB_BIN\" --no-startup-apps >\"$HUB_OUT\" 2>&1 &"
} >"$CONFIG_DIR/autostart"

# Runs the compositor given as arguments in the background, in a process
# group of its own (`set -m`), so a TERM / INT to this script is passed on to
# it and, once it is gone, to the whole group: tools that outlive it
# (opusdm-keyring-watch is not a Wayland client and does not quit with it)
# do not stay behind. Not exec: this shell runs the EXIT trap that removes
# the temp dirs. Exit status: the compositor's.
_run_compositor() {
    local pid=""
    # The trap is set before the job starts; an empty pid is skipped.
    trap '[ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null; true' TERM INT HUP
    set -m
    "$@" &
    pid=$!
    set +m
    local status=0
    wait "$pid" || status=$?
    # A trapped signal ends `wait` early: wait again until the compositor is gone.
    while kill -0 "$pid" 2>/dev/null; do
        status=0
        wait "$pid" || status=$?
    done
    kill -TERM -- "-$pid" 2>/dev/null || true
    # The group of the autostart script (hub and tools), recorded by it.
    local hub_pgid=""
    hub_pgid="$(cat "$CONFIG_DIR/autostart.pgid" 2>/dev/null || true)"
    # Only a real group of its own: not 0 / 1 (every process) nor this script's
    local own_pgid
    own_pgid="$(ps -o pgid= -p $$ | tr -d ' ')"
    case "$hub_pgid" in
        ''|*[!0-9]*) [ -n "$hub_pgid" ] && echo "run-wayland.sh: bad autostart pgid '$hub_pgid', not ended" >&2 ;;
        *)
            if [ "$hub_pgid" -gt 1 ] && [ "$hub_pgid" != "$own_pgid" ]; then
                kill -TERM -- "-$hub_pgid" 2>/dev/null || true
            else
                echo "run-wayland.sh: unsafe autostart pgid '$hub_pgid', not ended" >&2
            fi
            ;;
    esac
    # No signal may reach the local pid above once this function is gone
    trap - TERM INT HUP
    return "$status"
}

if [ "$WAYFIRE" = "1" ]; then
    # wayfire.ini starts $OPUSDM_HUB: here the same autostart script labwc runs.
    export OPUSDM_HUB="sh $CONFIG_DIR/autostart"
    echo "Starting Wayfire ($RESOLUTION), config: $WAYFIRE_CONFIG"
    STATUS=0
    if [ "$HEADLESS" = "1" ]; then
        _run_compositor env WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WAYLAND_DISPLAY= wayfire -c "$WAYFIRE_CONFIG" || STATUS=$?
    else
        _run_compositor wayfire -c "$WAYFIRE_CONFIG" || STATUS=$?
    fi
    exit "$STATUS"
fi

echo "Starting labwc ($RESOLUTION), config: $LABWC_CONFIG"
STATUS=0
if [ "$HEADLESS" = "1" ]; then
    # Prints the socket to use, e.g. WAYLAND_DISPLAY=wayland-1 grim shot.png
    _run_compositor env WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WAYLAND_DISPLAY= labwc --config-dir "$CONFIG_DIR" || STATUS=$?
else
    _run_compositor labwc --config-dir "$CONFIG_DIR" || STATUS=$?
fi
exit "$STATUS"
