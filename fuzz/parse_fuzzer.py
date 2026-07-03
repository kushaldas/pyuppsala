#!/usr/bin/env python
"""Fuzz the core string parser: ``pyuppsala.parse`` + ``etree.fromstring``.

Primary surface for the CVE families:
  * C (non-billion-laughs DoS) -- deep element nesting, oversized tokens,
    pathological attribute lists. Resource limits live in the library; the
    harness just feeds input and lets libFuzzer's ``-timeout`` / ``-rss_limit_mb``
    observe a hang or blowup.
  * D (memory safety / "no panic on untrusted input") -- any Rust ``panic!``
    crossing PyO3 becomes a ``PanicException`` and escapes the oracle; native
    faults are caught by ASan when built with it.
  * I (namespace handling) -- prefix rebinding, reserved prefixes, colon edge
    cases, all reachable from raw markup.

Oracle: only the documented malformed-input exceptions are swallowed (see
``harness_common``). Anything else is a finding.
"""

import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc


@atheris.instrument_func
def TestOneInput(data: bytes):
    text = hc.as_text(data)
    if text is None:
        return  # invalid UTF-8 belongs to fuzz_parse_bytes
    # Native parser (owns the DOM + namespace resolution).
    doc = hc.guard(lambda: pyuppsala_parse(text))
    if doc is not None:
        # Touch the tree so lazy work (doctype, root) actually runs.
        hc.guard(lambda: (doc.doctype, doc.root))
    # etree facade parses through the same core but adds the proxy/tag layer.
    hc.guard(lambda: hc.ET.fromstring(text))


def pyuppsala_parse(text):
    return hc.pyuppsala.parse(text)


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
