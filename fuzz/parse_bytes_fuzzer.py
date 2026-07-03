#!/usr/bin/env python
"""Fuzz the byte parser + encoding detector: ``pyuppsala.parse_bytes``.

Primary surface for CVE family H (encoding / character):
  * charset confusion -- a declared ``encoding=`` that disagrees with a BOM,
  * UTF-16 with and without BOM, odd trailing bytes, truncated multibyte
    sequences,
  * invalid UTF-8 that must be rejected cleanly rather than mis-decoded,
  * numeric/character-reference edge cases in the raw byte stream.

Unlike ``fuzz_parse`` this harness does NOT gate on valid UTF-8 -- feeding the
decoder raw bytes is the whole point. It also runs the bytes through the etree
facade, whose ``fromstring`` accepts bytes and re-detects the encoding.

Oracle: documented malformed-input errors only; a panic / native fault / hang is
a finding.
"""

import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc


@atheris.instrument_func
def TestOneInput(data: bytes):
    doc = hc.guard(lambda: hc.pyuppsala.parse_bytes(data))
    if doc is not None:
        hc.guard(lambda: (doc.doctype, doc.input_text))
    hc.guard(lambda: hc.ET.fromstring(data))


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
