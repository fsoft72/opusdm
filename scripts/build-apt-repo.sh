#!/bin/sh
# Add .deb files to a signed apt repository folder (the content of the
# gh-pages branch, served by GitHub Pages).
#
# Usage: GPG_KEY_ID=<key id or fingerprint> scripts/build-apt-repo.sh <repo dir> <file.deb>...
#
# One suite per Ubuntu release, named after its codename (resolute = 26.04,
# stonking = 26.10), so apt never offers a package built for a newer release.
# The suite of a .deb comes from the "~ubuntuNN.NN" suffix of its version
# (see build-deb-docker.sh). A .deb without the suffix goes to the legacy suite
# "stable"; the 26.04 build is published in "stable" too, so the existing
# sources lists keep working.
# The secret key must already be in the gpg keyring of the caller. Layout:
#   <repo>/pool/<suite>/main/o/opusdm/*.deb   (only the last KEEP_VERSIONS stay)
#   <repo>/dists/<suite>/main/binary-<arch>/Packages{,.gz}
#   <repo>/dists/<suite>/{Release,InRelease,Release.gpg}
#   <repo>/opusdm.gpg                   the public key, to be used with signed-by=
#   <repo>/index.html                   how to add the repository
# Needs apt-ftparchive (package apt-utils) and gpg.
set -eu
COMPONENT=main
LEGACY_SUITE=stable
KEEP_VERSIONS=${KEEP_VERSIONS:-5}
BASE_URL=${BASE_URL:-https://fsoft72.github.io/opusdm}

[ $# -ge 2 ] || { sed -n '2,5p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
[ -n "${GPG_KEY_ID:-}" ] || { echo "build-apt-repo: GPG_KEY_ID is not set" >&2; exit 1; }
for tool in apt-ftparchive gpg gzip; do
    command -v "$tool" >/dev/null 2>&1 || { echo "build-apt-repo: $tool not found (package: apt-utils, gnupg)" >&2; exit 1; }
done

# Prints the suite codename of an Ubuntu release (NN.NN), nothing if unknown.
_codename() {
    case "$1" in
        26.04) echo resolute ;;
        26.10) echo stonking ;;
    esac
}

REPO=$1
shift
for deb in "$@"; do
    [ -f "$deb" ] || { echo "build-apt-repo: $deb not found" >&2; exit 1; }
done
mkdir -p "$REPO"
REPO=$(CDPATH= cd -- "$REPO" && pwd)

# Layout of the first versions of the repository: one pool for the suite "stable"
if [ -d "$REPO/pool/$COMPONENT/o/opusdm" ]; then
    mkdir -p "$REPO/pool/$LEGACY_SUITE/$COMPONENT/o"
    rm -rf "$REPO/pool/$LEGACY_SUITE/$COMPONENT/o/opusdm"
    mv "$REPO/pool/$COMPONENT/o/opusdm" "$REPO/pool/$LEGACY_SUITE/$COMPONENT/o/opusdm"
    rmdir "$REPO/pool/$COMPONENT/o" "$REPO/pool/$COMPONENT" 2>/dev/null || true
fi

for deb in "$@"; do
    version=$(dpkg-deb -f "$deb" Version)
    release=$(printf '%s\n' "$version" | sed -n 's/.*~ubuntu\([0-9][0-9]\.[0-9][0-9]\)$/\1/p')
    suites=$LEGACY_SUITE
    if [ -n "$release" ]; then
        suite=$(_codename "$release")
        [ -n "$suite" ] || { echo "build-apt-repo: no suite for Ubuntu $release ($deb), add it to _codename" >&2; exit 1; }
        suites=$suite
        [ "$release" != "26.04" ] || suites="$suite $LEGACY_SUITE"
    fi
    for suite in $suites; do
        mkdir -p "$REPO/pool/$suite/$COMPONENT/o/opusdm"
        cp "$deb" "$REPO/pool/$suite/$COMPONENT/o/opusdm/"
    done
done

# Keep the newest versions only (GitHub Pages sites are limited in size)
for pool in "$REPO"/pool/*/"$COMPONENT"/o/opusdm; do
    ls "$pool"/opusdm_*.deb | sort -V | head -n "-$KEEP_VERSIONS" | while read -r old; do
        echo "Pruning $old"
        rm -f "$old"
    done
done

cd "$REPO"
rm -rf dists
SUITES=""
for pooldir in pool/*/; do
    suite=$(basename "$pooldir")
    SUITES="${SUITES:+$SUITES }$suite"
    ARCHS=""
    for deb in "pool/$suite/$COMPONENT/o/opusdm"/opusdm_*.deb; do
        arch=$(dpkg-deb -f "$deb" Architecture)
        case " $ARCHS " in *" $arch "*) ;; *) ARCHS="${ARCHS:+$ARCHS }$arch" ;; esac
    done
    for arch in $ARCHS; do
        mkdir -p "dists/$suite/$COMPONENT/binary-$arch"
        apt-ftparchive --arch "$arch" packages "pool/$suite" > "dists/$suite/$COMPONENT/binary-$arch/Packages"
        gzip -9 -k -f "dists/$suite/$COMPONENT/binary-$arch/Packages"
    done

    apt-ftparchive \
        -o "APT::FTPArchive::Release::Origin=OpusDM" \
        -o "APT::FTPArchive::Release::Label=OpusDM" \
        -o "APT::FTPArchive::Release::Suite=$suite" \
        -o "APT::FTPArchive::Release::Codename=$suite" \
        -o "APT::FTPArchive::Release::Architectures=$ARCHS" \
        -o "APT::FTPArchive::Release::Components=$COMPONENT" \
        release "dists/$suite" > "dists/$suite/Release"

    gpg --batch --yes --pinentry-mode loopback --local-user "$GPG_KEY_ID" \
        --clearsign --output "dists/$suite/InRelease" "dists/$suite/Release"
    gpg --batch --yes --pinentry-mode loopback --local-user "$GPG_KEY_ID" \
        --armor --detach-sign --output "dists/$suite/Release.gpg" "dists/$suite/Release"
done
# Binary keyring: this is the format signed-by= wants
gpg --batch --yes --export "$GPG_KEY_ID" > opusdm.gpg

cat > index.html <<HTML
<!doctype html>
<meta charset="utf-8">
<title>OpusDM apt repository</title>
<h1>OpusDM apt repository</h1>
<p>amd64. Use the suite of your Ubuntu release: <code>resolute</code> (26.04) or <code>stonking</code> (26.10).
The suite <code>stable</code> is the legacy one and holds the 26.04 build.</p>
<pre>
sudo install -d -m 0755 /etc/apt/keyrings
sudo curl -fsSL $BASE_URL/opusdm.gpg -o /etc/apt/keyrings/opusdm.gpg
echo "deb [signed-by=/etc/apt/keyrings/opusdm.gpg] $BASE_URL \$(. /etc/os-release; echo \$VERSION_CODENAME) $COMPONENT" | sudo tee /etc/apt/sources.list.d/opusdm.list
sudo apt update
sudo apt install opusdm
</pre>
HTML
# GitHub Pages must not run Jekyll over the repository
touch .nojekyll
echo "Done: $REPO ($SUITES)"
