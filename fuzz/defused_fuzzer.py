#!/usr/bin/env python
"""Fuzz the defused-XML security knobs and pin the safe-by-default posture.

CVE families A (XXE), B (entity-expansion DoS) and G (DTD abuse); spec sections
0 and 1. Uppsala is safe-by-default: it does not load external entities, it caps
internal entity expansion, and it preserves ``<!DOCTYPE>`` verbatim without
acting on it. The parser also accepts ``forbid_dtd`` / ``forbid_entities``
override knobs. This harness fuzzes those knobs and asserts two invariants that,
if broken, are real security regressions:

  1. forbid_dtd bypass: if the DEFAULT parse recognised a DOCTYPE
     (``doc.doctype is not None``), then reparsing the SAME bytes with
     ``forbid_dtd=True`` MUST raise. A returned document is a bypass = finding.
     Gating on the default parse's own doctype detection makes this
     false-positive-free -- a ``<!DOCTYPE`` buried in a comment/CDATA is not a
     real doctype and is correctly ignored by both parses.

  2. forbid_entities bypass: if that recognised DOCTYPE string contains an
     ``<!ENTITY`` declaration, then ``forbid_entities=True`` MUST raise.

Billion-laughs / quadratic-expansion DoS is covered structurally: the default
parse must terminate (``-timeout``) and stay within memory (``-rss_limit_mb``);
no expansion bomb should get past the 1 MiB ``max_entity_expansion`` cap.

A one-time :func:`_check_posture` pins the documented cap CONSTANTS at startup so
a silent value regression fails loudly before fuzzing even begins.
"""

import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc

# Documented defaults (spec section 0). A regression here re-enables a DoS class.
_EXPECTED_CONSTANTS = {
    "DEFAULT_MAX_DEPTH": 128,
    "DEFAULT_MAX_ENTITY_EXPANSION": 1048576,
    "DEFAULT_MAX_ENTITY_DEPTH": 256,
    "DEFAULT_MAX_XPATH_DEPTH": 32,
    "DEFAULT_MAX_XPATH_NODE_VISITS": 100000,
    "DEFAULT_MAX_REGEX_GROUP_DEPTH": 64,
    "DEFAULT_MAX_REGEX_STEPS": 1000000,
}


def _check_posture():
    U = hc.pyuppsala
    for name, want in _EXPECTED_CONSTANTS.items():
        got = getattr(U, name, None)
        assert got == want, f"cap regression: {name} is {got}, expected {want}"


@atheris.instrument_func
def TestOneInput(data: bytes):
    text = hc.as_text(data)
    if text is None:
        return

    # Default (safe) parse. Must terminate + stay bounded -- billion laughs and
    # quadratic-expansion inputs are stopped by the entity-expansion cap.
    doc = hc.guard(lambda: hc.pyuppsala.parse(text))
    if doc is None:
        return

    doctype = hc.guard(lambda: doc.doctype)
    if not doctype:
        return  # no recognised DOCTYPE -> the DTD/entity knobs have nothing to gate

    # Invariant 1: forbid_dtd must reject a document that really has a DOCTYPE.
    forbidden = hc.guard(lambda: hc.pyuppsala.parse(text, forbid_dtd=True))
    assert forbidden is None, "forbid_dtd=True accepted a document with a DOCTYPE"

    # Invariant 2: forbid_entities must reject a DOCTYPE that declares entities.
    if "<!ENTITY" in doctype:
        no_ent = hc.guard(lambda: hc.pyuppsala.parse(text, forbid_entities=True))
        assert no_ent is None, "forbid_entities=True accepted a document declaring entities"


def main():
    _check_posture()
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
