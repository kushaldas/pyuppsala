#!/usr/bin/env bash
# Minimize a target's corpus (drop inputs that add no coverage) via libFuzzer's
# -merge. Run periodically during long campaigns to keep the corpus small.
#
#   minimize.sh <target>
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_tools
TARGET="${1:?usage: minimize.sh <target>}"

CORPUS="$FUZZ_DIR/corpus/$TARGET"
MIN="$FUZZ_DIR/corpus/${TARGET}.min"
rm -rf "$MIN"; mkdir -p "$MIN"

cd "$REPO_ROOT"
$RUNNER "$FUZZ_DIR/$TARGET.py" -merge=1 "$MIN" "$CORPUS"
rm -rf "$CORPUS"; mv "$MIN" "$CORPUS"
echo "Minimized corpus for $TARGET -> $(ls -1 "$CORPUS" | wc -l) inputs"
