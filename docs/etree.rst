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

    parser = ET.XMLParser(huge_tree=True)              # lift depth/expansion caps
    parser = ET.XMLParser(max_depth=256, remove_comments=True)

    root = ET.fromstring(deeply_nested_xml, parser)

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
  plus :class:`XPath` / :class:`ETXPath` / :func:`XPathEvaluator`.
- **Parser & validation**: :class:`XMLParser`, :func:`register_namespace`, and
  :class:`XMLSchema` (wrapping :class:`pyuppsala.XsdValidator`).
- **Cross-tree moves**: appending an element from another tree deep-copies the
  subtree into the target document and preserves Python object identity.
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

Cosmetic options without an Uppsala equivalent (``compact``, ``collect_ids``,
``no_network``, ``ns_clean``) are accepted and ignored.

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
