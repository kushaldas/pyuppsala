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
    Xslt,
    # Functions
    parse,
    parse_bytes,
    parse_many,
    # Resource-limit constants
    DEFAULT_MAX_DEPTH,
    DEFAULT_MAX_ENTITY_EXPANSION,
    DEFAULT_MAX_ENTITY_DEPTH,
    DEFAULT_MAX_XPATH_DEPTH,
    DEFAULT_MAX_XPATH_NODE_VISITS,
    DEFAULT_MAX_REGEX_GROUP_DEPTH,
    DEFAULT_MAX_REGEX_STEPS,
    DEFAULT_MAX_XSLT_DEPTH,
    # Exceptions
    XmlParseError,
    XmlWellFormednessError,
    XmlNamespaceError,
    XPathError,
    XsdValidationError,
)

# Native fetch APIs exist only when the extension was built with the
# default-on "net" cargo feature; a network-free build (maturin
# --no-default-features, e.g. for distro packaging) simply lacks them.
try:
    from ._pyuppsala import FetchResult, fetch_and_parse_many, fetch_many  # noqa: F401

    _HAS_NET = True
except ImportError:
    _HAS_NET = False

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
    "Xslt",
    "parse",
    "parse_bytes",
    "parse_many",
    "DEFAULT_MAX_DEPTH",
    "DEFAULT_MAX_ENTITY_EXPANSION",
    "DEFAULT_MAX_ENTITY_DEPTH",
    "DEFAULT_MAX_XPATH_DEPTH",
    "DEFAULT_MAX_XPATH_NODE_VISITS",
    "DEFAULT_MAX_REGEX_GROUP_DEPTH",
    "DEFAULT_MAX_REGEX_STEPS",
    "DEFAULT_MAX_XSLT_DEPTH",
    "XmlParseError",
    "XmlWellFormednessError",
    "XmlNamespaceError",
    "XPathError",
    "XsdValidationError",
    "etree",
]

if _HAS_NET:
    __all__ += ["FetchResult", "fetch_many", "fetch_and_parse_many"]
