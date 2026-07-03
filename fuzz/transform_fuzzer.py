#!/usr/bin/env python
"""Fuzz the XSLT engine: ``pyuppsala.Xslt`` / ``etree.XSLT``.

CVE family G (DTD / feature abuse): XSLT is the richest "feature" surface -- it
drives the compiled-XPath evaluator, template recursion, and the result-tree
serializer (including the SIMD escaper) in one shot. Bounded XSLT recursion
depth means a legitimately deep or recursive stylesheet returns an error rather
than overflowing the stack; the harness checks that promise holds under
adversarial stylesheets.

Input is split on a NUL byte into (stylesheet, source). NUL never appears in
well-formed XML text, so it is an unambiguous separator libFuzzer can discover
and preserve -- same convention as uppsala's ``fuzz_transform.rs``.

Oracle: a documented error is fine; a panic / native fault / hang is a finding.
The transform output (when produced) is reparsed to exercise the parser on
machine-generated markup.
"""

import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc


@atheris.instrument_func
def TestOneInput(data: bytes):
    style_b, src_b = hc.split_two(data, b"\0")
    if not src_b:
        return
    style = hc.as_text(style_b)
    src = hc.as_text(src_b)
    if style is None or src is None:
        return

    xslt = hc.guard(lambda: hc.pyuppsala.Xslt(style))
    if xslt is None:
        return
    out = hc.guard(lambda: xslt.transform(src))
    if out is not None:
        hc.guard(lambda: hc.pyuppsala.parse(out))


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
