#!/usr/bin/env bash
# Build the pyuppsala native extension for fuzzing.
#
# Uppsala source: the committed Cargo.toml depends on the RELEASED uppsala from
# crates.io (this is what CI and normal builds use, and must stay that way).
# Fuzzing, however, should test the NEWEST uppsala, so this script retargets the
# `uppsala` dependency to the git `main` branch via a Cargo `[patch.crates-io]`
# override passed on the command line (maturin forwards `--config` to cargo).
# Nothing is written to Cargo.toml -- the override lives only in this build
# invocation, so it affects fuzzing builds ONLY.
#
#   Env:
#     UPPSALA_GIT   git URL         (default https://github.com/kushaldas/uppsala)
#     UPPSALA_REF   branch/tag/rev  (default main)
#     UPPSALA_PATH  local checkout to use INSTEAD of git (e.g. ../uppsala or a
#                   worktree); overrides UPPSALA_GIT/REF when set
#     ASAN=1        instrument the Rust extension with AddressSanitizer (nightly)
#
# Two build modes:
#   (default)  plain release build. Atheris still gives coverage-guided fuzzing
#              of the pure-Python etree layer and the PyO3 boundary; native
#              segfaults are caught by faulthandler and libFuzzer.
#   ASAN=1     use-after-free / OOB inside uppsala is detected directly; run.sh
#              then LD_PRELOADs Atheris's asan_with_fuzzer.so.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
cd "$REPO_ROOT"

UPPSALA_GIT="${UPPSALA_GIT:-https://github.com/kushaldas/uppsala}"
UPPSALA_REF="${UPPSALA_REF:-main}"
UPPSALA_PATH="${UPPSALA_PATH:-}"

# Build the [patch.crates-io] override args (maturin -> cargo --config).
PATCH_ARGS=()
if [ -n "$UPPSALA_PATH" ]; then
  ABS="$(cd "$UPPSALA_PATH" && pwd)"
  PATCH_ARGS=(--config "patch.crates-io.uppsala.path=\"$ABS\"")
  SRC_DESC="local path $ABS"
else
  PATCH_ARGS=(
    --config "patch.crates-io.uppsala.git=\"$UPPSALA_GIT\""
    --config "patch.crates-io.uppsala.branch=\"$UPPSALA_REF\""
  )
  SRC_DESC="git $UPPSALA_GIT @ $UPPSALA_REF"
fi

echo "Fuzz build: retargeting uppsala -> $SRC_DESC (via [patch.crates-io], Cargo.toml untouched)"

if [ "${ASAN:-0}" = "1" ]; then
  echo "Building with AddressSanitizer (nightly + -Zsanitizer=address)..."
  TRIPLE="$(rustc -vV | sed -n 's/host: //p')"
  export RUSTFLAGS="-Zsanitizer=address -Cdebuginfo=1 ${RUSTFLAGS:-}"
  export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-nightly}"
  uv run maturin develop --release --target "$TRIPLE" -Zbuild-std "${PATCH_ARGS[@]}"
  echo "ASan build done. run.sh will LD_PRELOAD $(asan_preload)"
else
  echo "Building pyuppsala (release, no sanitizer)..."
  uv run maturin develop --release "${PATCH_ARGS[@]}"
fi

# Effectiveness check: confirm the override actually took effect. [patch.crates-io]
# only rewrites a dependency that comes FROM crates.io. If the working tree still
# pins uppsala to a local `path = ...` (a dev convenience during the perf cycle),
# the patch silently no-ops and the fuzzer would test that local copy, NOT main.
# Warn loudly so the result is never silently misattributed.
SRC="$(cargo metadata --format-version 1 "${PATCH_ARGS[@]}" 2>/dev/null \
  | uv run python -c "import sys,json; d=json.load(sys.stdin); print(next((p['source'] or 'local-path' for p in d['packages'] if p['name']=='uppsala'), 'none'))" 2>/dev/null || echo unknown)"

if [ -z "$UPPSALA_PATH" ] && [ "${SRC#git+}" = "$SRC" ]; then
  cat >&2 <<EOF

  WARNING: uppsala resolved to '$SRC', not the git '$UPPSALA_REF' branch.
  The [patch.crates-io] override only applies when Cargo.toml depends on the
  crates.io release (as CI does). This working tree appears to pin uppsala to a
  local path, so the fuzzer is building THAT, not main. To fuzz against main,
  either use a tree with the crates.io dependency, or set UPPSALA_PATH to point
  the override at a checkout of main explicitly.

EOF
else
  echo "Confirmed: uppsala resolves to '$SRC'."
fi
