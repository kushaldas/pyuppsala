# ADR 0001: Keep etree performance hot paths in native Rust

## Status

Accepted

## Context

pyuppsala exposes a largely lxml-compatible `etree` API on top of the Rust
`uppsala` DOM. The compatibility layer is intentionally Python-shaped, but
profiles from the pyFF workload and the local fastree benchmarks showed that
large metadata documents still spent too much time crossing the Python/native
boundary for simple descendant scans, attribute reads, text extraction, parser
cleanup, and subtree copies.

The existing performance work had already moved proxy caching, tag interning,
iteration, parsing, serialization, XSD, XSLT, batch parsing, and native fetch
paths into Rust. The remaining hot paths were not fundamentally XML parsing
problems. They were repeated Python loops over Rust-owned nodes, or post-parse
tree rewrites that could be done once before any Python element proxies were
visible.

## Decision

Keep the lxml-compatible API as the default surface, but add explicit native
bulk paths for workflows where pyuppsala can preserve the same observable
result without materializing one Python element proxy per matching node.

For the 0.9.0 performance work this means:

- Add `_Element.fast_has(tag=None)` as a native short-circuit descendant
  existence check using the same tag semantics as `Element.iter(tag)`.
- Add `_Element.fast_collect_grouped_text(group_tag, item_tag, key, value_tag)`
  for the pyFF SAML EntityAttributes shape. The full grouped descendant walk,
  attribute lookup, leading text collection, and whitespace trimming run under
  one Rust document lock while the GIL is released.
- Apply `XMLParser` post-parse options natively through
  `Document.postprocess_parse_options`. Comment removal, processing instruction
  removal, CDATA stripping, and text coalescing now happen in Rust before
  normal `_Element` proxies are exposed, avoiding Python recursion and
  proxy-cache repair.
- Honor `XMLParser(compact=True)` by discarding the retained decoded input
  buffer after etree parsing through `Document.discard_input`. The DOM remains
  usable, while source-inspection helpers have no retained source text to
  report. Callers that need source snippets can opt out with `compact=False` or
  use the lower-level document APIs that retain input.
- Implement `copy.deepcopy(element)` by importing the subtree into a fresh
  document and copying inherited namespace declarations, rather than
  serializing and reparsing the element.
- Expose the new native methods in the Python type stubs so typed consumers can
  adopt the fast paths deliberately.

## Consequences

- The primary performance rule is now explicit: broad scans and fixed-shape
  extractions should stay in Rust until the final result must become Python
  data. This reduces GIL contention, avoids per-node proxy allocation, and keeps
  large tree rewrites close to the arena representation.
- pyuppsala now has a small set of extension methods in addition to the
  lxml-compatible API. These methods are intentionally narrow and should be
  added only when profiling shows a repeated workload shape that cannot be made
  fast enough through normal `etree` loops.
- `compact=True` changes the memory profile of etree parsing by dropping the
  source buffer for long-lived trees. This is appropriate for lxml-compatible
  callers that inspect the tree, not the original byte ranges. Code that
  depends on source snippets must opt out explicitly.
- Native post-processing happens before proxies escape, so it does not need to
  repair existing wrapper identity. Future parser-option transforms should
  follow the same placement when possible.

## Alternatives Considered

### Make every lxml-shaped Python loop faster implicitly

This keeps the public surface smaller, but the profiling bottleneck is the
per-node transition into Python. The binding cannot remove that cost while still
returning and inspecting each Python proxy in the loop body.

### Keep parser option cleanup in Python

The Python implementation is simpler to inspect, but it repeats tree traversal
in Python after parsing and before steady-state use. Running the same cleanup
natively is lower overhead and avoids proxy-cache concerns.

### Always retain decoded input text

This preserves source inspection by default, but it keeps an extra full input
buffer alive for parsed etree documents. `compact=True` matches the common
lxml-compatible memory expectation, while `compact=False` keeps the source-aware
behavior available.

## References

See the repository-root `PERFORMANCE.md` for benchmark history, measured ratios
against lxml, reproduction commands, and remaining performance gaps.
