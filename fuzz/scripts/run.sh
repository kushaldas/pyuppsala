#!/usr/bin/env bash
# Run ONE Atheris fuzz target.
#
#   run.sh <target> [max_total_time_seconds] [extra libfuzzer args...]
#
# Env:
#   JOBS      parallel libFuzzer fork workers (default 1; >1 sets -fork/-ignore_crashes)
#   MAX_LEN   max input length in bytes        (default 16384)
#   TIMEOUT   per-input timeout in seconds      (default 25; the DoS oracle)
#   RSS_MB    per-process RSS cap in MiB        (default 4096; the memory-DoS oracle)
#   ASAN      1 => LD_PRELOAD Atheris's asan_with_fuzzer.so (needs an ASan build)
#
# libFuzzer writes any crash/timeout/oom input to artifacts/<target>/ and, in
# fork mode, keeps going -- safe to leave running unattended in tmux.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_tools

TARGET="${1:?usage: run.sh <target> [seconds] [extra libfuzzer args...]}"
TIME="${2:-0}"
shift || true; shift || true

JOBS="${JOBS:-1}"
MAX_LEN="${MAX_LEN:-16384}"
TIMEOUT="${TIMEOUT:-25}"
RSS_MB="${RSS_MB:-4096}"

CORPUS="$FUZZ_DIR/corpus/$TARGET"
SEEDS="$FUZZ_DIR/seeds/$TARGET"
ARTIFACTS="$FUZZ_DIR/artifacts/$TARGET"
mkdir -p "$CORPUS" "$ARTIFACTS"

# Seed the working corpus from the tracked seeds (never overwrites discoveries).
if [ -d "$SEEDS" ]; then
  cp -n "$SEEDS"/* "$CORPUS/" 2>/dev/null || true
fi

DICT="$(dict_for "$TARGET")"
DICT_ARG=()
[ -f "$DICT" ] && DICT_ARG=(-dict="$DICT")

FORK_ARG=()
if [ "$JOBS" -gt 1 ]; then
  FORK_ARG=(-fork="$JOBS" -ignore_crashes=1)
fi

# ASan mode: preload Atheris's fuzzer+ASan runtime and tame the common noise.
if [ "${ASAN:-0}" = "1" ]; then
  export LD_PRELOAD="$(asan_preload)"
  export ASAN_OPTIONS="${ASAN_OPTIONS:-allocator_may_return_null=1:detect_leaks=0}"
fi

cd "$REPO_ROOT"
echo ">> $TARGET  jobs=$JOBS  max_len=$MAX_LEN  timeout=${TIMEOUT}s  rss=${RSS_MB}M  time=${TIME}s  dict=$(basename "${DICT:-none}")  asan=${ASAN:-0}"
exec $RUNNER "$FUZZ_DIR/$TARGET.py" \
  "$CORPUS" \
  -artifact_prefix="$ARTIFACTS/" \
  -max_len="$MAX_LEN" \
  -timeout="$TIMEOUT" \
  -rss_limit_mb="$RSS_MB" \
  -max_total_time="$TIME" \
  "${DICT_ARG[@]}" \
  "${FORK_ARG[@]}" \
  "$@"
