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

if tmux has-session -t "$SESSION" 2>/dev/null; then
  echo "tmux session '$SESSION' already exists."
  echo "Attach: tmux attach -t $SESSION    Kill: tmux kill-session -t $SESSION"
  exit 1
fi

# Compile once up front so windows start fuzzing immediately.
"$(dirname "${BASH_SOURCE[0]}")/build.sh"

N="${#ALL_TARGETS[@]}"
echo "Launching $N targets in tmux session '$SESSION'  time=${TIME}s  jobs/target=${JOBS:-1}"
tmux new-session -d -s "$SESSION" -n "${ALL_TARGETS[0]}"
first=1
for t in "${ALL_TARGETS[@]}"; do
  if [ "$first" -eq 0 ]; then
    tmux new-window -t "$SESSION" -n "$t"
  fi
  first=0
  tmux send-keys -t "$SESSION:$t" \
    "JOBS=${JOBS:-1} '$FUZZ_DIR/scripts/run.sh' $t $TIME" C-m
done

cat <<EOF
Launched $N targets in tmux session '$SESSION'.
  Attach:      tmux attach -t $SESSION
  Next window: Ctrl-b n     Previous: Ctrl-b p     Detach: Ctrl-b d
  Stop all:    tmux kill-session -t $SESSION
  Crashes:     $FUZZ_DIR/artifacts/<target>/
EOF
