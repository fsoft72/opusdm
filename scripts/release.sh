#!/bin/sh
# Tag the current commit with v<version> and push the branch and the tag.
# The tag starts .github/workflows/release-deb.yml, which builds the .deb and
# attaches it to the GitHub Release.
#
# Usage: scripts/release.sh [-y]
#   -y   do not ask for confirmation before pushing
#
# <version> is the workspace version of Cargo.toml. Bump it (and commit)
# before running this script.
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
REMOTE=origin
ASSUME_YES=0

case "${1:-}" in
    "") ;;
    -y) ASSUME_YES=1 ;;
    -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "release: unknown option $1" >&2; exit 2 ;;
esac

cd "$REPO_ROOT"

# Workspace version: the first `version = "x.y.z"` of Cargo.toml
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
[ -n "$VERSION" ] || { echo "release: cannot read the version from Cargo.toml" >&2; exit 1; }
TAG="v$VERSION"
BRANCH=$(git rev-parse --abbrev-ref HEAD)

# Exit conditions up front
[ "$BRANCH" != "HEAD" ] || { echo "release: detached HEAD, check out a branch first" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "release: the working tree is not clean, commit or stash first" >&2; exit 1; }
! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || { echo "release: tag $TAG already exists locally" >&2; exit 1; }
git remote get-url "$REMOTE" >/dev/null 2>&1 || { echo "release: remote $REMOTE not found" >&2; exit 1; }
if git ls-remote --exit-code --tags "$REMOTE" "refs/tags/$TAG" >/dev/null 2>&1; then
    echo "release: tag $TAG already exists on $REMOTE" >&2
    exit 1
fi

echo "Release $TAG from branch $BRANCH ($(git rev-parse --short HEAD): $(git log -1 --format=%s))"
echo "Will push: $BRANCH and $TAG to $REMOTE ($(git remote get-url "$REMOTE"))"

if [ "$ASSUME_YES" -ne 1 ]; then
    printf 'Continue? [y/N] '
    read -r ANSWER
    case "$ANSWER" in
        y|Y|yes|YES) ;;
        *) echo "release: aborted"; exit 1 ;;
    esac
fi

git tag -a "$TAG" -m "OpusDM $VERSION"
# Branch first, so the tag never points at a commit the remote does not have
git push "$REMOTE" "$BRANCH"
git push "$REMOTE" "$TAG"
echo "Done: $TAG pushed, follow the build in the Actions tab of the repository"
