#!/usr/bin/env bash
# Re-run a single crashing input under a target to reproduce it and get a
# Python traceback (+ ASan report when built with ASAN=1).
#
#   repro.sh <target> <path-to-artifact>
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_tools
TARGET="${1:?usage: repro.sh <target> <artifact>}"
ARTIFACT="${2:?usage: repro.sh <target> <artifact>}"

if [ "${ASAN:-0}" = "1" ]; then
  export LD_PRELOAD="$(asan_preload)"
  export ASAN_OPTIONS="${ASAN_OPTIONS:-allocator_may_return_null=1:detect_leaks=0}"
fi

cd "$REPO_ROOT"
exec $RUNNER "$FUZZ_DIR/$TARGET.py" "$ARTIFACT"
