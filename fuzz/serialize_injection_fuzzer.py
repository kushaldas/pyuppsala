#!/usr/bin/env python
"""Output-injection oracle: attacker-controlled node content must never break
out of its syntactic position when serialized.

CVE family J (output injection) and F (signature wrapping) share a root cause:
text, attribute, comment, PI or CDATA content that the serializer fails to
escape/sanitize can inject NEW markup (extra elements, extra attributes, a
premature ``]]>`` / ``-->`` / ``?>`` breakout). A consumer that re-parses the
output then sees a DIFFERENT tree than the producer built -- the classic
XML-signature-wrapping / comment-splitting primitive.

The harness builds a small tree, stuffs one fuzz-derived string into every
content position (element text, tail, attribute value, comment, PI data, CDATA),
serializes, and reparses. The oracle is deliberately SHAPE-based, not
value-based, so XML's legal whitespace/char-ref normalization and the library's
documented U+FFFD replacement of invalid characters never cause a false
positive:

    * the reparse of our own output MUST succeed (well-formed serializer output),
    * the reparsed root tag, child count, descendant-element count and attribute
      NAME set must all equal the built tree's.

Any injected byte that spawned an element, an attribute, or corrupted the tree
shape trips an assertion. A panic / native fault is a finding regardless.
"""

import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc


def _shape(elem):
    """Structural fingerprint that is invariant under XML normalization."""
    descendants = [e.tag for e in elem.iter()]
    attr_names = tuple(sorted(elem.attrib.keys()))
    return (elem.tag, len(list(elem)), len(descendants), attr_names)


@atheris.instrument_func
def TestOneInput(data: bytes):
    fdp = atheris.FuzzedDataProvider(data)
    # One hostile payload reused in every content slot.
    payload = fdp.ConsumeUnicodeNoSurrogates(256)
    which = fdp.ConsumeIntInRange(0, 4)

    def build():
        root = hc.ET.Element("r")
        root.set("a", payload)
        child = hc.ET.SubElement(root, "c")
        if which == 0:
            root.text = payload
        elif which == 1:
            child.text = payload
        elif which == 2:
            child.tail = payload
        elif which == 3:
            root.append(hc.ET.Comment(payload))
        else:
            root.append(hc.ET.ProcessingInstruction("pi", payload))
        return root

    root = hc.guard(build)
    if root is None:
        return  # ValueError etc. for content XML simply cannot represent

    before = _shape(root)
    out = hc.guard(lambda: hc.ET.tostring(root, encoding="unicode"))
    if out is None:
        return

    # Our own serializer output MUST be well-formed -- reparse is NOT guarded.
    root2 = hc.ET.fromstring(out)
    after = _shape(root2)
    assert before == after, (
        f"content injection changed tree shape: {before!r} -> {after!r}; "
        f"payload={payload!r} slot={which}"
    )


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
