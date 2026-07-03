# pyuppsala - Python bindings for the Uppsala XML library
default:
    @just --list

# ─── Build & test ───

# Build the native extension in development mode
build:
    uv run maturin develop

# Build the native extension optimized
build-release:
    uv run maturin develop --release

# Run the Python test suite
test:
    uv run pytest tests/ -v

# Run a single test, e.g. `just test-one test_parse_simple`
test-one name:
    uv run pytest tests/ -v -k {{name}}

# Build a release wheel
wheel:
    uv run maturin build --release

# Build the Sphinx docs
docs:
    cd docs && make html

# ─── Fuzzing (top-level fuzz/, Atheris + libFuzzer over the PyO3 extension) ───
#
# OSS-Fuzz-style layout: harnesses are fuzz/<name>_fuzzer.py. Atheris drives the
# same input->crash loop as uppsala's cargo-fuzz targets, but against the Python
# API of the compiled extension. The oracle is "any exception that is NOT a
# documented malformed-input error is a finding" (Rust panics surface as
# PanicException; hangs/OOM are caught by -timeout / -rss_limit_mb; native
# faults by faulthandler, or ASan in the ASAN=1 build). See fuzz/README.md.

# Install the fuzzing toolchain (atheris + coverage). Run once per machine.
fuzz-setup:
    sfw uv pip install -r fuzz/requirements.txt

# Build the extension for fuzzing. Retargets uppsala to git `main` via a
# [patch.crates-io] override (Cargo.toml untouched; CI keeps the crates.io dep).
# Env: UPPSALA_REF=<branch> UPPSALA_GIT=<url> UPPSALA_PATH=<local> ASAN=1
fuzz-build:
    ./fuzz/scripts/build.sh

# Fuzz ALL targets in a detached tmux session (remote-friendly).
# Arg = seconds per target, 0 = forever.
fuzz seconds="0":
    ./fuzz/scripts/fuzz-all.sh {{seconds}}

# Fuzz a single target in the foreground.
# e.g. `just fuzz-one roundtrip_fuzzer 3600`
fuzz-one target seconds="0":
    ./fuzz/scripts/run.sh {{target}} {{seconds}}

# Reattach to the running fuzz session.
fuzz-attach:
    tmux attach -t pyuppsala-fuzz

# Stop all fuzzing (kills the tmux session).
fuzz-stop:
    tmux kill-session -t pyuppsala-fuzz || true

# List any crash/timeout/oom artifacts found so far.
fuzz-crashes:
    @find fuzz/artifacts -type f 2>/dev/null | sort || echo "no artifacts yet"

# Reproduce a crash: `just fuzz-repro parse_fuzzer fuzz/artifacts/parse_fuzzer/crash-abc`
fuzz-repro target artifact:
    ./fuzz/scripts/repro.sh {{target}} {{artifact}}

# Best-effort etree-layer line-coverage report for a target's corpus.
fuzz-coverage target:
    ./fuzz/scripts/coverage.sh {{target}}

# Minimize a target's corpus (drop redundant inputs).
fuzz-cmin target:
    ./fuzz/scripts/minimize.sh {{target}}
