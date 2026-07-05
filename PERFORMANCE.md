# pyuppsala performance notes

How the binding layer gets its speed, what was done in each optimization cycle, current
numbers vs `lxml`, and how to reproduce them. The consumer-side view of the same work
(pyFF macro benchmarks, per-cycle A/B/C wall times on the deployment host) lives in
`pyFF/performance.md`; this file is the library-side record.

Status: the work below ships in pyuppsala 0.8.0, built against the `uppsala` 0.8 release
from crates.io (`Cargo.toml`: `uppsala = "0.8"`), which carries the required uppsala-side
changes (the xpath arena-reuse fix and the serializer fast paths).

## Design principles

The architecture follows five decisions (in leverage order):

1. **Identity-stable proxy cache, in Rust.** lxml's core trick is that the tree lives in
   C and Python wrappers are created lazily and cached. Our equivalent: the tree lives in
   uppsala's arena; `_DocHolderBase` (native) owns a `node_id -> weakref(_Element)` map so
   repeated access returns the *same* wrapper (`root[0] is root[0]`) and untouched nodes
   cost nothing. Both the hit path (dict lookup + weakref upgrade) and the miss path
   (construct `_Element` through the registered type object, insert a callback-free
   weakref, bounded tombstone sweep) run without a Python frame.
2. **Interned names.** `.tag` returns an interned per-document `Py<PyString>`: one object
   per unique `(namespace, local)` pair for the document's lifetime, verified by
   piecewise comparison (zero allocation on a hit). Equal tags become pointer-identical,
   so `elem.tag == "{ns}local"` hits CPython's string identity fast path. Renames hash to
   a different key, so there is nothing to invalidate. (Arena-level QName interning was
   evaluated and deliberately deferred: parsed names are zero-copy `Cow::Borrowed` slices,
   which is why parse beats lxml, and interning would tax that path.)
3. **Bulk operations never cross the FFI boundary per node.** Traversal
   (`iter`/`__iter__`/`getparent`) walks the arena natively and yields cached proxies;
   `iter` is a native iterator whose `__next__` ends in the proxy cache -- a cache hit
   allocates no Python object at all. Serialization builds the whole string in Rust.
   Batch APIs (`parse_many`, `fetch_many`, `fetch_and_parse_many`) take one FFI call for
   an entire corpus.
4. **Allocation discipline.** `#[pyclass(freelist = 2048)]` pools the short-lived `Node`
   shells that navigation creates; the serializer's per-element/per-attribute allocations
   were removed (see Cycle 12 below); the proxy cache's weakrefs are callback-free
   because walk-created proxies die instantly and a death callback per proxy dominated.
5. **The GIL is released wherever the work is pure Rust.** Parse, XSLT compile+transform,
   XSD build/validate, serialization, and the batch/network APIs all run under
   `Python::detach`. Lock rule: GIL -> document mutex, never the reverse; only owned data
   crosses into a detached closure; a detached closure may take and release the document
   mutex internally. This is what lets N Python threads parse on N cores -- and it
   multiplies with consumers like pyFF's thread-pooled resource loading.

## Optimization cycles (library-side)

Cycles 1-11 are recorded in detail in `pyFF/performance.md` (they were driven by the pyFF
macro benchmark). Library-side summary:

| Cycle | What moved into Rust / changed |
|-------|-------------------------------|
| 2 | `Node.iter_descendants` + `DescendantIterator` (lazy pre-order walk, filter parsed once) |
| 3 | uppsala `Document::import_subtree` (cross-document deep copy in one native pass) |
| 4 | `Node.nsmap`, `Node.content_children` / `content_child_count` |
| 5 | `Node.clark_tag` (native Clark-notation formatting) |
| 7 | Proxy cache: `WeakValueDictionary` -> plain dict of callback-free `weakref.ref` + bounded sweep (pure Python) |
| 8-11 | Native `_ElementBase`: `tag`/`text`/`tail`/`__len__`/`nsmap`/`prefix`/`sourceline` getters |

### Cycle 12 (2026-07-02) -- the big one

**uppsala: XPath arena leak fixed (correctness + memory).** `prepare_xpath()` builds
virtual attribute nodes into the (append-only) arena; every re-preparation after a tree
mutation used to append a fresh generation and orphan the old one, so a mutate -> query
loop grew the arena quadratically (observed: a 26 GB OOM in pyFF's
`set_entity_attributes` loop). Superseded slots are now recycled through
`Document::attr_node_pool` and overwritten in place; a regression test asserts the arena
stays flat over 100 mutate->prepare rounds.

**pyuppsala: GIL released on heavy ops** (`Python::detach`): `Document()` /
`Document.from_bytes` / `parse` / `parse_bytes`, `Xslt` compile + `transform`,
`XsdValidator` build + `validate` / `validate_str` / `is_valid*`, `Document`/`Node`
`to_xml*` / `write_to_file`. Smoke tests assert two threads parsing genuinely overlap in
wall-clock.

**pyuppsala: native object model completed.**
- `_DocHolderBase` pyclass: the identity proxy cache in Rust. Three-phase
  lookup/construct/insert so the `RefCell` borrow is never held across a call into
  Python (proxy construction can trigger GC, which can re-enter the cache). Native
  `repoint_subtree` preserves wrapper identity across cross-document moves, taking the
  two document mutexes in the same fixed global order as `import_subtree`.
- `ProxyDescendantIterator`: `_Element.iter()` returns a native iterator whose
  `__next__` finds the next match under one lock and finishes in the proxy cache.
  Native `getparent` and `_children_proxies` (backing `__iter__`/`__getitem__`).
- Interned Clark-tag table on the holder (design principle 2).
- `Node` freelist (2048 slots).

**uppsala: serializer overhaul** (all byte-compatible -- gated by golden-file equality on
a 7 MB SAML aggregate for compact, indented, and fragment output):
- Piecewise QName writing for open and close tags: no `format!("{prefix}:{local}")`
  allocation per prefixed element or attribute; same sanitization semantics
  (`safe_xml_qname`) applied piecewise.
- Attribute-name uniqueness tracking switched to `Cow`s: a valid, unused name (every
  attribute of every parsed document) is recorded as a borrow -- the old code paid two
  `String` allocations per attribute per serialize.
- Per-element `children` `Vec` replaced by sibling-link walks (`first_child` /
  `next_sibling`); the pretty-print content probe only runs when indenting.
- **Run-based + SIMD escaping for the DOM serializer.** The old escaper made a virtual
  `write_char` call per character -- 38% of whole-document serialization in perf. Now an
  SSE2 scan (`simd::scan_escape_run`) finds the longest verbatim-copyable run (16
  bytes/cycle) and bulk-writes it; only the rare special byte is handled individually.
  Scalar fallback keeps non-x86_64 correct.
- ASCII fast path for `is_valid_xml_ncname` and a byte-level colon split for
  `is_valid_xml_qname` (these predicates run per name per serialize).

Local effect: `tostring_whole` on the 7 MB aggregate 88 ms -> ~36 ms (3.3x -> ~1.6x of
lxml).

**pyuppsala: batch + network APIs (new).**
- `parse_many(items, *, max_threads=None, **parse_kwargs)`: parses a whole list of
  str/bytes on a scoped native thread pool under one GIL release (work-stealing index, a
  parser per worker, same encoding auto-detection as `parse_bytes`). Returns an
  index-aligned list; per-item failures come back as exception *objects*, never a
  wholesale raise. `etree.fromstring_many` wraps it with the standard parser-option
  mapping and returns root elements.
- `fetch_many` / `fetch_and_parse_many` (+ frozen `FetchResult`): concurrent HTTP(S) GET
  in Rust (ureq + rustls + gzip) with the GIL released for the whole batch;
  `verify_tls=False` supported; `file://` read natively; retries with exponential
  backoff; non-2xx responses are results, not errors. `fetch_and_parse_many` parses each
  response on the worker that fetched it, so bodies never enter Python. Gated behind the
  default-on `net` cargo feature: `maturin build --no-default-features` produces a
  network-free extension (verified to build and import; `pyuppsala._HAS_NET` reflects it).

### Cycle 13 (2026-07-04) -- fastree bulk scans

**pyuppsala: native pull-backed `iterparse` baseline.** `etree.iterparse()` reads the
full input before iteration, collects owned pull-parser events, then replays those
events into the backing document while yielding cached `_Element` proxies. Parser
options (`remove_comments`, `remove_pis`, `strip_cdata`) apply during native replay,
before skipped nodes allocate Python proxies. The upfront syntax scan and each replay
batch run with the GIL released, but this is not a streaming-memory implementation by
default: the document tree grows as events are replayed unless callers clear/detach
completed elements or drop the iterator/document.

`Element.clear()` is intentionally lxml-compatible: detached children and tail text
remain valid if Python or low-level `Node` handles still reference them. In the current
arena-backed DOM that means `clear()` unlinks nodes but does not scrub detached subtree
payloads for memory reclamation; memory is released when the owning document/iterator is
dropped. `XMLParser(compact=True)` only drops the retained source buffer after parsing.

**pyuppsala: Rust-bulk extension methods.** `_Element.fast_count(tag=None)`,
`_Element.fast_sum_int_attr(key, tag=None)` and
`_Element.fast_collect_attr(key, tag=None)` run the whole descendant walk under one
document lock with the GIL released. They reuse the same tag semantics as
`Element.iter()`, but do not materialise one Python `_Element` per match. `fast_count`
and `fast_sum_int_attr` allocate no per-node Python objects; `fast_collect_attr` still
allocates the returned strings/list because that is the requested result.

Current perf diagnosis: plain lxml-shaped Python loops remain slower mainly because
every matching node crosses the extension boundary and often touches proxy creation,
`.tag`, `.get()` or Python allocation. The bulk methods show the expected direction:
when the loop body stays in Rust, pyuppsala is at or ahead of lxml for count/tag/sum
work on the generated corpus.

## Current numbers

### fastree generated corpus checkpoint

Command: `uv run python benchmarks/fastree_bench.py --budget 0.35` after
`uv run maturin develop --release`. Environment: CPython 3.14, lxml 6.1.1,
pyuppsala `fastree` branch using local `../uppsala`.

Ratio = pyuppsala / lxml wall; lower is better. `py fast` is the Rust-bulk extension
method where one exists.

corpus: 5,000 items, 0.58 MiB

| operation | pyuppsala | py fast | lxml | py/lxml | fast/lxml |
|---|---:|---:|---:|---:|---:|
| `fromstring` | 9.840 ms | - | 8.513 ms | 1.16x | - |
| `count iter(item)` | 2.046 ms | 0.297 ms | 0.600 ms | 3.41x | 0.50x |
| `count iter()+tag` | 8.334 ms | 0.297 ms | 2.563 ms | 3.25x | 0.12x |
| `sum int get(id)` | 3.684 ms | 0.376 ms | 1.893 ms | 1.95x | 0.20x |
| `collect get(id)` | 3.276 ms | 0.708 ms | 1.429 ms | 2.29x | 0.50x |
| `attrib items` | 4.696 ms | - | 2.871 ms | 1.64x | - |
| `iterparse end tag clear` | 15.554 ms | - | 14.248 ms | 1.09x | - |

corpus: 25,000 items, 2.98 MiB

| operation | pyuppsala | py fast | lxml | py/lxml | fast/lxml |
|---|---:|---:|---:|---:|---:|
| `fromstring` | 64.418 ms | - | 47.021 ms | 1.37x | - |
| `count iter(item)` | 12.089 ms | 3.867 ms | 3.491 ms | 3.46x | 1.11x |
| `count iter()+tag` | 42.931 ms | 3.791 ms | 13.588 ms | 3.16x | 0.28x |
| `sum int get(id)` | 20.792 ms | 4.558 ms | 11.076 ms | 1.88x | 0.41x |
| `collect get(id)` | 20.859 ms | 6.308 ms | 8.243 ms | 2.53x | 0.77x |
| `attrib items` | 27.314 ms | - | 17.503 ms | 1.56x | - |
| `iterparse end tag clear` | 100.829 ms | - | 73.571 ms | 1.37x | - |

### vs lxml, per operation (dev box, `benchmarks/etree_bench.py`, 1,032-entity / 7.1 MB SAML aggregate)

Ratio = pyuppsala / lxml wall; lower is better. Run-to-run wobble is +/-15%.

| Operation | ratio | note |
|-----------|------:|------|
| `parse_aggregate` / `parse_entities` | **0.75-0.95x** | ahead of lxml (SIMD parser) |
| `has_tag_per_entity` | **~1.0x** | find-first via native iter |
| `with_entity_attributes` | **~1.1-1.5x** | |
| `iter_entitydescriptor` | **~1.4x** | |
| `with_tree` | **~1.5x** | was 4-12x before the native model |
| `tostring_whole` / `tostring_per_entity` | **~1.5-1.8x** | was 3-4x before Cycle 12 |
| `build_aggregate` | **~2.2-2.8x** | cross-doc import + ns planning; next lever |
| `xpath_ns` | **~3.7-4.7x** | evaluator-side; next lever |
| `findall_predicate` | **~6x** | stdlib ElementPath driving native iter (small absolute ms) |
| `attr_get` | **~5-6x** | Python wrapper (small absolute ms) |

### Batch / parallel (no lxml equivalent; `benchmarks/parallel_bench.py`, 12-core dev box)

| Model | wall | vs lxml sequential |
|-------|-----:|-------------------:|
| lxml sequential `fromstring` x1032 | 59.5 ms | 1.0x |
| pyuppsala sequential | 39.1 ms | 1.5x |
| `parse_many(max_threads=4)` | 19.8 ms | 3.0x |
| `parse_many(auto)` | **18.7 ms** | **3.2x** |
| `fetch_many(8)`, 1032 URLs local server | 1.03 s | 2.1x vs requests+ThreadPool(8) (2.17 s) |
| `fetch_and_parse_many(8)` | 1.08 s | fetch + all 1032 parses for +50 ms |

Note the harness measures threaded lxml too (lxml also releases the GIL during parse);
Python-thread pools barely help either backend on many small documents -- the win comes
from the single-call native batch.

### Real-world (4-vCPU host, see `pyFF/performance.md` Cycle 12 for full context)

- SWAMID production-like full-sign (720 entities, parse -> XSLT -> sign RSA-4096 ->
  publish, output verifies): **~8.1 s wall, ~525 MB peak RSS**.
- Full eduGAIN build (6 live feeds, 98 MB fetched, 10,342 entities aggregated, signed
  102 MB output, cold cache): **~38-40 s wall, ~3.2 GB peak RSS**.
- Native fetch vs requests over the 6 eduGAIN feeds: fetch phase **0.55 s vs 1.08 s
  (2x)**; full load pipeline 13.2 s vs 14.1 s.

## Reproducing

```bash
# per-operation vs lxml (needs lxml installed; corpus defaults to the pyFF test aggregate)
uv run python benchmarks/etree_bench.py [--json out.json]

# batch/parallel ingest (add --fetch for the HTTP rows against a local server)
uv run python benchmarks/parallel_bench.py [--reps 3] [--fetch]

# fastree Python-loop vs Rust-bulk microbenchmarks
uv run python benchmarks/fastree_bench.py [--budget 0.35] [--json out.json]

# serializer byte-compat gate: parse + serialize the corpus and diff against a golden copy
# (regenerate goldens BEFORE a serializer change, compare AFTER)

# profiling the native side
perf record -g -- .venv/bin/python <script>; perf report --no-children
```

Suite gates: `uv run pytest tests/ -q` (444 tests, including the lxml-differential set
in `tests/test_etree.py`) and, in `../uppsala`, `cargo test`. Serializer changes must
additionally keep golden serialization byte-identical, since consumers (pyFF) re-parse
and sign fragments.

## Known remaining gaps / next levers

- `build_aggregate` (~2.2-2.8x): cross-document import still re-plans namespaces per
  element and copies strings; a string-pool arena ("forest" arena, plan D4) would make
  moves near-memcpy but is a large change -- do it only if profiling demands.
- `xpath_ns` (~3.7-4.7x): uppsala evaluator-side (axis iteration, node-set handling).
- `findall` predicates still go through the stdlib ElementPath driver.
- Per-getter mutex + FFI cost (plan D5) is the structural floor under everything
  (~1.0-1.5x ratios); revisiting the `Arc<Mutex<>>` scheme is the only way below it.
- uppsala XSD choice validation has a known gap (`uppsala/xsd_bug.md`) that blocks
  schema validation of some real-world metadata; unrelated to performance but caps what
  the pyFF benchmark exercises (validation is skipped there).
