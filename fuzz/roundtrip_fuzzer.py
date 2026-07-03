#!/usr/bin/env python
"""Parse -> serialize -> reparse -> serialize idempotence over the etree facade.

This is the highest-value security harness. CVE family E (round-trip
instability) is exactly the bug class behind several SAML authentication
bypasses (Go ``encoding/xml``, Ruby REXML, various xmlsec wrappers): a document
whose serialization is NOT a fixpoint lets an attacker craft input that one
component sees one way and a second component (after a re-serialize) sees
another. It also covers:
  * family I (namespace handling) -- prefix rebinding / default-namespace churn
    must survive a round trip,
  * family J (output injection) -- if the serializer failed to escape something,
    the reparse yields a different tree and the fixpoint assertion trips.

Oracle: once a document has been serialized, reparsing and re-serializing it
must reproduce the exact same bytes. The assertion only fires when BOTH the
first serialization reparses AND the second serialization is produced, so
parser resource limits never cause a false positive. A mismatch, a Rust panic,
or a native fault is a finding.

Mirrors uppsala's ``fuzz_roundtrip.rs`` at the Python/etree layer, so it also
exercises the proxy cache and interned-tag table on the second parse.
"""

import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc


def _tostring(elem, **kw):
    return hc.ET.tostring(elem, encoding="unicode", **kw)


@atheris.instrument_func
def TestOneInput(data: bytes):
    text = hc.as_text(data)
    if text is None:
        return

    root = hc.guard(lambda: hc.ET.fromstring(text))
    if root is None:
        return

    # --- Compact serialization must be a fixpoint. ---
    out1 = hc.guard(lambda: _tostring(root))
    if out1 is None:
        return
    root2 = hc.guard(lambda: hc.ET.fromstring(out1))
    if root2 is not None:
        out2 = _tostring(root2)  # NOT guarded: our own output must serialize
        assert out1 == out2, "compact serialization is not idempotent"

    # --- Pretty-printed path is a different serializer branch (element-only
    # probe + indentation); it must also round-trip to a fixpoint. ---
    pretty = hc.guard(lambda: _tostring(root, pretty_print=True))
    if pretty is not None:
        rootp = hc.guard(lambda: hc.ET.fromstring(pretty))
        if rootp is not None:
            pretty2 = _tostring(rootp, pretty_print=True)
            assert pretty == pretty2, "pretty serialization is not idempotent"


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
