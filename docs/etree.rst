The ``pyuppsala.etree`` module
==============================

``pyuppsala.etree`` provides an API compatible with `lxml.etree
<https://lxml.de/>`_, layered on Uppsala's secure, pure-Rust parser. Much
existing lxml code runs unchanged after swapping the import::

    # from lxml import etree
    from pyuppsala import etree

    root = etree.fromstring("<a><b>hello</b></a>")
    print(root.find("b").text)   # hello

Elements are *live views* over a backing native document (just like lxml's
``_Element`` objects are views over a libxml2 tree). Object identity is stable,
so ``root[0] is root[0]`` and ``root.find("b") is root[0]`` both hold.

Quick start
-----------

.. code-block:: python

    from pyuppsala import etree as ET

    # Parse
    root = ET.fromstring("<catalog><book id='1'>Dune</book></catalog>")
    book = root.find("book")
    assert book.text == "Dune"
    assert book.get("id") == "1"

    # Build
    cat = ET.Element("catalog")
    b = ET.SubElement(cat, "book", {"id": "2"})
    b.text = "Neuromancer"
    print(ET.tostring(cat, encoding="unicode"))
    # <catalog><book id="2">Neuromancer</book></catalog>

    # Namespaces (Clark notation)
    ns = ET.Element("{http://example.com/ns}root", nsmap={"e": "http://example.com/ns"})
    ET.SubElement(ns, "{http://example.com/ns}item")
    print(ET.tostring(ns, encoding="unicode"))
    # <e:root xmlns:e="http://example.com/ns"><e:item/></e:root>

Secure parsing
--------------

The same resource limits that protect :func:`pyuppsala.parse` apply here.
Billion-laughs entity expansion and pathologically deep nesting are rejected by
default. Use :class:`~pyuppsala.etree.XMLParser` to adjust limits:

.. code-block:: python

    from pyuppsala import etree as ET

    parser = ET.XMLParser(
        max_depth=256,
        remove_comments=True,
        forbid_dtd=True,
        forbid_entities=True,
    )

    root = ET.fromstring(deeply_nested_xml, parser)

.. important::

   ``XMLParser()`` keeps Uppsala's safe parser defaults: depth and entity
   expansion are capped, namespace processing is enabled, and no network fetches
   are performed by parsing. ``forbid_dtd`` and ``forbid_entities`` default to
   ``False`` for lxml compatibility; set them to ``True`` for untrusted XML that
   should not contain DTDs or entity declarations.

.. important::

   ``huge_tree=True`` deliberately lifts the parser depth and entity-expansion
   caps for lxml compatibility. Use it only for trusted documents. Prefer
   explicit ``max_depth`` / ``max_entity_expansion`` values when you know the
   expected upper bound.

XPath and XInclude security defaults
------------------------------------

``.xpath()``, :class:`XPath`, :class:`ETXPath`, and :func:`XPathEvaluator` use
the native XPath engine. By default, the etree compatibility layer keeps the
native per-evaluation node-visit budget
(:data:`pyuppsala.DEFAULT_MAX_XPATH_NODE_VISITS`); raise
``pyuppsala.etree.MAX_XPATH_NODE_VISITS`` only for trusted large documents.

XInclude processing is explicit: call ``tree.xinclude()`` or ``element.xinclude()``
when you want to process ``xi:include`` elements. Remote ``http(s)``/``ftp``
includes are blocked by default and require ``network_access=True``. Local file
targets must stay under the including document's base directory after symlink
resolution, and every include target is size-limited before buffering.

.. important::

   Do not run XInclude over untrusted XML with ``network_access=True`` unless
   your application has already applied an allowlist for remote destinations.
   The default is local-only, sandboxed, and size-limited.

XSLT security defaults
----------------------

:class:`XSLT` compiles stylesheets through the native :class:`pyuppsala.Xslt`
engine. Stylesheets and source documents are parsed with the native parser
resource caps, and template recursion uses the native
:data:`pyuppsala.DEFAULT_MAX_XSLT_DEPTH` cap by default.

.. important::

   Treat XSLT stylesheets as trusted application configuration. The lxml
   compatibility wrapper defaults ``regexp=True`` and enables the supported
   EXSLT regexp functions. Passing ``regexp=False`` raises
   ``NotImplementedError`` rather than silently ignoring a request to disable
   them. Custom extension functions and XSLT access-control objects are not
   supported.

Native fast scans
-----------------

The normal ``etree`` traversal APIs return live ``_Element`` proxies, matching
lxml. That is the right default when you need to inspect elements, mutate the
tree, run complex Python predicates, or keep code portable between lxml and
pyuppsala.

For large trees where the loop body is only a simple aggregate or fixed-shape
extraction, pyuppsala also exposes native ``fast_*`` methods on ``_Element``.
These are pyuppsala extensions, not lxml APIs. They run the full descendant walk
in Rust under one document lock with the GIL released, and they avoid creating a
Python proxy for every matching node.

All ``fast_*`` methods use the same tag matching rules as ``Element.iter(tag)``:

* ``tag=None`` scans this element and all descendants, including comments and
  processing instructions.
* ``tag="*"`` matches element nodes only.
* ``tag="item"``, ``tag="{urn:example}item"``, and :class:`QName` match named
  elements.
* The current element is included, just like ``Element.iter()``.

Use ``fast_count`` when a Python loop only counts matching nodes:

.. code-block:: python

    from pyuppsala import etree as ET

    root = ET.fromstring(
        "<catalog>"
        "<book id='1' pages='412'/>"
        "<book id='2' pages='271'/>"
        "<magazine id='m1'/>"
        "</catalog>"
    )

    # Equivalent to: sum(1 for _ in root.iter("book"))
    assert root.fast_count("book") == 2

    # "*" is an element-only wildcard and includes the root element.
    assert root.fast_count("*") == 4

Use ``fast_has`` when you only need to know whether a match exists. It stops as
soon as the first match is found:

.. code-block:: python

    # Equivalent to: next(root.iter("book"), None) is not None
    if root.fast_has("book"):
        print("catalog has books")

Use ``fast_sum_int_attr`` when every matching attribute value is expected to be
an integer and the desired result is the sum. Missing attributes are skipped; a
present non-integer value raises :class:`ValueError`.

.. code-block:: python

    # Equivalent to:
    # sum(int(el.get("pages")) for el in root.iter("book")
    #     if el.get("pages") is not None)
    assert root.fast_sum_int_attr("pages", "book") == 683

Use ``fast_collect_attr`` when the loop only gathers one attribute from matching
elements. Missing attributes are skipped and the returned values are Python
strings:

.. code-block:: python

    # Equivalent to:
    # [el.get("id") for el in root.iter("book") if el.get("id") is not None]
    assert root.fast_collect_attr("id", "book") == ["1", "2"]

    nsroot = ET.fromstring(
        "<r xmlns:p='urn:parts'>"
        "<item p:code='A'/>"
        "<item p:code='B'/>"
        "</r>"
    )
    assert nsroot.fast_collect_attr(ET.QName("urn:parts", "code"), "item") == [
        "A",
        "B",
    ]

Use ``fast_collect_grouped_text`` for the SAML EntityAttributes-style nested
shape: find each ``group_tag`` descendant, then each ``item_tag`` descendant
inside that group, read ``key`` from the item, and collect stripped leading text
from each ``value_tag`` descendant of that item. It returns one
``(attribute_value_or_None, values)`` tuple per item.

.. code-block:: python

    root = ET.fromstring(
        "<md:EntityDescriptor "
        "xmlns:md='urn:oasis:names:tc:SAML:2.0:metadata' "
        "xmlns:mdattr='urn:oasis:names:tc:SAML:metadata:attribute' "
        "xmlns:saml='urn:oasis:names:tc:SAML:2.0:assertion'>"
        "<md:Extensions>"
        "<mdattr:EntityAttributes>"
        "<saml:Attribute Name='category'>"
        "<saml:AttributeValue> one </saml:AttributeValue>"
        "<saml:AttributeValue>two</saml:AttributeValue>"
        "</saml:Attribute>"
        "<saml:Attribute>"
        "<saml:AttributeValue> missing-name </saml:AttributeValue>"
        "</saml:Attribute>"
        "</mdattr:EntityAttributes>"
        "</md:Extensions>"
        "</md:EntityDescriptor>"
    )

    groups = root.fast_collect_grouped_text(
        "{urn:oasis:names:tc:SAML:metadata:attribute}EntityAttributes",
        "{urn:oasis:names:tc:SAML:2.0:assertion}Attribute",
        "Name",
        "{urn:oasis:names:tc:SAML:2.0:assertion}AttributeValue",
    )

    assert groups == [
        ("category", ["one", "two"]),
        (None, ["missing-name"]),
    ]

``fast_collect_grouped_text`` is deliberately narrower than XPath or a general
Python loop: it reads leading text directly under each value element, strips
that text, and does not expose the intermediate elements. Use ``iter()``,
``findall()``, or ``xpath()`` instead when you need element proxies, recursive
string values, predicates, mutation, sibling/tail handling, or lxml-compatible
source portability.

Supported features
------------------

- **Elements**: ``tag`` (Clark ``{uri}local`` notation), ``text``, ``tail``,
  ``attrib``, ``get``/``set``/``keys``/``values``/``items``, indexing and
  slicing, ``append``/``insert``/``remove``/``extend``/``replace``,
  ``getparent``/``getnext``/``getprevious``/``getroottree``, ``makeelement``,
  ``addnext``/``addprevious``, ``nsmap``, ``prefix``, ``sourceline``.
- **Factories**: :func:`Element`, :func:`SubElement`, :func:`Comment`,
  :func:`ProcessingInstruction` / ``PI``, :class:`QName`, :func:`ElementTree`.
- **I/O**: :func:`fromstring` / ``XML``, :func:`fromstringlist`, :func:`parse`,
  :func:`tostring` (``method="xml"`` only), :func:`tounicode`, :func:`dump`,
  :func:`indent`. As in lxml, :func:`fromstring` takes in-memory XML while
  :func:`parse` takes a filename/path or a file-like object (wrap in-memory
  data in ``io.BytesIO`` to use it). Byte input is decoded by Uppsala (UTF-8
  and UTF-16, with or without a BOM); ``XMLParser(encoding=...)`` overrides the
  declared encoding for byte input.
- **Search**: ``find`` / ``findall`` / ``findtext`` / ``iterfind`` (ElementPath),
  ``iter`` / ``itertext``, and full ``.xpath()`` via Uppsala's XPath 1.0 engine,
  plus :class:`XPath` / :class:`ETXPath` / :func:`XPathEvaluator`. pyuppsala also
  provides native bulk-scan extensions: ``fast_count``, ``fast_has``,
  ``fast_sum_int_attr``, ``fast_collect_attr``, and
  ``fast_collect_grouped_text``.
- **Parser & validation**: :class:`XMLParser`, :func:`register_namespace`, and
  :class:`XMLSchema` (wrapping :class:`pyuppsala.XsdValidator`).
- **XSLT**: :class:`XSLT` supports native XSLT 1.0 transforms with EXSLT regexp
  compatibility enabled by default.
- **Cross-tree moves**: appending an element from another tree deep-copies the
  subtree into the target document and preserves Python object identity.
  Native ``NodeId`` values are document-scoped; low-level native mutators reject
  foreign node handles, and the etree layer uses deep-copy/import behavior for
  cross-tree operations.
- **DOCTYPE**: ``tree.docinfo.doctype`` returns the ``<!DOCTYPE ...>``
  declaration preserved from the source (``""`` when absent). Serializing a
  whole :class:`_ElementTree` round-trips that DOCTYPE; serializing a bare
  element omits it. :func:`tostring` also accepts a ``doctype=<str>`` argument
  to inject a custom declaration, matching lxml. The DOCTYPE is preserved
  verbatim and not otherwise processed (no DTD validation or entity loading).

Exceptions
----------

``pyuppsala.etree`` exposes an lxml-style hierarchy. Parsing errors raise
:class:`XMLSyntaxError` (also available as ``ParseError``); all etree exceptions
derive from :class:`LxmlError`.

.. code-block:: python

    from pyuppsala import etree as ET

    try:
        ET.fromstring("<a></b>")
    except ET.XMLSyntaxError as exc:
        print("bad XML:", exc)

.. list-table::
   :header-rows: 1
   :widths: 40 60

   * - Exception
     - Raised when
   * - ``LxmlError`` (alias ``Error``)
     - Base class for all etree exceptions
   * - ``XMLSyntaxError`` (alias ``ParseError``)
     - Parsing / well-formedness failure
   * - ``XPathError`` / ``XPathEvalError``
     - XPath evaluation failure
   * - ``DocumentInvalid``
     - ``XMLSchema.assertValid`` on an invalid document
   * - ``XMLSchemaParseError``
     - An XSD schema cannot be built

Unsupported in v1
-----------------

The following lxml features are **not** part of the first release. Options that
would silently change parsing correctness raise ``NotImplementedError`` rather
than being ignored:

- ``XMLParser(recover=True)`` -- error-recovery parsing
- DTD processing (``dtd_validation``, ``load_dtd``, ``resolve_entities=False``)
- custom URI resolvers and parser ``target`` objects
- ``tostring(method=...)`` other than ``"xml"`` (``"html"``, ``"text"``,
  ``"c14n"`` raise ``NotImplementedError``)
- XPath variable binding (passing ``$name`` keyword arguments to ``.xpath()``)
- ``iterparse``, C14N / ``canonicalize``
- RelaxNG, Schematron, and DTD schema classes (only :class:`XMLSchema` /
  XSD is provided)

Cosmetic options without an Uppsala equivalent (``collect_ids``, ``no_network``,
``ns_clean``) are accepted and ignored. ``compact=True`` is honored for etree
parsing by discarding the retained source buffer after parse-time cleanup; pass
``compact=False`` if you need source-inspection helpers to retain the decoded
input text.

.. note::

   As with :class:`pyuppsala.XsdValidator`, XSD schemas passed to
   :class:`XMLSchema` must **not** include an ``<?xml version="1.0"?>``
   declaration.

API reference
-------------

.. currentmodule:: pyuppsala.etree

.. autofunction:: fromstring
.. autofunction:: parse
.. autofunction:: tostring
.. autofunction:: Element
.. autofunction:: SubElement
.. autofunction:: Comment
.. autofunction:: ProcessingInstruction
.. autofunction:: register_namespace
.. autoclass:: QName
   :members:
.. autoclass:: DocInfo
   :members:
.. autoclass:: XMLParser
   :members:
.. autoclass:: XMLSchema
   :members:
.. autoclass:: XSLT
   :members:
