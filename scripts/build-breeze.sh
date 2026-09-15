#!/bin/sh
# Build the Breeze Dark theme package from ./themes/breeze-dark.
set -e
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

(cd "$REPO_ROOT" && cargo run -p xtask -- package-themes)

ZIP_PATH="$REPO_ROOT/target/themes/breeze-dark.zip"
if [ ! -f "$ZIP_PATH" ]; then
    echo "build-breeze: expected $ZIP_PATH to be built, but it is missing" >&2
    exit 1
fi

echo "Built $ZIP_PATH"
