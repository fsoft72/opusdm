#!/bin/sh
# Build the opusdm .deb for one or more Ubuntu releases, each one inside an
# ubuntu:<release> container, so the Depends match that release.
#
# Usage: scripts/build-deb-docker.sh [release...]
#   release   Ubuntu release to build for (default: 26.04 26.10)
#
# Output: target/deb-docker/opusdm_<version>~ubuntu<release>_<arch>.deb
#
# The release suffix keeps the two packages distinct for apt (26.10 sorts
# after 26.04), so both can live in the same repository pool.
# The source tree is copied into the container (target/, dist/ and .git are
# left out), the host checkout is never written. Cargo keeps one persistent
# target directory per release under target/docker-cache/.
# Needs Docker.
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
OUT_DIR="$REPO_ROOT/target/deb-docker"
CACHE_DIR="$REPO_ROOT/target/docker-cache"

case "${1:-}" in
    -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
esac

command -v docker >/dev/null 2>&1 || { echo "build-deb-docker: docker not found" >&2; exit 1; }
[ $# -gt 0 ] || set -- 26.04 26.10
for release in "$@"; do
    case "$release" in
        [0-9][0-9].[0-9][0-9]) ;;
        *) echo "build-deb-docker: invalid release '$release' (expected NN.NN, e.g. 26.04)" >&2; exit 2 ;;
    esac
done

mkdir -p "$OUT_DIR" "$CACHE_DIR/registry"

for release in "$@"; do
    image="opusdm-deb-build:$release"
    target_cache="$CACHE_DIR/target-$release"
    mkdir -p "$target_cache"

    echo "==> [$release] Building the Docker image $image"
    docker build -t "$image" --build-arg "RELEASE=$release" -f "$SCRIPT_DIR/Dockerfile.deb" "$SCRIPT_DIR" \
        || { echo "build-deb-docker: docker build failed for $release" >&2; exit 1; }

    echo "==> [$release] Building the .deb"
    rm -f "$OUT_DIR"/opusdm_*"~ubuntu$release"_*.deb
    docker run --rm \
        -v "$REPO_ROOT:/src:ro" \
        -v "$OUT_DIR:/out" \
        -v "$CACHE_DIR/registry:/opt/cargo/registry" \
        -v "$target_cache:/cargo-target" \
        -e CARGO_TARGET_DIR=/cargo-target \
        -e "DEB_VERSION_SUFFIX=~ubuntu$release" \
        -e "RELEASE=$release" \
        -e "HOST_UID=$(id -u)" -e "HOST_GID=$(id -g)" \
        "$image" \
        sh -c '
            set -eu
            trap "chown -R $HOST_UID:$HOST_GID /out /opt/cargo/registry /cargo-target" EXIT
            tar -C /src --exclude=./target --exclude=./dist --exclude=./.git -cf - . | tar -C /work -xf -
            ln -s /cargo-target /work/target
            rm -rf /cargo-target/deb
            /work/scripts/build-deb.sh
            cp /cargo-target/deb/opusdm_*"~ubuntu$RELEASE"_*.deb /out/
        ' || { echo "build-deb-docker: build failed for $release" >&2; exit 1; }
done

echo "==> Done:"
for release in "$@"; do
    ls -l "$OUT_DIR"/opusdm_*"~ubuntu$release"_*.deb
done
