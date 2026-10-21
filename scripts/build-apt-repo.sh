#!/bin/sh
# Add .deb files to a signed apt repository folder (the content of the
# gh-pages branch, served by GitHub Pages).
#
# Usage: GPG_KEY_ID=<key id or fingerprint> scripts/build-apt-repo.sh <repo dir> <file.deb>...
#
# The secret key must already be in the gpg keyring of the caller. Layout:
#   <repo>/pool/main/o/opusdm/*.deb     the packages (only the last KEEP_VERSIONS stay)
#   <repo>/dists/stable/main/binary-<arch>/Packages{,.gz}
#   <repo>/dists/stable/{Release,InRelease,Release.gpg}
#   <repo>/opusdm.gpg                   the public key, to be used with signed-by=
#   <repo>/index.html                   how to add the repository
# Needs apt-ftparchive (package apt-utils) and gpg.
set -eu
SUITE=stable
COMPONENT=main
KEEP_VERSIONS=${KEEP_VERSIONS:-5}
BASE_URL=${BASE_URL:-https://fsoft72.github.io/opusdm}

[ $# -ge 2 ] || { sed -n '2,5p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
[ -n "${GPG_KEY_ID:-}" ] || { echo "build-apt-repo: GPG_KEY_ID is not set" >&2; exit 1; }
for tool in apt-ftparchive gpg gzip; do
    command -v "$tool" >/dev/null 2>&1 || { echo "build-apt-repo: $tool not found (package: apt-utils, gnupg)" >&2; exit 1; }
done

REPO=$1
shift
for deb in "$@"; do
    [ -f "$deb" ] || { echo "build-apt-repo: $deb not found" >&2; exit 1; }
done
mkdir -p "$REPO"
REPO=$(CDPATH= cd -- "$REPO" && pwd)

POOL="$REPO/pool/$COMPONENT/o/opusdm"
mkdir -p "$POOL"
cp "$@" "$POOL/"
# Keep the newest versions only (GitHub Pages sites are limited in size)
ls "$POOL"/opusdm_*.deb | sort -V | head -n "-$KEEP_VERSIONS" | while read -r old; do
    echo "Pruning $old"
    rm -f "$old"
done

cd "$REPO"
ARCHS=""
for deb in "$POOL"/opusdm_*.deb; do
    arch=$(dpkg-deb -f "$deb" Architecture)
    case " $ARCHS " in *" $arch "*) ;; *) ARCHS="${ARCHS:+$ARCHS }$arch" ;; esac
done

rm -rf "dists/$SUITE"
for arch in $ARCHS; do
    mkdir -p "dists/$SUITE/$COMPONENT/binary-$arch"
    apt-ftparchive --arch "$arch" packages pool > "dists/$SUITE/$COMPONENT/binary-$arch/Packages"
    gzip -9 -k -f "dists/$SUITE/$COMPONENT/binary-$arch/Packages"
done

apt-ftparchive \
    -o "APT::FTPArchive::Release::Origin=OpusDM" \
    -o "APT::FTPArchive::Release::Label=OpusDM" \
    -o "APT::FTPArchive::Release::Suite=$SUITE" \
    -o "APT::FTPArchive::Release::Codename=$SUITE" \
    -o "APT::FTPArchive::Release::Architectures=$ARCHS" \
    -o "APT::FTPArchive::Release::Components=$COMPONENT" \
    release "dists/$SUITE" > "dists/$SUITE/Release"

gpg --batch --yes --pinentry-mode loopback --local-user "$GPG_KEY_ID" \
    --clearsign --output "dists/$SUITE/InRelease" "dists/$SUITE/Release"
gpg --batch --yes --pinentry-mode loopback --local-user "$GPG_KEY_ID" \
    --armor --detach-sign --output "dists/$SUITE/Release.gpg" "dists/$SUITE/Release"
# Binary keyring: this is the format signed-by= wants
gpg --batch --yes --export "$GPG_KEY_ID" > opusdm.gpg

cat > index.html <<HTML
<!doctype html>
<meta charset="utf-8">
<title>OpusDM apt repository</title>
<h1>OpusDM apt repository</h1>
<p>Ubuntu 26.04 and later, amd64.</p>
<pre>
sudo install -d -m 0755 /etc/apt/keyrings
sudo curl -fsSL $BASE_URL/opusdm.gpg -o /etc/apt/keyrings/opusdm.gpg
echo "deb [signed-by=/etc/apt/keyrings/opusdm.gpg] $BASE_URL $SUITE $COMPONENT" | sudo tee /etc/apt/sources.list.d/opusdm.list
sudo apt update
sudo apt install opusdm
</pre>
HTML
# GitHub Pages must not run Jekyll over the repository
touch .nojekyll
echo "Done: $REPO ($ARCHS)"
