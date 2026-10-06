#!/bin/sh
# Build OpusDM in release mode and replace the installed version of the running
# desktop session (the current user's Desktop Manager).
#
# Usage: scripts/live-update.sh [--config] [--all] [--prefix DIR]
#   (default)  replace the binaries in PREFIX/bin and restart only opusdm-hub.
#              Listers, dock and other windows already open keep running the
#              old binaries until they are closed and opened again.
#   --config   also replace PREFIX/config (Openbox and picom configuration) and
#              PREFIX/opusdm-session, then ask Openbox to reload its config.
#   --all      terminate every OpusDM process of the current user (hub, lister,
#              dock, ...) and start the hub again, so everything runs the new
#              binaries. Open OpusDM windows are closed.
#   --prefix   installation prefix, default /opt/opusdm.
#
# Openbox and picom are never restarted. The copy needs sudo because the prefix
# is owned by root.
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
DIST="$REPO_ROOT/dist"
PREFIX=/opt/opusdm
WITH_CONFIG=0
RESTART_ALL=0
# Process names (as shown by pgrep -x) of every OpusDM binary
PROCESSES="opusdm-hub opusdm-lister opusdm-settings opusdm-dock opusdm-run opusdm-shot opusdm-player opusdm-viewer opusdm-apps-editor"

while [ $# -gt 0 ]; do
    case "$1" in
        --config) WITH_CONFIG=1 ;;
        --all) RESTART_ALL=1 ;;
        --prefix)
            [ $# -ge 2 ] || { echo "live-update: --prefix needs a directory" >&2; exit 2; }
            PREFIX="$2"
            shift
            ;;
        -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "live-update: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done

if [ "$(id -u)" -eq 0 ]; then SUDO=""; else SUDO="sudo"; fi

# Exit conditions up front
[ -d "$PREFIX/bin" ] || { echo "live-update: $PREFIX/bin not found, run dist/install.sh first" >&2; exit 1; }

# Build and stage dist/. A failed build stops here and the install is untouched.
"$SCRIPT_DIR/build-release.sh"

# Remember the display of the running hub: the script may be started from a
# terminal that does not carry the session environment.
HUB_PID=$(pgrep -u "$(id -u)" -x opusdm-hub | head -n 1 || true)
if [ -z "${DISPLAY:-}" ] && [ -n "$HUB_PID" ] && [ -r "/proc/$HUB_PID/environ" ]; then
    DISPLAY=$(tr '\0' '\n' < "/proc/$HUB_PID/environ" | sed -n 's/^DISPLAY=//p' | head -n 1)
    export DISPLAY
fi

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
    if command -v openbox >/dev/null 2>&1 && [ -n "${DISPLAY:-}" ]; then
        openbox --reconfigure || echo "live-update: openbox --reconfigure failed" >&2
    fi
fi

# Without a running hub there is no session to update
if [ -z "$HUB_PID" ]; then
    echo "No opusdm-hub running for this user: files installed, nothing restarted"
    exit 0
fi
[ -n "${DISPLAY:-}" ] || { echo "live-update: DISPLAY is not set, hub not restarted" >&2; exit 1; }

# Stop the hub, or every OpusDM process with --all
if [ "$RESTART_ALL" -eq 1 ]; then STOP="$PROCESSES"; else STOP="opusdm-hub"; fi
echo "Stopping: $STOP"
for p in $STOP; do
    pkill -u "$(id -u)" -x "$p" || true
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

# Same environment as opusdm-session: the OpusDM binaries are found by name.
# setsid detaches the new hub from this terminal.
echo "Starting opusdm-hub"
PATH="$PREFIX/bin:$PATH" setsid "$PREFIX/bin/opusdm-hub" >/dev/null 2>&1 < /dev/null &
echo "Done"
