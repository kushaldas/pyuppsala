#!/bin/bash -eu
# OSS-Fuzz build script for pyuppsala. Copy into oss-fuzz/projects/pyuppsala/.
#
# Steps:
#   1. Retarget the uppsala dependency to git `main` (fuzz builds test the newest
#      uppsala; the committed Cargo.toml keeps the crates.io release for CI).
#   2. Build + install the maturin/PyO3 extension so `import pyuppsala` works.
#   3. Compile every fuzz/<name>_fuzzer.py with compile_python_fuzzer.
#   4. Ship each target's seed corpus and dictionary into $OUT.

FUZZ_DIR="$SRC/pyuppsala/fuzz"

# --- 1. uppsala main override, via a Cargo config [patch] (no Cargo.toml edit) ---
# [patch] is honoured in $CARGO_HOME/config.toml and only applies to the
# crates.io dependency pyuppsala ships, exactly as CI pins it.
UPPSALA_GIT="${UPPSALA_GIT:-https://github.com/kushaldas/uppsala}"
UPPSALA_REF="${UPPSALA_REF:-main}"
mkdir -p "${CARGO_HOME:-$HOME/.cargo}"
cat >> "${CARGO_HOME:-$HOME/.cargo}/config.toml" <<EOF

[patch.crates-io]
uppsala = { git = "$UPPSALA_GIT", branch = "$UPPSALA_REF" }
EOF

# --- 2. build + install the extension ---
# For the address sanitizer, instrument the Rust code too (C-oriented CFLAGS do
# not reach rustc). Requires the nightly toolchain installed in the Dockerfile.
if [ "${SANITIZER:-}" = "address" ]; then
  export RUSTUP_TOOLCHAIN=nightly
  export RUSTFLAGS="-Zsanitizer=address -Cdebuginfo=1 ${RUSTFLAGS:-}"
fi
pip3 install .

# harness_common.py sits beside the harnesses; put it on the path so
# PyInstaller (inside compile_python_fuzzer) bundles it.
export PYTHONPATH="${PYTHONPATH:-}:$FUZZ_DIR"

# --- 3 + 4. compile each fuzzer, ship its seeds + dict ---
for fuzzer in "$FUZZ_DIR"/*_fuzzer.py; do
  name="$(basename "$fuzzer" .py)"
  compile_python_fuzzer "$fuzzer"

  # Seed corpus: fuzz/seeds/<name>/  ->  $OUT/<name>_seed_corpus.zip
  if [ -d "$FUZZ_DIR/seeds/$name" ]; then
    zip -j "$OUT/${name}_seed_corpus.zip" "$FUZZ_DIR/seeds/$name/"* >/dev/null || true
  fi

  # Dictionary: xpath/xsd_regex have their own; everything else is XML-shaped.
  case "$name" in
    xpath_fuzzer)     dict="$FUZZ_DIR/dict/xpath.dict" ;;
    xsd_regex_fuzzer) dict="$FUZZ_DIR/dict/xsd_regex.dict" ;;
    *)                dict="$FUZZ_DIR/dict/xml.dict" ;;
  esac
  [ -f "$dict" ] && cp "$dict" "$OUT/${name}.dict"
done
