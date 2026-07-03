#!/usr/bin/env bash
# Best-effort line-coverage report for a target's corpus over the pure-Python
# etree layer (coverage.py). Unlike uppsala's llvm-cov this cannot see inside
# the compiled _pyuppsala extension -- native coverage needs an ASan/SanCov
# build (ASAN=1) plus llvm-cov, which is out of scope for this wrapper. Use this
# to check which parts of etree.py / _elementpath.py the corpus actually drives.
#
#   coverage.sh <target>
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_tools
TARGET="${1:?usage: coverage.sh <target>}"

$RUNNER -c "import coverage" 2>/dev/null || {
  echo "coverage.py missing: uv pip install -r fuzz/requirements.txt"; exit 1; }

CORPUS="$FUZZ_DIR/corpus/$TARGET"
OUT="$FUZZ_DIR/coverage/$TARGET"
mkdir -p "$OUT"
shopt -s nullglob
FILES=("$CORPUS"/*)
[ "${#FILES[@]}" -eq 0 ] && { echo "empty corpus for $TARGET; run it first"; exit 1; }

cd "$REPO_ROOT"
# Replay the corpus as individual libFuzzer inputs (runs each once, then exits)
# under coverage.py restricted to the etree layer.
ATHERIS_NO_INSTRUMENT=1 $RUNNER -m coverage run \
  --source=pyuppsala.etree,pyuppsala._elementpath \
  "$FUZZ_DIR/$TARGET.py" "${FILES[@]}" -runs=0 || true
$RUNNER -m coverage html -d "$OUT/html"
echo "Coverage HTML: $OUT/html/index.html"
