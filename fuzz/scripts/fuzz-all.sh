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
# NOTE: no require_tools here -- its first probe runs $RUNNER, and on a fresh
# checkout `uv run` silently bootstraps the whole venv (and builds the
# extension), blocking the caller's terminal for minutes. The outer phase
# below needs only tmux; require_tools runs in the inner phase, inside the
# session, where slow bootstrap output is visible and harmless.
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
  tmux new-session -d -s "$SESSION" -n bootstrap "$ENVSTR bash '$SCRIPT' $TIME"
  # Keep the bootstrap window after its command exits so the build log (and,
  # crucially, any build/require_tools FAILURE) stays visible instead of the
  # window -- and, before the target windows exist, the whole session --
  # vanishing. `exec bash` is unreliable here (a non-interactive shell on an
  # unattached pane can hit EOF and exit). remain-on-exit is a *window*
  # option, so it must be set with -w against the bootstrap window
  # specifically; a bare `set-option -t "$SESSION"` silently no-ops.
  tmux set-option -w -t "$SESSION:bootstrap" remain-on-exit on
  cat <<EOF
Detached: building + launching $N targets in tmux session '$SESSION' (time=${TIME}s, jobs/target=${JOBS:-1}).
  Build log:   window 'bootstrap', or  tail -f $FUZZ_DIR/artifacts/bootstrap.log
               (fuzzing windows appear once the build finishes; if it FAILS the
                bootstrap window stays open with the error)
  Attach:      tmux attach -t $SESSION
  Next window: Ctrl-b n     Previous: Ctrl-b p     Detach: Ctrl-b d
  Stop all:    tmux kill-session -t $SESSION
  Crashes:     $FUZZ_DIR/artifacts/<target>/
EOF
  exit 0
fi

# ---- inner phase: runs inside the session's 'bootstrap' window ----

# Mirror everything this phase prints (require_tools, the uppsala retarget +
# build, the launch summary) to a logfile, so the build log -- and any
# FAILURE -- is inspectable with `cat` even if the tmux pane is gone or its
# scrollback is not captured. The on-screen bootstrap window (kept via
# remain-on-exit) is the interactive view; this file is the durable one.
mkdir -p "$FUZZ_DIR/artifacts"
BOOTSTRAP_LOG="$FUZZ_DIR/artifacts/bootstrap.log"
exec > >(tee "$BOOTSTRAP_LOG") 2>&1

# Tool probing happens here, not in the outer phase: on a fresh checkout the
# first `uv run` bootstraps the venv, which is slow but fine inside tmux.
require_tools

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
