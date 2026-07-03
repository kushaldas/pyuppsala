#!/usr/bin/env bash
# Launch every Atheris target in parallel inside one tmux session, one window
# per target. Detach, come back later, check artifacts/. Mirrors uppsala's
# fuzz-all.sh but each window runs a Python/Atheris process instead of a
# cargo-fuzz binary.
#
#   fuzz-all.sh [max_total_time_seconds]     (0 = forever, the default)
#
# Env:
#   SESSION  tmux session name (default: pyuppsala-fuzz)
#   JOBS     fork workers per target (default 1)
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_tools
command -v tmux >/dev/null || { echo "tmux not found"; exit 1; }

SESSION="${SESSION:-pyuppsala-fuzz}"
TIME="${1:-0}"
N="${#ALL_TARGETS[@]}"

# Two-phase launch so the caller's terminal is NEVER blocked (uppsala's
# fuzz-all.sh builds up front too, but its incremental cargo build takes
# seconds; ours retargets uppsala to git main and recompiles for minutes).
# The outer invocation only creates a detached session whose first window
# re-runs this script with FUZZ_ALL_INNER=1; the inner run does the slow
# build and then fans out one window per target. The bootstrap window stays
# open afterwards so the build log remains inspectable.
if [ "${FUZZ_ALL_INNER:-0}" != "1" ]; then
  if tmux has-session -t "$SESSION" 2>/dev/null; then
    echo "tmux session '$SESSION' already exists."
    echo "Attach: tmux attach -t $SESSION    Kill: tmux kill-session -t $SESSION"
    exit 1
  fi
  SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/fuzz-all.sh"
  # Forward the knobs the inner build/run steps read; tmux windows get fresh
  # shells, so anything not spelled out here would be silently dropped.
  ENVSTR="FUZZ_ALL_INNER=1 SESSION='$SESSION' JOBS='${JOBS:-1}'"
  for v in UPPSALA_GIT UPPSALA_REF UPPSALA_PATH ASAN MAX_LEN TIMEOUT RSS_MB RUNNER; do
    [ -n "${!v:-}" ] && ENVSTR="$ENVSTR $v='${!v}'"
  done
  tmux new-session -d -s "$SESSION" -n bootstrap \
    "$ENVSTR bash '$SCRIPT' $TIME; exec bash"
  cat <<EOF
Detached: building + launching $N targets in tmux session '$SESSION' (time=${TIME}s, jobs/target=${JOBS:-1}).
  Build log:   window 'bootstrap' (fuzzing windows appear when the build finishes)
  Attach:      tmux attach -t $SESSION
  Next window: Ctrl-b n     Previous: Ctrl-b p     Detach: Ctrl-b d
  Stop all:    tmux kill-session -t $SESSION
  Crashes:     $FUZZ_DIR/artifacts/<target>/
EOF
  exit 0
fi

# ---- inner phase: runs inside the session's 'bootstrap' window ----

# Compile once up front so target windows start fuzzing immediately instead
# of fighting over the cargo build lock.
"$(dirname "${BASH_SOURCE[0]}")/build.sh"

echo "Launching $N targets in tmux session '$SESSION'  time=${TIME}s  jobs/target=${JOBS:-1}"
for t in "${ALL_TARGETS[@]}"; do
  tmux new-window -d -t "$SESSION" -n "$t"
  tmux send-keys -t "$SESSION:$t" \
    "JOBS=${JOBS:-1} ASAN=${ASAN:-0} '$FUZZ_DIR/scripts/run.sh' $t $TIME" C-m
done

cat <<EOF
Launched $N targets in tmux session '$SESSION'.
  Stop all:    tmux kill-session -t $SESSION
  Crashes:     $FUZZ_DIR/artifacts/<target>/
This bootstrap window stays open with the build log; switch with Ctrl-b n.
EOF
