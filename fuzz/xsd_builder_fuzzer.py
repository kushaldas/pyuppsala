#!/usr/bin/env python
"""Fuzz XSD schema building + validation: ``pyuppsala.XsdValidator``.

CVE family G / spec section 4. The highest-value memory-safety CVEs in libxml2
are XSD identity-constraint bugs (CVE-2024-56171 UAF, CVE-2025-32415 heap
under-read in ``xmlSchemaIDCFillNodeTables``, CVE-2025-49796 type confusion).
Uppsala re-implements that surface in Rust: identity constraints
(key/keyref/unique), substitution groups, wildcards, restriction/extension
chains, list types, 44+ built-ins and facets.

Input layout: first line-feed splits (schema, instance-document). The schema is
fed to ``XsdValidator`` (string form -- deliberately NO ``from_file``, so this
harness does zero I/O; the include/import resolver is fuzzed separately in
``fuzz_xsd_from_file``). If the schema builds, the instance document is
validated against it, driving the identity-constraint node tables.

Oracle: a hostile or malformed schema must raise ``XsdValidationError`` /
``XMLSchemaParseError``; recursive type definitions must terminate (no infinite
loop / stack overflow); validation must never panic. ``-timeout`` and
``-rss_limit_mb`` catch schema-as-DoS.
"""

import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc


@atheris.instrument_func
def TestOneInput(data: bytes):
    head, tail = hc.split_two(data, b"\n")
    schema = hc.as_text(head)
    instance = hc.as_text(tail) or "<a/>"
    if schema is None:
        return

    validator = hc.guard(lambda: hc.pyuppsala.XsdValidator(schema))
    if validator is not None:
        # Validate the instance -- drives identity-constraint tables, the
        # libxml2-CVE analog. is_valid_str returns bool or raises a doc error.
        hc.guard(lambda: validator.is_valid_str(instance))


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
