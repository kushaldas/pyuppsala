"""Shared oracle helpers for the pyuppsala Atheris harnesses.

The whole point of a fuzz oracle is to draw a sharp line between two outcomes:

  * EXPECTED  -- the library rejecting malformed / hostile input by raising one
    of its documented exceptions. This is correct behaviour and must be
    swallowed, otherwise every random byte string would be reported as a
    "crash" and drown the real findings.
  * FINDING   -- anything else. A Rust ``panic!`` crossing the PyO3 boundary
    surfaces in Python as ``pyo3_runtime.PanicException`` (which subclasses
    ``BaseException``, NOT ``Exception``, so it slips past ``except Exception``
    and propagates straight out of the harness -- exactly what we want). A
    native memory-safety fault is caught by AddressSanitizer when the extension
    was built with it. A hang is caught by libFuzzer's ``-timeout``. Unbounded
    memory growth is caught by ``-rss_limit_mb``. And any stray Python-level
    ``KeyError``/``IndexError``/``AttributeError`` from the pure-Python
    ``etree`` layer is a real logic bug we let propagate.

So the rule for every harness is: wrap the target in :func:`guard` (or the
:data:`EXPECTED` tuple) which absorbs ONLY the documented malformed-input
errors, and let everything else escape to Atheris.

This mirrors uppsala's Rust fuzz targets, whose oracle is "any ``panic!`` is a
finding; a returned ``XmlError`` is not". Here the returned-error set is the
Python exception hierarchy that pyuppsala documents for bad input.
"""

from __future__ import annotations

import pyuppsala
from pyuppsala import etree as ET

# Documented malformed-input errors from the native extension. Every one of
# these means "pyuppsala looked at your bytes and correctly refused them".
_NATIVE_ERRORS = (
    pyuppsala.XmlParseError,
    pyuppsala.XmlWellFormednessError,
    pyuppsala.XmlNamespaceError,
    pyuppsala.XPathError,
    pyuppsala.XsdValidationError,
)

# The etree facade wraps the native errors in lxml-named classes. ``LxmlError``
# is their common base (XMLSyntaxError, ParseError, XPathError, XPathEvalError,
# XPathSyntaxError, DocumentInvalid, XMLSchemaParseError all descend from it),
# so catching the base covers the whole facade surface.
_ETREE_ERRORS = (ET.LxmlError,)

# Argument / decoding errors that arise from feeding fuzz bytes straight into an
# API that expects, say, a valid encoding name or a str. These are contract
# violations by the harness input, not library defects.
_ARG_ERRORS = (
    ValueError,
    TypeError,
    UnicodeError,       # base of UnicodeDecodeError / UnicodeEncodeError
    NotImplementedError,  # etree deliberately raises for unsupported options
)

#: Exceptions a harness may swallow without it counting as a finding.
EXPECTED = _NATIVE_ERRORS + _ETREE_ERRORS + _ARG_ERRORS


def guard(fn, *extra):
    """Run ``fn()`` swallowing :data:`EXPECTED` (plus any ``extra`` types).

    Returns ``fn()``'s value on success, or ``None`` if an expected error was
    raised. Any exception NOT in the allowed set propagates to Atheris and is
    reported as a crash. ``PanicException`` is never in the set (it is a
    ``BaseException``), so Rust panics always escape.
    """
    allowed = EXPECTED + tuple(extra)
    try:
        return fn()
    except allowed:
        return None


def split_two(data: bytes, sep: bytes):
    """Split fuzz input into two parts on the first ``sep`` occurrence.

    Used by the two-input harnesses (expr + document, pattern + input,
    stylesheet + source). Returns ``(head, tail)`` as bytes; ``tail`` is empty
    when the separator is absent.
    """
    idx = data.find(sep)
    if idx < 0:
        return data, b""
    return data[:idx], data[idx + len(sep):]


def as_text(data: bytes) -> str | None:
    """Decode ``data`` as strict UTF-8, or ``None`` if it is not valid UTF-8.

    Harnesses that take a ``str`` (parse, fromstring, xpath expressions) use
    this so that invalid-UTF-8 exploration is left to the ``*_bytes`` harnesses
    that specifically exercise the decoder.
    """
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return None
