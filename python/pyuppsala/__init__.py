"""pyuppsala - Python bindings for the Uppsala pure-Rust XML library.

The native classes, functions, constants, and exceptions live in the compiled
``pyuppsala._pyuppsala`` extension and are re-exported here so ``import pyuppsala``
continues to expose them at the top level. The :mod:`pyuppsala.etree` submodule
provides an ``lxml.etree``-compatible API layered on top of these primitives.
"""

from __future__ import annotations

from ._pyuppsala import (
    # Classes
    Document,
    Node,
    QName,
    Attribute,
    XPathEvaluator,
    XsdValidator,
    ValidationError,
    XmlWriter,
    XsdRegex,
    # Functions
    parse,
    parse_bytes,
    # Resource-limit constants
    DEFAULT_MAX_DEPTH,
    DEFAULT_MAX_ENTITY_EXPANSION,
    DEFAULT_MAX_XPATH_DEPTH,
    DEFAULT_MAX_REGEX_GROUP_DEPTH,
    DEFAULT_MAX_REGEX_STEPS,
    # Exceptions
    XmlParseError,
    XmlWellFormednessError,
    XmlNamespaceError,
    XPathError,
    XsdValidationError,
)

from . import etree  # noqa: F401  (registers the submodule on import)

__all__ = [
    "Document",
    "Node",
    "QName",
    "Attribute",
    "XPathEvaluator",
    "XsdValidator",
    "ValidationError",
    "XmlWriter",
    "XsdRegex",
    "parse",
    "parse_bytes",
    "DEFAULT_MAX_DEPTH",
    "DEFAULT_MAX_ENTITY_EXPANSION",
    "DEFAULT_MAX_XPATH_DEPTH",
    "DEFAULT_MAX_REGEX_GROUP_DEPTH",
    "DEFAULT_MAX_REGEX_STEPS",
    "XmlParseError",
    "XmlWellFormednessError",
    "XmlNamespaceError",
    "XPathError",
    "XsdValidationError",
    "etree",
]
