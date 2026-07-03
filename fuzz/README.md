# pyuppsala fuzzing (Atheris)

Coverage-guided fuzzing of the pyuppsala PyO3 extension and its `etree` layer,
built with [Atheris](https://github.com/google/atheris) (libFuzzer for Python).
This mirrors the Rust `cargo-fuzz` harness in `../../uppsala/audit/fuzz` but
exercises the library through its **Python API**, so it also covers the binding
glue: the proxy cache, the interned-tag table, cross-document moves, and the
handle lifetime boundary that only exists in pyuppsala.

The layout follows the [OSS-Fuzz Python
convention](https://google.github.io/oss-fuzz/getting-started/new-project-guide/python-lang/):
harnesses are top-level `fuzz/<name>_fuzzer.py` files with an `atheris.Setup(...,
TestOneInput)` entry point, discovered by `find $SRC -name '*_fuzzer.py'`. The
harness design also follows the Trail of Bits guide for fuzzing Python C
extensions with Atheris: <https://appsec.guide/docs/fuzzing/python/>.

```
fuzz/
  <name>_fuzzer.py        the 12 harnesses (TestOneInput entry point)
  harness_common.py       shared oracle (NOT a *_fuzzer.py, so not a target)
  dict/                   xml.dict, xpath.dict, xsd_regex.dict
  seeds/<name>_fuzzer/    per-target seed corpus -> <name>_fuzzer_seed_corpus.zip
  scripts/                local campaign runner (build/run/fuzz-all/repro/...)
  oss-fuzz/               project.yaml, Dockerfile, build.sh for oss-fuzz/projects/pyuppsala
  corpus/ artifacts/      campaign working state (gitignored)
```

## The oracle

Every harness draws one line:

* **Expected** (swallowed): the library raising a *documented malformed-input
  error* — `XmlParseError`, `XmlWellFormednessError`, `XmlNamespaceError`,
  `XPathError`, `XsdValidationError`, the `etree` `LxmlError` family, plus
  `ValueError`/`TypeError`/`UnicodeError`/`NotImplementedError` for bad
  arguments. This is correct behaviour on hostile bytes.
* **Finding** (propagates → libFuzzer reports a crash): anything else. A Rust
  `panic!` crosses PyO3 as `pyo3_runtime.PanicException` (a `BaseException`, so
  it slips past `except Exception`); a hang is caught by `-timeout`; unbounded
  memory by `-rss_limit_mb`; a native use-after-free / OOB by `faulthandler`
  (always) or AddressSanitizer (in the `ASAN=1` build); an oracle violation
  (e.g. a non-idempotent serialization) by an `assert`.

See `harness_common.py`.

## Quick start

```bash
just fuzz-setup                       # sfw uv pip install atheris coverage
just fuzz-build                       # maturin develop --release (uppsala main)
just fuzz-one roundtrip_fuzzer 60     # one target, 60s, foreground
just fuzz 0                           # all targets, forever, in tmux
just fuzz-crashes                     # list artifacts
just fuzz-repro roundtrip_fuzzer fuzz/artifacts/roundtrip_fuzzer/crash-<hash>
```

Per-run knobs are environment variables read by `scripts/run.sh`: `JOBS`
(fork workers), `MAX_LEN`, `TIMEOUT` (per-input, the DoS oracle), `RSS_MB`,
and `ASAN`.

## Targets and what they hunt

| Target | Surface | CVE families (A–J) | Spec sections |
|--------|---------|--------------------|---------------|
| `parse_fuzzer` | `parse` / `etree.fromstring` (str) | C, D, I | §2, §8, §13 |
| `parse_bytes_fuzzer` | `parse_bytes` + encoding detect | H, D | §7, §13 |
| `roundtrip_fuzzer` | parse→serialize→reparse idempotence | **E**, I, J | §8, **§10** |
| `serialize_injection_fuzzer` | text/attr/comment/PI content escaping | **J**, F | §9 |
| `xpath_fuzzer` | XPath eval over fuzzed expr + doc | J, C | §3 |
| `dom_mutate_fuzzer` | proxy cache + cross-tree deep-copy | D, E | §10, §11 |
| `transform_fuzzer` | XSLT engine (`Xslt.transform`) | G | §12 |
| `xsd_builder_fuzzer` | XSD schema build + validate (string) | G, C | §4 |
| `xsd_regex_fuzzer` | XSD regex compile + match (ReDoS) | C | §6 |
| `xsd_from_file_fuzzer` | `XsdValidator.from_file` include/import | A, G | §5 |
| `defused_fuzzer` | DOCTYPE / entity knobs + cap posture | **A**, **B**, G | §0, §1 |
| `ffi_lifetime_fuzzer` | `Node`/`Document` handle lifetime (UAF) | **D** | **§11** |

Two harnesses assert **invariants**, not just "no crash":

* `roundtrip_fuzzer` — once serialized, a document must reparse-and-reserialize to
  the exact same bytes (the SAML round-trip / auth-bypass class, CVE-2020-29509).
* `defused_fuzzer` — if the default parse recognised a `<!DOCTYPE>`, then
  `forbid_dtd=True` must reject the same input, and `forbid_entities=True` must
  reject it when the DOCTYPE declares an `<!ENTITY>`. A returned document is a
  hardening bypass. It also pins the documented cap constants at startup so a
  silent regression (e.g. `DEFAULT_MAX_DEPTH` drifting off 128) fails loudly.

`serialize_injection_fuzzer` asserts a **shape** invariant (attacker content must
never spawn new elements/attributes when serialized), chosen so that legal XML
whitespace/char-ref normalization and the library's U+FFFD replacement of
invalid characters never produce a false positive.

## Uppsala version: fuzzing builds test git `main`

pyuppsala's committed `Cargo.toml` depends on the **released** uppsala from
crates.io — that is what CI and normal builds use, and it must stay that way.
Fuzzing, though, should exercise the *newest* uppsala, so `scripts/build.sh`
retargets the `uppsala` dependency to the git `main` branch of
<https://github.com/kushaldas/uppsala> using a Cargo `[patch.crates-io]`
override passed on the command line (`maturin --config` → `cargo --config`).
**Nothing is written to `Cargo.toml`** — the override exists only inside the
fuzz build invocation, so it affects fuzzing builds only. CI's crates.io
dependency is untouched.

Knobs (environment variables read by `build.sh`):

| Var | Default | Effect |
|-----|---------|--------|
| `UPPSALA_GIT` | `https://github.com/kushaldas/uppsala` | git URL to fuzz against |
| `UPPSALA_REF` | `main` | branch / tag / rev |
| `UPPSALA_PATH` | *(unset)* | use a local checkout instead of git (e.g. a `main` worktree) |

`build.sh` runs `cargo metadata` after building and prints `Confirmed: uppsala
resolves to git+...` when the override took effect. Because `[patch.crates-io]`
only rewrites a dependency that *comes from* crates.io, a working tree that pins
uppsala to a local `path = ...` (a dev convenience during the perf cycle) makes
the patch a no-op; `build.sh` detects this and prints a loud warning so a fuzz
result is never silently attributed to `main` when it actually tested a local
copy. On such a tree, set `UPPSALA_PATH` to a `main` checkout to force it.

## Two build modes

`fuzz-build` (default) does a plain `maturin develop --release`. Atheris still
provides coverage-guided fuzzing of the pure-Python `etree` layer and the PyO3
boundary, catches Rust panics, hangs and memory blowups, and — with
`faulthandler` armed in `ffi_lifetime_fuzzer` — turns any native segfault into a
visible traceback. This mode runs with the stock toolchain.

`ASAN=1 just fuzz-build` instruments the Rust extension itself with
AddressSanitizer (`-Zsanitizer=address`, nightly) so a use-after-free or
out-of-bounds *inside uppsala* is caught directly, and `run.sh` then
`LD_PRELOAD`s Atheris's `asan_with_fuzzer.so` with
`ASAN_OPTIONS=allocator_may_return_null=1:detect_leaks=0`. Note the core uppsala
scanners are already fuzzed under ASan at the Rust level; this mode is for the
binding glue and the Python-reachable paths.

## OSS-Fuzz integration

`fuzz/oss-fuzz/` holds the three files OSS-Fuzz needs, ready to copy into
`oss-fuzz/projects/pyuppsala/`:

* `project.yaml` — `language: python`, libFuzzer, address + undefined sanitizers.
* `Dockerfile` — `base-builder-python` plus a Rust toolchain (pyuppsala is a
  maturin/PyO3 project) and `maturin`; clones the repo into `$SRC/pyuppsala`.
* `build.sh` — writes a `[patch.crates-io] uppsala = { git, branch = main }`
  into `$CARGO_HOME/config.toml` (same "fuzz against uppsala main, keep the
  crates.io dep for CI" contract, via the config form of `[patch]`), `pip3
  install .`, then for each `fuzz/*_fuzzer.py`: `compile_python_fuzzer`, zips
  `seeds/<name>/` into `$OUT/<name>_seed_corpus.zip`, and copies the matching
  dictionary to `$OUT/<name>.dict`.

Local check with the OSS-Fuzz tooling:

```bash
# from an oss-fuzz checkout, after copying fuzz/oss-fuzz/* to projects/pyuppsala/
python3 infra/helper.py build_image pyuppsala
python3 infra/helper.py build_fuzzers --sanitizer address pyuppsala
python3 infra/helper.py check_build pyuppsala
python3 infra/helper.py run_fuzzer pyuppsala roundtrip_fuzzer
```

## Coverage note

`scripts/coverage.sh` reports line coverage of the **pure-Python** `etree` /
`_elementpath` layer only — it cannot see inside the compiled extension.
Meaningful native-code coverage needs the `ASAN=1` SanitizerCoverage build plus
`llvm-cov`; the deep parser/XPath/XSD/serializer coverage lives in uppsala's own
`cargo fuzz coverage` reports.

## Scope: what this does NOT cover (belongs in the pytest security suite)

The fuzzer proves *no panic / no crash / no oracle break* on arbitrary input. It
does **not** assert exact resource-cap boundaries, prove the filesystem/network
is never touched, or check spec-conformant results. Those are deterministic
assertions better written as `pytest` cases per the spec's §0–§13:

* exact cap boundaries (depth 127 passes / 129 fails, node-visit budget, regex
  step cap wall-clock ceiling);
* `from_file` path-traversal / SSRF *proof* via an `open`/syscall monitor
  watching the planted sentinel (§5);
* `NotImplementedError` for `recover`, C14N, `iterparse`, RelaxNG, etc. (§0/§10);
* namespace-resolution correctness and Clark round-trip identity (§8);
* the threading / GIL concurrency contract (§11);
* XSLT `document()` / `xsl:include` file-read + SSRF behaviour, once the source
  audit in §12 establishes what the engine actually implements.
