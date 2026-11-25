#!/usr/bin/env bash
#
# Launch a nested X server (Xephyr) on a chosen display and run opusdm-hub
# inside it. The hub spawns the first Lister window itself. Useful as a
# sandbox for testing OpusDM without touching the real session.
#
# Usage:
#   ./xephyr-sandbox.sh [--debug] [--grid-debug] [--config-dir DIR] [display] [resolution]
#
# Examples:
#   ./xephyr-sandbox.sh              # :1 at 1920x1080
#   ./xephyr-sandbox.sh :2 1920x1080
#   ./xephyr-sandbox.sh --debug      # lister debug log in /ramdisk/opusdm.log
#   ./xephyr-sandbox.sh --grid-debug # red box around every icon-view item
#   ./xephyr-sandbox.sh --config-dir ~/test-config
#                                    # use DIR as the OpusDM configuration
#                                    # (settings.json, hotkeys.json, ...) instead
#                                    # of ~/.config/opusdm; DIR must exist. The
#                                    # sandbox reads and writes only DIR (also
#                                    # picom.conf); XDG_CONFIG_HOME points to a
#                                    # private folder, so ~/.config/mimeapps.list
#                                    # is not touched either. Same as XEPHYR_CONFIG_DIR.
#
# Transparency: picom runs on the sandbox's picom.conf as it is, and the
# sandbox hub gets no OPUSDM_PICOM_BASE, so changing the opacity in Settings
# does not apply live in the sandbox (the price of never touching the live
# picom or ~/.config/opusdm/picom.conf).

set -euo pipefail

# A private D-Bus session bus, always: on the live bus the sandbox hub would
# take org.freedesktop.FileManager1, the notification and SNI names from the
# live hub. The script runs itself again under dbus-run-session;
# OPUSDM_SANDBOX_BUS (the marker) names a file this outer run created.
# dbus-run-session dies on TERM without passing it on, so the outer run
# stays: it passes TERM / INT / HUP to the inner run (the group of
# dbus-run-session before the inner run has started), waits until it is gone
# (its trap cleans up), ends what is left of the group and exits with the
# status dbus-run-session hands back (the inner run's). This runs before the arguments are parsed, so
# the inner run gets them unchanged.
if [ -z "${OPUSDM_SANDBOX_BUS:-}" ] || [ ! -f "$OPUSDM_SANDBOX_BUS" ]; then
    command -v dbus-run-session >/dev/null 2>&1 || { echo "dbus-run-session not found (package: dbus-daemon or dbus-bin): the sandbox needs a private D-Bus session"; exit 1; }
    BUS_MARKER="$(mktemp /tmp/opusdm-xephyr-bus-XXXXXX)"
    trap 'rm -f "$BUS_MARKER"' EXIT
    # The forwarding trap comes first (an empty BUS_PID is skipped); HUP too:
    # closing the terminal reaches only this run, the sandbox has a group of its own.
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
    # Own process group: a Ctrl+C reaches only this run, which passes it on.
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
    # Whatever of the sandbox is left in the group (the hub's tools).
    kill -TERM -- "-$BUS_PID" 2>/dev/null || true
    exit "$STATUS"
fi
# The outer run passes its signals on to this pid
echo "$$" >"$OPUSDM_SANDBOX_BUS"
unset OPUSDM_SANDBOX_BUS

DEBUG=0
GRID_DEBUG=0
CONFIG_DIR="${XEPHYR_CONFIG_DIR:-}"
ARGS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --debug) DEBUG=1 ;;
        --grid-debug) GRID_DEBUG=1 ;;
        --config-dir)
            [ $# -ge 2 ] || { echo "--config-dir needs a folder"; exit 2; }
            CONFIG_DIR="$2"
            shift
            ;;
        *) ARGS+=("$1") ;;
    esac
    shift
done
set -- ${ARGS[@]+"${ARGS[@]}"}

# A relative --config-dir means relative to where the user started the script
if [ -n "$CONFIG_DIR" ]; then
    [ -d "$CONFIG_DIR" ] || { echo "Config folder '$CONFIG_DIR' is not a directory"; exit 1; }
    CONFIG_DIR="$(cd "$CONFIG_DIR" && pwd)"
fi

# Work from the repository root, wherever the script was started: the build,
# the config files and the binaries are all found from there.
cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")"

# Configuration (overridable via args or environment)
DISPLAY_NUM="${1:-${XEPHYR_DISPLAY:-:1}}"
RESOLUTION="${2:-${XEPHYR_RES:-1920x1080}}"
TERMINAL="${XEPHYR_TERM:-xterm}"
START_WM="${XEPHYR_WM:-openbox}"   # WM providing decorations; set empty to disable
OPENBOX_CONFIG="${XEPHYR_OB_CONFIG:-$(pwd)/openbox-rc.xml}"  # rc.xml for openbox
COMPOSITOR="${XEPHYR_COMPOSITOR:-picom}"   # compositor for shadows and transparency; set empty to disable
PICOM_CONFIG="${XEPHYR_PICOM_CONFIG:-$(pwd)/picom.conf}"  # picom.conf (GL backend)
READY_TIMEOUT_TENTHS=50            # 50 * 0.1s = 5s max wait for the server

# Exit conditions up front
command -v Xephyr >/dev/null 2>&1 || { echo "Xephyr not found (package: xserver-xephyr)"; exit 1; }
command -v "$TERMINAL" >/dev/null 2>&1 || { echo "Terminal '$TERMINAL' not found"; exit 1; }

# The sandbox is an X11 session even when started from a Wayland desktop:
# without this GTK (and OpusDM's session detection) would pick the host
# compositor instead of Xephyr.
unset WAYLAND_DISPLAY
export GDK_BACKEND=x11
# A live OpusDM session exports OPUSDM_PICOM_BASE: with it the sandbox hub
# would rewrite $XDG_CONFIG_HOME/opusdm/picom.conf (the live file when no
# --config-dir is given) and signal every picom of the user, the live one
# included. Without it the hub leaves picom alone: transparency changes made
# inside the sandbox do not apply live there.
unset OPUSDM_PICOM_BASE

# Use DIR as the OpusDM configuration: OpusDM reads $XDG_CONFIG_HOME/opusdm,
# so XDG_CONFIG_HOME becomes a private folder whose `opusdm` entry is DIR.
if [ -n "$CONFIG_DIR" ]; then
    XDG_CONFIG_HOME="$(mktemp -d /tmp/opusdm-xephyr-config-XXXXXX)"
    ln -s "$CONFIG_DIR" "$XDG_CONFIG_HOME/opusdm"
    export XDG_CONFIG_HOME
    echo "OpusDM configuration: $CONFIG_DIR (XDG_CONFIG_HOME=$XDG_CONFIG_HOME)"
fi

CHILD_PIDS=()
HUB_RUNTIME_DIR=""

# Kill every process we spawned when the script exits
_cleanup() {
    local pid
    for pid in "${CHILD_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    # The hub's runtime dir (made by mktemp below); never an empty path
    [ -n "$HUB_RUNTIME_DIR" ] && rm -rf "$HUB_RUNTIME_DIR"
    # Only the symlink to the config folder lives here; rm never follows it
    [ -n "$CONFIG_DIR" ] && rm -rf "$XDG_CONFIG_HOME"
}
trap _cleanup EXIT INT TERM

# Wait until the nested server answers, or give up
_wait_for_server() {
    local i
    for ((i = 0; i < READY_TIMEOUT_TENTHS; i++)); do
        if DISPLAY="$DISPLAY_NUM" xdpyinfo >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    echo "Xephyr on $DISPLAY_NUM did not become ready in time"
    return 1
}

# Start the nested X server
echo "Starting Xephyr on $DISPLAY_NUM ($RESOLUTION)"
Xephyr "$DISPLAY_NUM" \
    -ac \
    -screen "$RESOLUTION" \
    -glamor \
    -resizeable \
    -no-host-grab \
    -title "Xephyr $DISPLAY_NUM" &
XEPHYR_PID=$!
CHILD_PIDS+=("$XEPHYR_PID")

_wait_for_server

# Optional bare window manager so windows can be moved and focused
if [ -n "$START_WM" ]; then
    if command -v "$START_WM" >/dev/null 2>&1; then
        echo "Starting window manager: $START_WM"
        WM_ARGS=()
        # Use the sandbox openbox config (Amiga-Workbench theme) when present
        if [ "$START_WM" = "openbox" ] && [ -f "$OPENBOX_CONFIG" ]; then
            WM_ARGS=(--config-file "$OPENBOX_CONFIG")
            echo "  using config: $OPENBOX_CONFIG"
        fi
        DISPLAY="$DISPLAY_NUM" XDG_DATA_DIRS="$(pwd)/assets/openbox-share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}" "$START_WM" "${WM_ARGS[@]}" &
        CHILD_PIDS+=("$!")
    else
        echo "Window manager '$START_WM' not found, skipping"
    fi
fi

# Optional compositor (shadows, transparency, GL acceleration)
if [ -n "$COMPOSITOR" ]; then
    if command -v "$COMPOSITOR" >/dev/null 2>&1; then
        echo "Starting compositor: $COMPOSITOR"
        COMP_ARGS=()
        # Use the sandbox picom config when present
        if [ "$COMPOSITOR" = "picom" ] && [ -f "$PICOM_CONFIG" ]; then
            # Run on the sandbox config as it is. The sandbox hub gets no
            # OPUSDM_PICOM_BASE (unset above), so it never rewrites a
            # picom.conf and never signals a picom: the user's live picom
            # and ~/.config/opusdm/picom.conf stay out of reach.
            COMP_ARGS=(--config "$PICOM_CONFIG")
            echo "  using config: $PICOM_CONFIG"
        fi
        DISPLAY="$DISPLAY_NUM" "$COMPOSITOR" ${COMP_ARGS[@]+"${COMP_ARGS[@]}"} &
        CHILD_PIDS+=("$!")
    else
        echo "Compositor '$COMPOSITOR' not found (package: picom), skipping"
    fi
fi

# Build the workspace before entering the sandbox
echo "Building opusdm (release)"
cargo build --release --workspace

# Launch the hub inside the nested server; it spawns the first Lister itself
HUB_BIN="$(pwd)/target/release/opusdm-hub"
if [ ! -x "$HUB_BIN" ]; then
    echo "opusdm-hub not built at $HUB_BIN"
    exit 1
fi
echo "Starting opusdm-hub"
# Give the hub its own runtime dir so a stale socket never blocks the sandbox
HUB_RUNTIME_DIR="$(mktemp -d /tmp/opusdm-xephyr-XXXXXX)"
export XDG_RUNTIME_DIR="$HUB_RUNTIME_DIR"
# Hotkeys such as exec:opusdm-run find the OpusDM binaries by name
export PATH="$(pwd)/target/release:$PATH"
# Draw a red box around every icon-view item (hub desktop and every Lister)
if [ "$GRID_DEBUG" = "1" ]; then
    echo "Grid debug overlay: on"
    export GRID_DEBUG=1
fi
# The sandbox never launches the Startup Apps: they belong to the real session
if [ "$DEBUG" = "1" ]; then
    LOG=/ramdisk/opusdm.log
    echo "Debug log: $LOG"
    OPUSDM_DEBUG=1 DISPLAY="$DISPLAY_NUM" "$HUB_BIN" --no-startup-apps >"$LOG" 2>&1 &
else
    DISPLAY="$DISPLAY_NUM" "$HUB_BIN" --no-startup-apps &
fi
CHILD_PIDS+=("$!")

# Optional extra terminal for poking around
if [ "${XEPHYR_TERM_EXTRA:-0}" = "1" ]; then
    DISPLAY="$DISPLAY_NUM" "$TERMINAL" &
    CHILD_PIDS+=("$!")
fi

# Keep running until Xephyr window is closed
wait "$XEPHYR_PID"
