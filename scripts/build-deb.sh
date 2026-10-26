#!/bin/sh
# Build the opusdm .deb from the dist/ staged by build-release.sh.
#
# Usage: scripts/build-deb.sh [--skip-build]
#   --skip-build   reuse the existing ./dist instead of running build-release.sh
#
# Output: target/deb/opusdm_<version>_<arch>.deb
#
# The package installs exactly what install.sh --x11 --wayland labwc --wayland
# wayfire does, with the same prefix (/opt/opusdm), so a .deb install and a
# live-update.sh install are interchangeable:
#   /opt/opusdm/{bin,config,themes,opusdm-session,opusdm-session-wayland}
#   /usr/share/{themes,applications,icons}, the three session entries and a
#   /usr/bin/opusdm-* link for every command.
# Shared library dependencies are computed with dpkg-shlibdeps, so they match
# the Ubuntu release the package is built on.
#
# Environment: DEB_VERSION_SUFFIX is appended to the package version (for
# example "~ubuntu26.04", see build-deb-docker.sh).
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
DIST="$REPO_ROOT/dist"
PREFIX=/opt/opusdm
SKIP_BUILD=0

while [ $# -gt 0 ]; do
    case "$1" in
        --skip-build) SKIP_BUILD=1 ;;
        -h|--help) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "build-deb: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done

for tool in dpkg-deb dpkg-shlibdeps dpkg strip; do
    command -v "$tool" >/dev/null 2>&1 || { echo "build-deb: $tool not found (package: dpkg-dev, binutils)" >&2; exit 1; }
done

cd "$REPO_ROOT"

[ "$SKIP_BUILD" -eq 1 ] || "$SCRIPT_DIR/build-release.sh"
[ -d "$DIST/bin" ] || { echo "build-deb: $DIST/bin not found, run scripts/build-release.sh first" >&2; exit 1; }

# Workspace version: the first `version = "x.y.z"` of Cargo.toml
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
[ -n "$VERSION" ] || { echo "build-deb: cannot read the version from Cargo.toml" >&2; exit 1; }
VERSION="$VERSION${DEB_VERSION_SUFFIX:-}"
ARCH=$(dpkg --print-architecture)
NAME="opusdm_${VERSION}_${ARCH}"
OUT_DIR="$REPO_ROOT/target/deb"
ROOT="$OUT_DIR/$NAME"

echo "Staging $NAME"
rm -rf "$ROOT"
mkdir -p "$ROOT/DEBIAN" "$ROOT$PREFIX/bin" "$ROOT/usr/bin" "$ROOT/usr/share/applications" \
    "$ROOT/usr/share/xsessions" "$ROOT/usr/share/wayland-sessions"

# Binaries are stripped in the staging copy only: dist/ and target/ keep their symbols
for f in "$DIST"/bin/*; do
    install -m 755 "$f" "$ROOT$PREFIX/bin/$(basename "$f")"
done
for f in "$ROOT$PREFIX"/bin/opusdm-*; do
    # opusdm-open-folder is a shell script
    if file -b "$f" 2>/dev/null | grep -q '^ELF'; then strip --strip-unneeded "$f"; fi
done
cp -r "$DIST/config" "$DIST/themes" "$ROOT$PREFIX/"
install -m 755 "$DIST/opusdm-session" "$DIST/opusdm-session-wayland" "$ROOT$PREFIX/"
# Every command is linked into /usr/bin so it is on the PATH of any session
# (the hub finds opusdm-lister through current_exe, which resolves the link)
for f in "$ROOT$PREFIX"/bin/opusdm-*; do
    ln -s "$PREFIX/bin/$(basename "$f")" "$ROOT/usr/bin/$(basename "$f")"
done

# Openbox theme, icon and the hidden per-app desktop entries
cp -r "$DIST/share/." "$ROOT/usr/share/"
cp "$DIST/opusdm-lister.desktop" "$ROOT/usr/share/applications/opusdm-lister.desktop"

# Session entries, same names install.sh writes
sed "s|@PREFIX@|$PREFIX|" "$DIST/opusdm.desktop" > "$ROOT/usr/share/xsessions/opusdm.desktop"
for compositor in labwc wayfire; do
    sed -e "s|@PREFIX@|$PREFIX|" -e "s|@COMPOSITOR@|$compositor|g" "$DIST/opusdm-wayland.desktop" \
        > "$ROOT/usr/share/wayland-sessions/opusdm-$compositor.desktop"
done

# Shared library dependencies. dpkg-shlibdeps wants a debian/control in the
# current folder, so run it in a scratch one.
SHLIBS_DIR=$(mktemp -d)
trap 'rm -rf "$SHLIBS_DIR"' EXIT
mkdir -p "$SHLIBS_DIR/debian"
printf 'Source: opusdm\n\nPackage: opusdm\nArchitecture: any\n' > "$SHLIBS_DIR/debian/control"
ELF_ARGS=""
for f in "$ROOT$PREFIX"/bin/opusdm-*; do
    if file -b "$f" 2>/dev/null | grep -q '^ELF'; then ELF_ARGS="$ELF_ARGS -e$f"; fi
done
# shellcheck disable=SC2086
SHLIBS=$(cd "$SHLIBS_DIR" && dpkg-shlibdeps -O $ELF_ARGS 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
[ -n "$SHLIBS" ] || { echo "build-deb: dpkg-shlibdeps found no dependencies, refusing to build" >&2; exit 1; }

# Do not inherit the umask of the build machine: 755 folders, 644 data files
# (executables keep their 755 and the maintainer scripts are set below)
chmod -R u=rwX,go=rX "$ROOT"

INSTALLED_SIZE=$(du -sk "$ROOT" | cut -f1)
cat > "$ROOT/DEBIAN/control" <<CONTROL
Package: opusdm
Version: $VERSION
Section: x11
Priority: optional
Architecture: $ARCH
Maintainer: Fabio Rotondo <fabio.rotondo@gmail.com>
Installed-Size: $INSTALLED_SIZE
Depends: $SHLIBS
Recommends: openbox, picom, labwc, wayfire, wlr-randr, grim, curl, gnome-keyring, gstreamer1.0-gtk4, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-libav, webp-pixbuf-loader
Homepage: https://github.com/fsoft72/opusdm
Description: OpusDM desktop environment and file manager
 OpusDM is a desktop made of a hub, the Lister file manager, a panel and a
 dock, global hotkeys, a media player, a viewer, a terminal and a screenshot
 tool. It registers the sessions "OpusDM (X11)", "OpusDM (labwc)" and
 "OpusDM (wayfire)" in the login manager.
CONTROL

cat > "$ROOT/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
gtk-update-icon-cache -f -t /usr/share/icons/hicolor 2>/dev/null || true
update-desktop-database -q /usr/share/applications 2>/dev/null || true
exit 0
POSTINST
cat > "$ROOT/DEBIAN/postrm" <<'POSTRM'
#!/bin/sh
set -e
case "$1" in
    remove|purge)
        gtk-update-icon-cache -f -t /usr/share/icons/hicolor 2>/dev/null || true
        update-desktop-database -q /usr/share/applications 2>/dev/null || true
        ;;
esac
exit 0
POSTRM
chmod 755 "$ROOT/DEBIAN/postinst" "$ROOT/DEBIAN/postrm"

dpkg-deb --root-owner-group --build "$ROOT" "$OUT_DIR/$NAME.deb"
echo "Done: $OUT_DIR/$NAME.deb"
