# ADR 0005: Keep the anti-DoS XPath visit budget; trusted callers raise a module knob

## Status

Accepted

## Context

uppsala bounds every XPath evaluation with a node-visit budget
(`DEFAULT_MAX_XPATH_NODE_VISITS`, 100,000 visits) so a hostile document or
expression cannot pin the CPU. pyuppsala's `etree.xpath()` passes that budget
through, which is the right default for untrusted input.

Real-world SAML aggregates blow straight past it: an eduGAIN aggregate is
roughly one million nodes, so even a plain `//md:EntityDescriptor` legitimately
exceeds 100,000 visits. pyFF hit this in practice: three test-suite failures
whose shared symptom was "XPath evaluation exceeded maximum node visit budget"
on a 525k-node working document, and any user pipeline with an XPath `select`
would hit it at production scale.

Options considered:

- Raise uppsala's default. Rejected: the default protects every consumer that
  evaluates XPath over genuinely untrusted documents, and no single number is
  right for both a SOAP endpoint and a 10k-entity metadata aggregator.
- Have the etree facade silently drop the cap. Rejected for the same reason;
  the facade is used on untrusted input too.
- Per-call arguments everywhere. Correct but invasive; pyFF alone has dozens
  of xpath call sites, and third-party pipeline code cannot be patched.

## Decision

Keep the native default untouched and expose the budget as a module-level
knob, `pyuppsala.etree.MAX_XPATH_NODE_VISITS`, initialized to the native
default. `etree.xpath()` (and the XPath facades) read it per evaluation.
Applications that evaluate XPath only over their own large, semi-trusted
working documents raise it once at import time.

pyFF sets it in `pyff.utils`:

```python
if hasattr(etree, 'MAX_XPATH_NODE_VISITS'):
    etree.MAX_XPATH_NODE_VISITS = max(etree.MAX_XPATH_NODE_VISITS, 50_000_000)
```

Fifty million visits comfortably covers multi-million-node aggregates while
still bounding a runaway evaluation to something that terminates.

## Consequences

- Security posture is unchanged for consumers that do nothing: the 100k cap
  still applies out of the box.
- The knob is process-global by design. That is acceptable for applications
  like pyFF that own their process; libraries embedded in someone else's
  process should prefer per-evaluator `max_node_visits` instead.
- Hot fixed-shape scans should not go through XPath at all: the `fast_count`,
  `fast_has`, and `fast_collect_*` native helpers bypass both the budget and
  the per-node Python cost, and pyFF's `stats` pipe uses them.
