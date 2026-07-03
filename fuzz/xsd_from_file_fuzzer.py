#!/usr/bin/env python
"""Fuzz XSD external-resource resolution: ``XsdValidator.from_file``.

Spec section 5 -- the nearest thing to XXE/SSRF in uppsala. ``from_file``
resolves ``xs:include`` / ``xs:import`` / ``xs:redefine`` ``schemaLocation``s
relative to a ``base_path``, reading files from disk during schema construction.
The documented risks are path traversal (``../../etc/passwd``, absolute paths,
symlinks), remote fetches (``http://``/``file://`` schemaLocations), and
include cycles / fan-out bombs.

This harness builds a small on-disk schema graph from the fuzz input inside a
throwaway ``base_path`` and calls ``from_file`` on it. It fuzzes for
ROBUSTNESS: an include cycle must be detected and terminate (no infinite loop /
stack overflow -- ``-timeout`` catches a hang), a missing/hostile
``schemaLocation`` must raise cleanly rather than panic, and resolution must
stay bounded in time and memory.

NOTE: proving that resolution never escapes ``base_path`` or never opens a
network socket requires a syscall / opened-files monitor (strace, an ``open``
audit hook, or a Rust ASan+seccomp run) and belongs in the pytest security
suite. A sentinel file is planted OUTSIDE ``base_path`` so that such a monitor,
or a future in-harness ``audit`` hook, has a canary to watch; the fuzzer here
concentrates on the panic/hang/cycle surface.
"""

import os
import shutil
import sys
import tempfile

import atheris

with atheris.instrument_imports():
    import harness_common as hc

# Created once: a canary a syscall monitor can watch for an out-of-base read.
_SENTINEL_DIR = tempfile.mkdtemp(prefix="pyuppsala_fuzz_sentinel_")
_SENTINEL = os.path.join(_SENTINEL_DIR, "SECRET_DO_NOT_READ")
with open(_SENTINEL, "w") as _f:
    _f.write("canary")


@atheris.instrument_func
def TestOneInput(data: bytes):
    fdp = atheris.FuzzedDataProvider(data)
    main_schema = fdp.ConsumeUnicodeNoSurrogates(1024)
    # A couple of satellite schema files the main one may include by name.
    inc_a = fdp.ConsumeUnicodeNoSurrogates(512)
    inc_b = fdp.ConsumeUnicodeNoSurrogates(512)

    base = tempfile.mkdtemp(prefix="pyuppsala_fuzz_base_")
    try:
        # Names the main schema can reference via schemaLocation="a.xsd" etc.
        for fname, content in (("a.xsd", inc_a), ("b.xsd", inc_b)):
            try:
                # utf-8 explicitly: the process locale's default encoding may
                # not represent arbitrary fuzz-derived unicode at all.
                with open(os.path.join(base, fname), "w", encoding="utf-8") as fh:
                    fh.write(content)
            except (OSError, ValueError, UnicodeError):
                # Harness/environment noise (unwritable file, unencodable
                # text), not a library defect -- never a fuzz finding.
                # UnicodeError is a ValueError subclass; listed explicitly
                # for self-documentation.
                pass

        validator = hc.guard(lambda: hc.pyuppsala.XsdValidator.from_file(main_schema, base))
        if validator is not None:
            hc.guard(lambda: validator.is_valid_str("<a/>"))
    finally:
        shutil.rmtree(base, ignore_errors=True)


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
