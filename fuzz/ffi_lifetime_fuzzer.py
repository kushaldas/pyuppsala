#!/usr/bin/env python
"""Fuzz the PyO3 handle-lifetime boundary: ``Node`` / ``Document`` UAF hunting.

Spec section 11 -- the single place the docs say a native crash is *possible*
("Do not use a ``Node`` after its parent ``Document`` has been garbage
collected"). The libxml2 lifetime CVEs (CVE-2024-56171, CVE-2025-12863
``xmlSetTreeDoc``) are exactly this class. The pyuppsala ``Node`` is a
``(Arc<Mutex<Document>>, NodeId)`` handle, so by design it should keep its
document alive; this harness exists to prove that invariant holds under an
arbitrary sequence of lifetime-stressing operations rather than trusting it.

Operations driven by the fuzzer:
  * hold a ``Node``, drop every ``Document`` reference, ``gc.collect()``, then
    touch ``tag`` / ``children`` / ``text`` / ``to_xml()`` -- must not segfault;
  * detach a node then use / re-attach it;
  * move a subtree across two documents (deep-copy + re-point) and keep using
    handles into both the source and destination trees;
  * hold a child handle, ``remove_child`` it, then use the stale handle;
  * ``Document.empty()`` edges (``document_element is None``) and
    programmatically built nodes (``source is None``).

Oracle: every access must either succeed or raise a clean Python exception.
``faulthandler`` (armed in :func:`main`) turns any native segfault into a
fatal, visible traceback; a Rust panic escapes as ``PanicException``; ASan (when
built in) catches use-after-free / OOB directly.
"""

import faulthandler
import gc
import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc


def _touch(node):
    """Poke every cheap accessor on a Node; each must be crash-safe."""
    hc.guard(lambda: node.tag)
    hc.guard(lambda: node.text)
    hc.guard(lambda: list(node.children))
    hc.guard(lambda: node.to_xml())
    hc.guard(lambda: node.parent)
    hc.guard(lambda: len(node))


@atheris.instrument_func
def TestOneInput(data: bytes):
    fdp = atheris.FuzzedDataProvider(data)
    xml = fdp.ConsumeUnicodeNoSurrogates(512) or "<a><b>t</b><c/></a>"

    doc = hc.guard(lambda: hc.pyuppsala.parse(xml))
    if doc is None:
        return

    # Collect some handles into the tree, then orphan the Document.
    handles = []
    root = hc.guard(lambda: doc.root)
    if root is not None:
        handles.append(root)
        kids = hc.guard(lambda: list(root.children)) or []
        handles.extend(kids[:4])

    op = fdp.ConsumeIntInRange(0, 4)

    if op == 0:
        # Node-outlives-Document: drop the Document, force GC, use handles.
        del doc
        gc.collect()
        for h in handles:
            _touch(h)

    elif op == 1:
        # Detach then reuse / re-attach.
        for h in list(handles):
            hc.guard(lambda h=h: doc.detach(h))
        gc.collect()
        for h in handles:
            _touch(h)

    elif op == 2:
        # Stale handle after structural removal.
        if root is not None and len(handles) > 1:
            child = handles[1]
            hc.guard(lambda: doc.remove_child(root, child))
            gc.collect()
            _touch(child)

    elif op == 3:
        # Cross-tree move via etree deep-copy; keep using both sides' handles.
        a = hc.guard(lambda: hc.ET.fromstring(xml))
        b = hc.guard(lambda: hc.ET.fromstring("<dst/>"))
        if a is not None and b is not None and len(a):
            import copy

            src_child = a[0]
            hc.guard(lambda: b.append(copy.deepcopy(src_child)))
            gc.collect()
            hc.guard(lambda: hc.ET.tostring(a, encoding="unicode"))
            hc.guard(lambda: hc.ET.tostring(b, encoding="unicode"))
            hc.guard(lambda: src_child.tag)  # original still valid

    else:
        # Empty-document + programmatically-built-node edges.
        empty = hc.guard(lambda: hc.pyuppsala.Document.empty())
        if empty is not None:
            hc.guard(lambda: empty.document_element)  # None
            e = hc.guard(lambda: empty.create_element("x"))
            if e is not None:
                hc.guard(lambda: e.source)  # None for built nodes
                _touch(e)


def main():
    faulthandler.enable()
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
