# ADR 0004: Native-document XSLT input with a byte-identity guard

## Status

Accepted

## Context

`etree.XSLT.__call__` serialized its input to a string and handed it to the
native `Xslt.transform`, which re-parsed that string before running the
stylesheet. uppsala's engine has had `Stylesheet::transform(&Document)` all
along; pyuppsala simply did not expose it. For pyFF's tidy transform of a
100 MB aggregate the round trip cost one full serialization, one full parse
(plus its arena and virtual attribute nodes), and hundreds of MiB of transient
peak memory, at the worst-stacked point of the pipeline.

The subtlety is that the two paths are not universally equivalent. The string
path serializes exactly the input element (or tree), so the engine never sees
a DOCTYPE or document-level comments and processing instructions; a
whole-document transform would. A stylesheet that matches comments would
produce different output for the same call.

## Decision

Expose `Xslt.transform_document(document)`: lock the shared document, run
`prepare_xpath()`, and apply the compiled stylesheet directly to the live DOM
under a released GIL. The source document is prepared for XPath as a side
effect but not otherwise mutated.

`etree.XSLT.__call__` uses it automatically, but only when transforming the
input is provably equivalent to transforming its whole document:

- the input is an `_ElementTree` or an `_Element` with no parent (the document
  root);
- the document has no DOCTYPE;
- the root element is the document's sole top-level node (no document-level
  comments or processing instructions).

Anything else falls back to the string path unchanged. The guard lives in
`XSLT._whole_document_source` and is unit-tested for each disqualifier.

## Consequences

- Whole-document transforms (the pyFF shape) skip one full serialization and
  one full parse per call; the XSLT stage's transient peak on an 83 MB
  aggregate dropped by roughly 200 MiB, and output is byte-identical to the
  string path by construction of the guard.
- The transform result is still a serialized string that `_XSLTResultTree`
  re-parses lazily. Returning a native result document instead would need an
  uppsala engine change and is a candidate for the next stage.
- Repeated transforms re-run `prepare_xpath` on the source document; the
  virtual attribute nodes it builds are recycled in place by uppsala's
  attribute node pool, so the arena does not grow across calls.
