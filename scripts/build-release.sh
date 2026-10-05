#!/bin/sh
# Build OpusDM in release mode and stage everything needed to deploy it in ./dist.
#
# Layout of dist/:
#   bin/      opusdm-hub, opusdm-lister, opusdm-settings, opusdm-dock, opusdm-run,
#             opusdm-shot
#             (the hub finds opusdm-lister next to itself, keep them together)
#   config/   openbox-rc.xml, picom.conf
#   themes/   <slug>.zip theme packages
#   opusdm-session    X11 session script (Openbox + picom + opusdm-hub)
#   opusdm.desktop    xsession entry template (Exec is set by install.sh)
#   install.sh        copies dist to a prefix and registers the xsession
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
DIST="$REPO_ROOT/dist"
BINARIES="opusdm-hub opusdm-lister opusdm-settings opusdm-dock opusdm-run opusdm-shot"

cd "$REPO_ROOT"

echo "Building workspace (release)"
cargo build --release --workspace

echo "Packaging themes"
cargo run --release -p xtask -- package-themes

# Start from a clean dist so removed files do not linger
rm -rf "$DIST"
mkdir -p "$DIST/bin" "$DIST/config" "$DIST/themes"

for b in $BINARIES; do
    src="$REPO_ROOT/target/release/$b"
    if [ ! -x "$src" ]; then
        echo "build-release: missing binary $src" >&2
        exit 1
    fi
    cp "$src" "$DIST/bin/$b"
done

cp "$REPO_ROOT/openbox-rc.xml" "$REPO_ROOT/picom.conf" "$DIST/config/"
cp "$REPO_ROOT"/target/themes/*.zip "$DIST/themes/"

# X11 session script: resolves its own location, so dist can live anywhere
cat > "$DIST/opusdm-session" <<'SESSION'
#!/bin/sh
# OpusDM X11 session: compositor, hub and Openbox as the window manager.
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# Hotkeys such as exec:opusdm-run find the OpusDM binaries by name.
export PATH="$HERE/bin:$PATH"

if command -v picom >/dev/null 2>&1; then
    picom --config "$HERE/config/picom.conf" &
fi
"$HERE/bin/opusdm-hub" &
exec openbox --config-file "$HERE/config/openbox-rc.xml"
SESSION
chmod +x "$DIST/opusdm-session"

cat > "$DIST/opusdm.desktop" <<'DESKTOP'
[Desktop Entry]
Name=OpusDM
Comment=OpusDM desktop on Openbox
Exec=@PREFIX@/opusdm-session
Type=Application
DESKTOP

cat > "$DIST/install.sh" <<'INSTALL'
#!/bin/sh
# Copy this dist to a prefix and register the OpusDM X11 session.
# Usage: ./install.sh [prefix]   (default: /opt/opusdm, needs sudo for the xsession entry)
set -eu
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PREFIX="${1:-/opt/opusdm}"
XSESSIONS=/usr/share/xsessions

if [ "$(id -u)" -eq 0 ]; then SUDO=""; else SUDO="sudo"; fi

$SUDO mkdir -p "$PREFIX"
$SUDO cp -r "$HERE/bin" "$HERE/config" "$HERE/themes" "$HERE/opusdm-session" "$PREFIX/"
sed "s|@PREFIX@|$PREFIX|" "$HERE/opusdm.desktop" | $SUDO tee "$XSESSIONS/opusdm.desktop" >/dev/null
echo "Installed to $PREFIX, session entry: $XSESSIONS/opusdm.desktop"
INSTALL
chmod +x "$DIST/install.sh"

echo "Done: $DIST"
