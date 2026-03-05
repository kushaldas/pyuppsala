API reference
=============

Module-level functions
----------------------

.. function:: parse(xml: str) -> Document

   Parse an XML string and return a :class:`Document`.

   :param xml: A well-formed XML string.
   :raises XmlParseError: If the XML is syntactically malformed.
   :raises XmlWellFormednessError: If a well-formedness constraint is violated.

.. function:: parse_bytes(data: bytes) -> Document

   Parse XML from bytes with automatic encoding detection (UTF-8, UTF-16 LE/BE).

   :param data: Raw bytes of an XML document.
   :raises XmlParseError: If the XML is malformed.

Document
--------

.. class:: Document(xml: str)

   Parse an XML string into a DOM document.

   :param xml: A well-formed XML string.

   .. staticmethod:: from_bytes(data: bytes) -> Document

      Parse XML from bytes with automatic encoding detection.

   .. staticmethod:: empty() -> Document

      Create a new empty document with no document element.

   .. attribute:: root
      :type: Node

      The root node (the Document node itself). This is the parent of the
      document element, processing instructions, and comments at the top level.

   .. attribute:: document_element
      :type: Node | None

      The document element (top-level element), or ``None`` for empty documents.

   .. method:: get_elements_by_tag_name(name: str) -> list[Node]

      Find all elements in the document with the given local tag name.

   .. method:: get_elements_by_tag_name_ns(namespace_uri: str, name: str) -> list[Node]

      Find all elements with the given namespace URI and local tag name.

   **Tree mutation**

   .. method:: create_element(local_name, namespace_uri=None, prefix=None) -> Node

      Create a new element node. The node is not yet attached to the tree;
      use :meth:`append_child`, :meth:`insert_before`, or :meth:`insert_after`
      to place it.

   .. method:: create_text(text: str) -> Node

      Create a new text node.

   .. method:: create_comment(text: str) -> Node

      Create a new comment node.

   .. method:: create_cdata(text: str) -> Node

      Create a new CDATA section node.

   .. method:: create_processing_instruction(target: str, data: str | None = None) -> Node

      Create a new processing instruction node.

   .. method:: append_child(parent: Node, child: Node) -> None

      Append *child* as the last child of *parent*.

   .. method:: insert_before(parent: Node, new_child: Node, reference: Node) -> None

      Insert *new_child* before *reference* under *parent*.

   .. method:: insert_after(parent: Node, new_child: Node, reference: Node) -> None

      Insert *new_child* after *reference* under *parent*.

   .. method:: remove_child(parent: Node, child: Node) -> None

      Remove *child* from *parent*.

   .. method:: replace_child(parent: Node, new_child: Node, old_child: Node) -> None

      Replace *old_child* with *new_child* under *parent*.

   .. method:: detach(node: Node) -> None

      Detach *node* from its parent. The node remains valid and can be
      re-attached elsewhere.

   **Serialization**

   .. method:: to_xml() -> str

      Serialize the document to a compact XML string.

   .. method:: to_xml_with_options(indent=None, expand_empty_elements=False) -> str

      Serialize with formatting options.

      :param indent: Indentation string (e.g. ``"  "``), or ``None`` for compact output.
      :param expand_empty_elements: If ``True``, write ``<foo></foo>`` instead of ``<foo/>``.

   .. method:: write_to_file(path: str) -> None

      Write the serialized document to a file.

   **XPath**

   .. method:: prepare_xpath() -> None

      Build internal indices required for XPath evaluation. Call this once
      before using :class:`XPathEvaluator` on this document. If you modify
      the DOM after calling this, call it again.

Node
----

.. class:: Node

   A lightweight handle to a node in a :class:`Document`. Nodes do not own
   their data -- the ``Document`` does. Do not use a ``Node`` after its parent
   ``Document`` has been garbage-collected.

   .. attribute:: kind
      :type: str

      The kind of this node. One of: ``"document"``, ``"element"``, ``"text"``,
      ``"comment"``, ``"processing_instruction"``, ``"cdata"``, ``"attribute"``.

   .. attribute:: tag
      :type: QName | None

      The tag name for element nodes, or ``None`` for other kinds.

   .. attribute:: text
      :type: str | None

      The text content for text, comment, and CDATA nodes, or ``None``.

   .. attribute:: text_content
      :type: str

      Recursively collected text content of this node and all descendants.

   .. attribute:: attributes
      :type: list[Attribute]

      The list of attributes for element nodes (empty list for non-elements).

   .. attribute:: parent
      :type: Node | None

      The parent node, or ``None`` for the document root.

   .. attribute:: children
      :type: list[Node]

      The child nodes.

   .. attribute:: line
      :type: int

      The line number of this node in the original source (1-based).

   .. attribute:: column
      :type: int

      The column number of this node in the original source.

   .. method:: get_attribute(name: str, namespace_uri: str | None = None) -> str | None

      Get an attribute value by local name, optionally filtered by namespace.

   .. method:: set_attribute(name, value, namespace_uri=None, prefix=None) -> str | None

      Set an attribute. Returns the previous value, or ``None``.

   .. method:: remove_attribute(name: str) -> str | None

      Remove an attribute by local name. Returns the old value, or ``None``.

   .. method:: to_xml() -> str

      Serialize this node and its subtree to XML.

   .. method:: to_xml_with_options(indent=None, expand_empty_elements=False) -> str

      Serialize this subtree with formatting options.

   .. method:: get_elements_by_tag_name(name: str) -> list[Node]

      Find descendant elements by local tag name.

   .. method:: get_elements_by_tag_name_ns(namespace_uri: str, name: str) -> list[Node]

      Find descendant elements by namespace URI and local tag name.

   **Protocols**

   - ``len(node)`` returns the number of child nodes.
   - ``for child in node`` iterates over children.
   - ``node[i]`` returns the *i*-th child (supports negative indices).
   - ``bool(node)`` is always ``True``.
   - ``str(node)`` returns :meth:`to_xml`.
   - ``repr(node)`` returns a short description like ``Node(<root>)``.

QName
-----

.. class:: QName(local_name, namespace_uri=None, prefix=None)

   A qualified XML name.

   .. attribute:: local_name
      :type: str

   .. attribute:: namespace_uri
      :type: str | None

   .. attribute:: prefix
      :type: str | None

   .. attribute:: prefixed_name
      :type: str

      The prefixed form (e.g. ``"ns:item"``) or just the local name.

   Equality is by ``local_name`` and ``namespace_uri`` (prefix is ignored).
   QNames are hashable.

Attribute
---------

.. class:: Attribute

   An XML attribute.

   .. attribute:: name
      :type: QName

   .. attribute:: value
      :type: str

XPathEvaluator
--------------

.. class:: XPathEvaluator()

   XPath 1.0 expression evaluator.

   .. method:: add_namespace(prefix: str, uri: str) -> None

      Register a namespace prefix for use in XPath expressions.

   .. method:: evaluate(doc, expr, context=None) -> list[Node] | bool | float | str

      Evaluate an XPath expression. The return type depends on the XPath
      result type:

      - **Node-set** -> ``list[Node]``
      - **Boolean** -> ``bool``
      - **Number** -> ``float``
      - **String** -> ``str``

      :param doc: The :class:`Document` to query (must have :meth:`~Document.prepare_xpath` called).
      :param expr: An XPath 1.0 expression string.
      :param context: Optional context node. Defaults to the document root.
      :raises XPathError: If the expression is invalid.

   .. method:: select(doc, expr, context=None) -> list[Node]

      Evaluate an XPath expression and return matching nodes. This is a
      convenience method equivalent to ``evaluate()`` when the result is a
      node-set.

XsdValidator
------------

.. class:: XsdValidator(schema_xml: str)

   XSD schema validator. Supports XSD structures and datatypes, 40+ built-in
   types, facets, complex types, extensions, restrictions, list types,
   wildcards, substitution groups, and identity constraints.

   :param schema_xml: An XSD schema as an XML string.

   .. staticmethod:: from_file(schema_xml: str, base_path: str) -> XsdValidator

      Create a validator that resolves ``xs:include``, ``xs:import``, and
      ``xs:redefine`` relative to *base_path*.

   .. method:: validate(doc: Document) -> list[ValidationError]

      Validate a parsed document. Returns a list of errors (empty = valid).

   .. method:: validate_str(xml: str) -> list[ValidationError]

      Parse and validate an XML string in one step.

   .. method:: is_valid(doc: Document) -> bool

      Quick boolean check.

   .. method:: is_valid_str(xml: str) -> bool

      Quick boolean check from a string. Returns ``False`` for malformed XML
      instead of raising.

   .. method:: set_enforce_qname_length_facets(enforce: bool) -> None

      Configure whether length facets on QName/NOTATION types are enforced.
      Enabled by default. See `W3C Bug #4009
      <https://www.w3.org/Bugs/Public/show_bug.cgi?id=4009>`_.

ValidationError
---------------

.. class:: ValidationError

   A single XSD validation error.

   .. attribute:: message
      :type: str

   .. attribute:: line
      :type: int | None

   .. attribute:: column
      :type: int | None

XmlWriter
---------

.. class:: XmlWriter()

   An imperative XML builder for constructing XML fragments without a DOM.

   .. method:: write_declaration() -> None

      Write ``<?xml version="1.0" encoding="UTF-8"?>``.

   .. method:: write_declaration_full(version="1.0", encoding=None, standalone=None) -> None

      Write an XML declaration with custom parameters.

   .. method:: start_element(name, attrs=None) -> None

      Start an element. *attrs* is an optional list of ``(name, value)`` tuples.

   .. method:: end_element(name: str) -> None

      Close the element.

   .. method:: empty_element(name, attrs=None) -> None

      Write a self-closing element: ``<name/>``.

   .. method:: empty_element_expanded(name, attrs=None) -> None

      Write an expanded empty element: ``<name></name>``.

   .. method:: text(content: str) -> None

      Write text content (auto-escaped).

   .. method:: cdata(content: str) -> None

      Write a CDATA section.

   .. method:: comment(content: str) -> None

      Write a comment.

   .. method:: processing_instruction(target, data=None) -> None

      Write a processing instruction.

   .. method:: raw(xml: str) -> None

      Write raw XML content (not escaped).

   .. method:: to_string() -> str

      Return the accumulated XML as a string.

   .. method:: to_bytes() -> bytes

      Return the accumulated XML as bytes.

   **Protocols**

   - ``len(writer)`` returns the number of bytes written so far.
   - ``bool(writer)`` is ``True`` if any content has been written.
   - ``str(writer)`` returns :meth:`to_string`.

XsdRegex
--------

.. class:: XsdRegex(pattern: str)

   XSD regular expression pattern matcher. XSD regexes are implicitly
   anchored -- they must match the **entire** input string.

   Supported features: alternation (``|``), grouping, quantifiers
   (``*``, ``+``, ``?``, ``{n}``, ``{n,m}``), character classes with
   subtraction (``[a-z-[aeiou]]``), Unicode category escapes
   (``\p{Lu}``), Unicode block escapes (``\p{IsBasicLatin}``),
   multi-char escapes (``\d``, ``\s``, ``\w``, ``\i``, ``\c``).

   :param pattern: An XSD regex pattern string.
   :raises ValueError: If the pattern is invalid.

   .. method:: is_match(input: str) -> bool

      Test whether *input* fully matches the pattern.

   .. attribute:: pattern
      :type: str

      The original pattern string.
