#!/bin/sh
# Bump the workspace version, build, commit, tag v<version> and push the branch
# and the tag. The tag starts .github/workflows/release-deb.yml, which builds
# the .deb, attaches it to the GitHub Release and updates the apt repository.
#
# Usage: scripts/release.sh <new version> [-y]
#   <new version>  X.Y.Z, greater than the current workspace version
#   -y             do not ask for confirmation
#
# Steps: set [workspace.package] version in Cargo.toml, add a line to
# CHANGES.md, `cargo build --release --workspace` (refreshes Cargo.lock),
# commit the three files, tag and push. A failed build restores the files.
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
REMOTE=origin
ASSUME_YES=0
NEW_VERSION=
BUMP_FILES="Cargo.toml Cargo.lock CHANGES.md"

# Prints the usage block of the header.
_usage() {
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
}

# Prints an error and exits.
_die() {
    echo "release: $*" >&2
    exit 1
}

# Prints the workspace version: the first `version = "x.y.z"` line of Cargo.toml.
_current_version() {
    sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1
}

# Puts back Cargo.toml, Cargo.lock and CHANGES.md as in HEAD.
_restore() {
    # shellcheck disable=SC2086
    git checkout -- $BUMP_FILES || echo "release: could not restore $BUMP_FILES, check git status" >&2
}

# Sets the first `version = "..."` line of Cargo.toml (the workspace one) to $1.
_set_cargo_version() {
    tmp=$(mktemp)
    awk -v v="$1" '!done && /^version = ".*"$/ { print "version = \"" v "\""; done = 1; next } { print }' Cargo.toml > "$tmp"
    cat "$tmp" > Cargo.toml
    rm -f "$tmp"
}

# Adds the version bump line to CHANGES.md under today's heading (created if missing).
_add_changes_line() {
    today=$(date +%F)
    line="- Version bump to **$1** (\`[workspace.package]\` in \`Cargo.toml\`, inherited by all crates and \`xtask\`; \`Cargo.lock\` refreshed) with \`scripts/release.sh\`. No code change. Verified: \`cargo build --release --workspace\` passes. NOT verified: nothing else exercised, no behaviour touched."
    tmp=$(mktemp)
    awk -v today="## $today" -v line="$line" '
        !done && /^## / {
            if ($0 == today) { print; print line } else { print today; print line; print "" ; print }
            done = 1; next
        }
        { print }
        END { if (!done) { print ""; print today; print line } }
    ' CHANGES.md > "$tmp"
    cat "$tmp" > CHANGES.md
    rm -f "$tmp"
}

for arg in "$@"; do
    case "$arg" in
        -y) ASSUME_YES=1 ;;
        -h|--help) _usage; exit 0 ;;
        -*) echo "release: unknown option $arg" >&2; exit 2 ;;
        *)
            [ -z "$NEW_VERSION" ] || { echo "release: more than one version given" >&2; exit 2; }
            NEW_VERSION=$arg
            ;;
    esac
done
[ -n "$NEW_VERSION" ] || { _usage >&2; exit 2; }

cd "$REPO_ROOT"

CURRENT=$(_current_version)
TAG="v$NEW_VERSION"
BRANCH=$(git rev-parse --abbrev-ref HEAD)

# Exit conditions up front, before any file is touched
[ -n "$CURRENT" ] || _die "cannot read the version from Cargo.toml"
echo "$NEW_VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || _die "version $NEW_VERSION is not X.Y.Z"
[ "$NEW_VERSION" != "$CURRENT" ] || _die "version $NEW_VERSION is already the current one"
[ "$(printf '%s\n%s\n' "$CURRENT" "$NEW_VERSION" | sort -V | tail -n 1)" = "$NEW_VERSION" ] || _die "version $NEW_VERSION is lower than the current $CURRENT"
[ "$BRANCH" != "HEAD" ] || _die "detached HEAD, check out a branch first"
[ -z "$(git status --porcelain)" ] || _die "the working tree is not clean, commit or stash first"
! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || _die "tag $TAG already exists locally"
git remote get-url "$REMOTE" >/dev/null 2>&1 || _die "remote $REMOTE not found"
if git ls-remote --exit-code --tags "$REMOTE" "refs/tags/$TAG" >/dev/null 2>&1; then
    _die "tag $TAG already exists on $REMOTE"
fi

echo "Release $TAG from branch $BRANCH ($(git rev-parse --short HEAD): $(git log -1 --format=%s))"
echo "Will: bump $CURRENT -> $NEW_VERSION, build (release), commit $BUMP_FILES, tag $TAG"
echo "      and push $BRANCH and $TAG to $REMOTE ($(git remote get-url "$REMOTE"))"

if [ "$ASSUME_YES" -ne 1 ]; then
    printf 'Continue? [y/N] '
    read -r ANSWER
    case "$ANSWER" in
        y|Y|yes|YES) ;;
        *) echo "release: aborted"; exit 1 ;;
    esac
fi

_set_cargo_version "$NEW_VERSION"
if [ "$(_current_version)" != "$NEW_VERSION" ]; then
    _restore
    _die "could not set the version in Cargo.toml"
fi
_add_changes_line "$NEW_VERSION"

echo "Building workspace (release)"
if ! cargo build --release --workspace; then
    _restore
    _die "build failed, version files restored"
fi

# shellcheck disable=SC2086
git add $BUMP_FILES
if ! git commit -m "Bump version to $NEW_VERSION" -m "Version-only change made by scripts/release.sh, no code touched. No lifetime or ownership changes."; then
    git reset -q -- $BUMP_FILES
    _restore
    _die "commit failed, version files restored"
fi

git tag -a "$TAG" -m "OpusDM $NEW_VERSION"
# Branch first, so the tag never points at a commit the remote does not have
git push "$REMOTE" "$BRANCH" || _die "push of $BRANCH failed; the commit and tag $TAG are local, retry with: git push $REMOTE $BRANCH && git push $REMOTE $TAG"
git push "$REMOTE" "$TAG" || _die "push of $TAG failed, retry with: git push $REMOTE $TAG"
echo "Done: $TAG pushed, follow the build in the Actions tab of the repository"
