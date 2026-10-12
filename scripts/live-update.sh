#!/bin/sh
# Build OpusDM in release mode and replace the installed version of the running
# desktop session (the current user's Desktop Manager).
#
# Usage: scripts/live-update.sh [--config] [--all] [--prefix DIR]
#   (default)  replace the binaries in PREFIX/bin and restart only opusdm-hub.
#              Listers, dock and other windows already open keep running the
#              old binaries until they are closed and opened again.
#   --config   also replace PREFIX/config (Openbox, picom and labwc
#              configuration) and the session scripts, then ask the window
#              manager of the running session (Openbox or labwc) to reload.
#   --all      terminate every OpusDM process of the current user (hub, lister,
#              dock, ...) and start the hub again, so everything runs the new
#              binaries. Open OpusDM windows are closed. The new hub does not
#              launch the Startup Apps again (--no-startup-apps).
#   --prefix   installation prefix, default /opt/opusdm.
#
# Works in the X11 and in the Wayland session: the restarted hub gets the
# session environment of the running one. Openbox, picom and labwc are never
# restarted. The copy needs sudo because the prefix is owned by root. The restarted hub writes its output to
# ${XDG_STATE_HOME:-~/.local/state}/opusdm/hub.log (the previous run's log is
# kept as hub.log.1).
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
DIST="$REPO_ROOT/dist"
PREFIX=/opt/opusdm
WITH_CONFIG=0
RESTART_ALL=0
# Every OpusDM binary that can be running. opusdm-terminal is left out on
# purpose: killing it would end the user's shells, possibly the one running
# this script. Open terminals pick up the new binary when opened again.
PROCESSES="opusdm-hub opusdm-lister opusdm-settings opusdm-dock opusdm-run opusdm-shot opusdm-player opusdm-viewer opusdm-apps-editor opusdm-input-config opusdm-screen opusdm-polkit"
# The kernel keeps only the first 15 characters of a process name, and pgrep -x
# compares against that: a longer name would never match.
PROC_NAME_MAX=15

while [ $# -gt 0 ]; do
    case "$1" in
        --config) WITH_CONFIG=1 ;;
        --all) RESTART_ALL=1 ;;
        --prefix)
            [ $# -ge 2 ] || { echo "live-update: --prefix needs a directory" >&2; exit 2; }
            PREFIX="$2"
            shift
            ;;
        -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "live-update: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done

if [ "$(id -u)" -eq 0 ]; then SUDO=""; else SUDO="sudo"; fi

# Exit conditions up front
[ -d "$PREFIX/bin" ] || { echo "live-update: $PREFIX/bin not found, run dist/install.sh first" >&2; exit 1; }

# Build and stage dist/. A failed build stops here and the install is untouched.
"$SCRIPT_DIR/build-release.sh"

# Take the session environment of the running hub: the script may be started
# from a terminal of another session (or none), and the new hub must join the
# same X11 or Wayland session. A variable the hub does not have is unset.
SESSION_VARS="DISPLAY WAYLAND_DISPLAY XDG_RUNTIME_DIR XDG_SESSION_TYPE XDG_CURRENT_DESKTOP LABWC_PID WAYFIRE_SOCKET WAYFIRE_CONFIG_FILE GDK_BACKEND DBUS_SESSION_BUS_ADDRESS OPUSDM_LABWC_RC"
HUB_PID=$(pgrep -u "$(id -u)" -x opusdm-hub | head -n 1 || true)
if [ -n "$HUB_PID" ] && [ -r "/proc/$HUB_PID/environ" ]; then
    for v in $SESSION_VARS; do
        line=$(tr '\0' '\n' < "/proc/$HUB_PID/environ" | grep "^$v=" | head -n 1 || true)
        if [ -n "$line" ]; then export "$line"; else unset "$v"; fi
    done
fi
# A hub restarted by an older live-update lost Wayfire's variables (and with
# them the Wayfire support of the panel and of the input settings). Wayfire's
# IPC plugin listens on /tmp/wayfire-$WAYLAND_DISPLAY.socket, and the session
# script runs it with the per-user OpusDM wayfire.ini.
if [ -z "${WAYFIRE_SOCKET:-}" ] && [ -n "${WAYLAND_DISPLAY:-}" ] && [ -S "/tmp/wayfire-${WAYLAND_DISPLAY##*/}.socket" ]; then
    export WAYFIRE_SOCKET="/tmp/wayfire-${WAYLAND_DISPLAY##*/}.socket"
    WAYFIRE_INI="${XDG_CONFIG_HOME:-$HOME/.config}/opusdm/wayfire.ini"
    [ -z "${WAYFIRE_CONFIG_FILE:-}" ] && [ -f "$WAYFIRE_INI" ] && export WAYFIRE_CONFIG_FILE="$WAYFIRE_INI"
fi
# Wayland unless the hub was forced onto X (GDK_BACKEND=x11), as OpusDM decides.
if [ -n "${WAYLAND_DISPLAY:-}" ] && [ "${GDK_BACKEND:-}" != "x11" ]; then SESSION=wayland; else SESSION=x11; fi

# Replace each binary through a rename: overwriting a running executable in
# place fails with "Text file busy", a rename does not touch the running inode.
echo "Installing binaries in $PREFIX/bin"
for f in "$DIST"/bin/*; do
    name=$(basename "$f")
    $SUDO install -m 755 "$f" "$PREFIX/bin/$name.new"
    $SUDO mv -f "$PREFIX/bin/$name.new" "$PREFIX/bin/$name"
done

if [ "$WITH_CONFIG" -eq 1 ]; then
    echo "Installing config"
    $SUDO cp -r "$DIST/config/." "$PREFIX/config/"
    $SUDO install -m 755 "$DIST/opusdm-session" "$PREFIX/opusdm-session"
    $SUDO install -m 755 "$DIST/opusdm-session-wayland" "$PREFIX/opusdm-session-wayland"
    if [ "$SESSION" = wayland ]; then
        # The user's ~/.config/labwc/rc.xml is theirs: only reload it.
        if [ -n "${LABWC_PID:-}" ]; then
            kill -HUP "$LABWC_PID" || echo "live-update: cannot ask labwc to reload" >&2
        fi
    elif command -v openbox >/dev/null 2>&1 && [ -n "${DISPLAY:-}" ]; then
        openbox --reconfigure || echo "live-update: openbox --reconfigure failed" >&2
    fi
fi

# Without a running hub there is no session to update
if [ -z "$HUB_PID" ]; then
    echo "No opusdm-hub running for this user: files installed, nothing restarted"
    exit 0
fi
if [ "$SESSION" = wayland ]; then
    echo "Session: Wayland ($WAYLAND_DISPLAY)"
else
    [ -n "${DISPLAY:-}" ] || { echo "live-update: neither WAYLAND_DISPLAY nor DISPLAY is set, hub not restarted" >&2; exit 1; }
    echo "Session: X11 ($DISPLAY)"
fi

# Stop the hub, or every OpusDM process with --all
if [ "$RESTART_ALL" -eq 1 ]; then STOP="$PROCESSES"; else STOP="opusdm-hub"; fi
echo "Stopping: $STOP"
for p in $STOP; do
    pkill -u "$(id -u)" -x "$(printf '%.*s' "$PROC_NAME_MAX" "$p")" || true
done

# Wait up to 5 seconds for the hub to go away before starting the new one
i=0
while pgrep -u "$(id -u)" -x opusdm-hub >/dev/null && [ "$i" -lt 50 ]; do
    sleep 0.1
    i=$((i + 1))
done
if pgrep -u "$(id -u)" -x opusdm-hub >/dev/null; then
    echo "live-update: opusdm-hub did not exit, not starting a second one" >&2
    exit 1
fi

# The hub's output goes to a log file, so a crash leaves a trace. The log of
# the previous run is kept once.
LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/opusdm"
HUB_LOG="$LOG_DIR/hub.log"
mkdir -p "$LOG_DIR"
[ -f "$HUB_LOG" ] && mv -f "$HUB_LOG" "$HUB_LOG.1"

# Same environment as opusdm-session: the OpusDM binaries are found by name.
# setsid detaches the new hub from this terminal. RUST_BACKTRACE puts the stack
# of a panic in the log.
echo "Starting opusdm-hub (log: $HUB_LOG)"
# With --all the Startup Apps are still running (only OpusDM processes were
# stopped), so the new hub must not launch them again.
HUB_ARGS=""
[ "$RESTART_ALL" -eq 1 ] && HUB_ARGS="--no-startup-apps"
PATH="$PREFIX/bin:$PATH" RUST_BACKTRACE=1 setsid "$PREFIX/bin/opusdm-hub" $HUB_ARGS >"$HUB_LOG" 2>&1 < /dev/null &
echo "Done"
