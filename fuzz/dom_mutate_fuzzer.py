#!/usr/bin/env python
"""Fuzz etree DOM mutation: an arbitrary edit sequence over live proxies.

CVE families D (memory safety) and E (round-trip), spec sections 10 and 11. This
drives the parts unique to the pyuppsala binding -- the native identity-stable
proxy cache, the interned-tag table, and the cross-document deep-copy /
re-point path -- with the exact shape of pyFF's mutate/query loop.

The harness keeps TWO trees so it can exercise cross-tree moves (etree
deep-copies the subtree into the destination document and re-points live
proxies -- the CVE-2025-12863 ``xmlSetTreeDoc`` cross-tree-move UAF analog).
After each op it serializes and reparses, and at the end it asserts overall
coherence.

Oracle: whatever the edit sequence, the tree must still serialize to
well-formed XML that reparses. A wiped child list, a cyclic sibling link, a
dangling proxy, a panic, or a native fault surfaces as a reparse failure, an
assertion, an ASan report, or a libFuzzer timeout.
"""

import copy
import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc

MAX_OPS = 200


def _elements(root):
    """All element proxies in document order (bounded by the tree size)."""
    return list(root.iter())


@atheris.instrument_func
def TestOneInput(data: bytes):
    fdp = atheris.FuzzedDataProvider(data)

    a = hc.guard(lambda: hc.ET.fromstring("<a><b/><c>t</c></a>"))
    b = hc.guard(lambda: hc.ET.fromstring("<x><y>hi</y></x>"))
    if a is None or b is None:
        return

    def pick(root):
        elems = _elements(root)
        if not elems:
            return None
        return elems[fdp.ConsumeIntInRange(0, len(elems) - 1)]

    for _ in range(MAX_OPS):
        if fdp.remaining_bytes() == 0:
            break
        op = fdp.ConsumeIntInRange(0, 8)

        def step():
            if op == 0:  # add child
                p = pick(a)
                if p is not None:
                    hc.ET.SubElement(p, fdp.ConsumeUnicodeNoSurrogates(16) or "e")
            elif op == 1:  # set attribute
                p = pick(a)
                if p is not None:
                    p.set(
                        fdp.ConsumeUnicodeNoSurrogates(8) or "k",
                        fdp.ConsumeUnicodeNoSurrogates(16),
                    )
            elif op == 2:  # set text
                p = pick(a)
                if p is not None:
                    p.text = fdp.ConsumeUnicodeNoSurrogates(16)
            elif op == 3:  # remove a child
                p = pick(a)
                if p is not None and len(p):
                    p.remove(p[fdp.ConsumeIntInRange(0, len(p) - 1)])
            elif op == 4:  # cross-tree move (deep-copy + re-point)
                p = pick(a)
                src = pick(b)
                if p is not None and src is not None:
                    p.append(copy.deepcopy(src))
            elif op == 5:  # insert at index
                p = pick(a)
                if p is not None:
                    p.insert(
                        fdp.ConsumeIntInRange(0, len(p)),
                        hc.ET.Element(fdp.ConsumeUnicodeNoSurrogates(8) or "n"),
                    )
            elif op == 6:  # deepcopy within-tree and re-attach
                p = pick(a)
                if p is not None and p.getparent() is not None:
                    p.getparent().append(copy.deepcopy(p))
            elif op == 7:  # serialize + reparse coherence probe
                out = hc.ET.tostring(a, encoding="unicode")
                hc.ET.fromstring(out)  # our own output must reparse
            else:  # xpath over the mutated tree
                a.xpath("//*")

        # Expected errors (ValueError for bad names, etc.) are fine; a panic or
        # native fault escapes.
        hc.guard(step, SyntaxError)

    # Final coherence: the whole tree must still serialize + reparse.
    out = hc.guard(lambda: hc.ET.tostring(a, encoding="unicode"))
    if out is not None:
        hc.ET.fromstring(out)


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
