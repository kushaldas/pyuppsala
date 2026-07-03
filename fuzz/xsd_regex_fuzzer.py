#!/usr/bin/env python
"""Fuzz XSD regex compile + match: ``pyuppsala.XsdRegex``.

CVE family C (algorithmic DoS), spec section 6. XSD regex adds Unicode
categories/blocks and character-class subtraction on top of ordinary regex, and
is a classic ReDoS surface. Uppsala bounds backtracking with
``DEFAULT_MAX_REGEX_STEPS`` (1e6) and group nesting with
``DEFAULT_MAX_REGEX_GROUP_DEPTH`` (64).

Input layout mirrors uppsala's ``fuzz_xsd_regex.rs``: first line = pattern,
remainder = subject string.

Oracle: a malformed pattern raises (``ValueError`` / documented error); a
compiled pattern's ``is_match`` must terminate under the step cap (``-timeout``
observes any runaway) and must never panic. Catastrophic-backtracking inputs are
expected to return quickly, not hang.
"""

import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc


@atheris.instrument_func
def TestOneInput(data: bytes):
    head, tail = hc.split_two(data, b"\n")
    pat = hc.as_text(head)
    subject = hc.as_text(tail) or ""
    if pat is None:
        return

    re = hc.guard(lambda: hc.pyuppsala.XsdRegex(pat))
    if re is not None:
        hc.guard(lambda: re.is_match(subject))


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
