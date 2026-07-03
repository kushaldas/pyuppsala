#!/usr/bin/env bash
# Shared helpers for the pyuppsala Atheris fuzz scripts. Sourced, not executed.
#
# Layout follows the OSS-Fuzz Python convention: harnesses are top-level
# `fuzz/<name>_fuzzer.py` files. The targets are Atheris (Python + native PyO3
# extension), so "building" means compiling the maturin extension (and, for
# ASAN=1, pointing LD_PRELOAD at Atheris's sanitizer runtime).
set -euo pipefail

FUZZ_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"   # .../fuzz
REPO_ROOT="$(cd "$FUZZ_DIR/.." && pwd)"                       # repo root

ALL_TARGETS=(
  parse_fuzzer
  parse_bytes_fuzzer
  roundtrip_fuzzer
  serialize_injection_fuzzer
  xpath_fuzzer
  dom_mutate_fuzzer
  transform_fuzzer
  xsd_builder_fuzzer
  xsd_regex_fuzzer
  xsd_from_file_fuzzer
  defused_fuzzer
  ffi_lifetime_fuzzer
)

# Map a target to its libFuzzer dictionary (empty if none).
dict_for() {
  case "$1" in
    xpath_fuzzer)     echo "$FUZZ_DIR/dict/xpath.dict" ;;
    xsd_regex_fuzzer) echo "$FUZZ_DIR/dict/xsd_regex.dict" ;;
    *)                echo "$FUZZ_DIR/dict/xml.dict" ;;   # all XML-shaped inputs
  esac
}

# `uv run` keeps everything inside the project venv (where pyuppsala + atheris
# live). Override RUNNER=python if you manage the environment yourself.
RUNNER="${RUNNER:-uv run python}"

require_tools() {
  # Probe the runner itself rather than guessing at uv/python separately:
  # the default is `uv run python`, but RUNNER=python is a supported
  # override, and only the interpreter the scripts will actually invoke
  # matters.
  $RUNNER -c "" 2>/dev/null || {
    echo "runner '$RUNNER' not usable: install uv, or set RUNNER=python for a self-managed environment"; exit 1; }
  $RUNNER -c "import atheris" 2>/dev/null || {
    echo "atheris missing: run 'just fuzz-setup' (uv pip install -r fuzz/requirements.txt)"; exit 1; }
  $RUNNER -c "import pyuppsala" 2>/dev/null || {
    echo "pyuppsala not importable: run 'just fuzz-build'"; exit 1; }
}

# Path to Atheris's ASan+libFuzzer preload .so (only needed for the ASan build).
asan_preload() {
  $RUNNER - <<'PY'
import atheris, os
print(os.path.join(os.path.dirname(atheris.__file__), "asan_with_fuzzer.so"))
PY
}
