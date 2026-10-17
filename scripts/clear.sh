#!/bin/sh
# Free disk space used by Cargo build output.
#
# Default: delete the `target/` directory of every agent worktree under
# `.claude/worktrees/` and the incremental caches of the main `target/`.
# No source file or git branch is touched.
#
# Usage: scripts/clear.sh [--dry-run] [--all]
#   --dry-run  only print what would be removed and the space it frees
#   --all      also run `cargo clean` on the main workspace (next build is full)
set -e
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
WORKTREES_DIR="$REPO_ROOT/.claude/worktrees"
MAIN_TARGET="$REPO_ROOT/target"

DRY_RUN=0
CLEAN_ALL=0

for ARG in "$@"; do
    case "$ARG" in
        --dry-run) DRY_RUN=1 ;;
        --all) CLEAN_ALL=1 ;;
        -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "clear: unknown option '$ARG' (use --dry-run, --all)" >&2; exit 2 ;;
    esac
done

if pgrep -x cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1; then
    echo "clear: cargo or rustc is running, refusing to delete build output" >&2
    exit 1
fi

# Print the size of a path in human readable form, or "0" when it is missing.
_size() {
    [ -e "$1" ] || { echo 0; return; }
    du -sh "$1" 2>/dev/null | cut -f1
}

# Remove a directory, or just report it when --dry-run is set.
_remove() {
    [ -d "$1" ] || return 0
    echo "  $(_size "$1")  $1"
    [ "$DRY_RUN" -eq 1 ] && return 0
    rm -rf "$1"
}

BEFORE=$(df -k "$REPO_ROOT" | awk 'NR==2 {print $4}')

echo "Agent worktree targets:"
for WT in "$WORKTREES_DIR"/*/; do
    [ -d "$WT" ] || continue
    _remove "${WT%/}/target"
done

if [ "$CLEAN_ALL" -eq 1 ]; then
    echo "Main target (full clean):"
    echo "  $(_size "$MAIN_TARGET")  $MAIN_TARGET"
    [ "$DRY_RUN" -eq 1 ] || (cd "$REPO_ROOT" && cargo clean)
else
    echo "Main target incremental caches:"
    for INC in "$MAIN_TARGET"/*/incremental; do
        _remove "$INC"
    done
fi

if [ "$DRY_RUN" -eq 1 ]; then
    echo "Dry run: nothing was removed."
    exit 0
fi

AFTER=$(df -k "$REPO_ROOT" | awk 'NR==2 {print $4}')
echo "Freed about $(( (AFTER - BEFORE) / 1024 )) MiB."
