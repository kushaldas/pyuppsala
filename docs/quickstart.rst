Quick start
===========

Installation
------------

Install from PyPI::

    pip install pyuppsala

Or with `uv <https://docs.astral.sh/uv/>`_::

    uv add pyuppsala

Requirements: Python 3.10 or later. No C compiler needed -- the package ships
pre-built wheels compiled from Rust.

Parse an XML document
---------------------

.. code-block:: python

    from pyuppsala import Document

    doc = Document("<root><child>hello</child></root>")
    el = doc.document_element
    print(el.tag.local_name)      # "root"
    print(el.text_content)        # "hello"

You can also parse from bytes (UTF-8 and UTF-16 are auto-detected):

.. code-block:: python

    from pyuppsala import parse_bytes

    doc = parse_bytes(b"<root>ok</root>")

Quick text access with element_text
------------------------------------

For simple elements like ``<name>value</name>``, use :attr:`~Node.element_text`
instead of :attr:`~Node.text_content` -- it returns the text of the first
Text/CDATA child without recursing:

.. code-block:: python

    from pyuppsala import Document

    doc = Document("<person><name>Alice</name><age>30</age></person>")
    root = doc.document_element

    for child in root:
        print(f"{child.tag.local_name} = {child.element_text}")
    # name = Alice
    # age = 30

Source tracking
---------------

Every parsed node remembers its position in the original input. You can
retrieve the original source text and byte ranges:

.. code-block:: python

    from pyuppsala import Document

    xml = '<root><item id="1">hello</item><item id="2">world</item></root>'
    doc = Document(xml)

    # The full original input
    print(doc.input_text == xml)  # True

    # Source text of a specific node
    item = doc.document_element.children[0]
    print(item.source)  # '<item id="1">hello</item>'

    # Byte range for slicing
    start, end = item.source_range
    print(xml[start:end])  # '<item id="1">hello</item>'

Query with XPath
----------------

.. code-block:: python

    from pyuppsala import Document, XPathEvaluator

    doc = Document("""\
    <bookstore>
      <book category="fiction">
        <title>The Great Gatsby</title>
      </book>
      <book category="non-fiction">
        <title>A Brief History of Time</title>
      </book>
    </bookstore>
    """)
    doc.prepare_xpath()

    xpath = XPathEvaluator()

    # Select nodes
    books = xpath.select(doc, "//book")
    print(len(books))  # 2

    # Evaluate to a string
    title = xpath.evaluate(doc, "string(//book[@category='fiction']/title)")
    print(title)  # "The Great Gatsby"

    # Evaluate to a number
    count = xpath.evaluate(doc, "count(//book)")
    print(count)  # 2.0

    # Evaluate to a boolean
    has_fiction = xpath.evaluate(doc, "boolean(//book[@category='fiction'])")
    print(has_fiction)  # True

Namespace-aware XPath requires registering prefixes:

.. code-block:: python

    doc = Document('<root xmlns:ns="urn:test"><ns:item/></root>')
    doc.prepare_xpath()

    xpath = XPathEvaluator()
    xpath.add_namespace("ns", "urn:test")
    nodes = xpath.select(doc, "/root/ns:item")

Find child elements by namespace
---------------------------------

For direct child lookups by namespace URI and local name, use
:meth:`~Node.first_child_element_by_name_ns` and
:meth:`~Node.child_elements_by_name_ns`:

.. code-block:: python

    from pyuppsala import Document

    xml = """\
    <root xmlns:a="urn:example">
      <a:item>first</a:item>
      <a:other>skip</a:other>
      <a:item>second</a:item>
    </root>
    """
    doc = Document(xml)
    root = doc.document_element

    # Get the first matching child
    first = root.first_child_element_by_name_ns("urn:example", "item")
    print(first.element_text)  # "first"

    # Get all matching children
    items = root.child_elements_by_name_ns("urn:example", "item")
    print(len(items))  # 2

Check element names with matches_name_ns
------------------------------------------

.. code-block:: python

    from pyuppsala import Document

    xml = '<saml:Assertion xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion">ok</saml:Assertion>'
    doc = Document(xml)
    root = doc.document_element

    if root.matches_name_ns("urn:oasis:names:tc:SAML:2.0:assertion", "Assertion"):
        print("This is a SAML Assertion")

Validate with XSD
-----------------

.. code-block:: python

    from pyuppsala import XsdValidator

    schema = """\
    <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:element name="greeting" type="xs:string"/>
    </xs:schema>
    """

    validator = XsdValidator(schema)

    # Quick boolean check
    print(validator.is_valid_str("<greeting>Hello</greeting>"))  # True

    # Detailed error list
    errors = validator.validate_str("<greeting><bad/></greeting>")
    for err in errors:
        print(err)  # prints line:column: message

Build XML without a DOM
-----------------------

.. code-block:: python

    from pyuppsala import XmlWriter

    w = XmlWriter()
    w.write_declaration()
    w.start_element("catalog", [("xmlns", "urn:example")])
    w.start_element("item", [("id", "1")])
    w.text("Widget")
    w.end_element("item")
    w.end_element("catalog")
    print(w.to_string())

Mutate the DOM
--------------

.. code-block:: python

    from pyuppsala import Document

    doc = Document("<root><a/></root>")
    root = doc.document_element

    # Create and attach new nodes
    b = doc.create_element("b")
    doc.append_child(root, b)

    text = doc.create_text("hello")
    doc.append_child(b, text)

    # Detach and reattach
    doc.detach(b)
    doc.insert_before(root, b, root.children[0])

    print(doc.to_xml())

QName matching
--------------

.. code-block:: python

    from pyuppsala import QName

    q = QName("Envelope", namespace_uri="http://schemas.xmlsoap.org/soap/envelope/", prefix="soap")

    # Match by local name and namespace
    print(q.matches("Envelope", namespace_uri="http://schemas.xmlsoap.org/soap/envelope/"))  # True
    print(q.matches("Envelope"))  # False -- namespace doesn't match None
    print(q.matches("Body", namespace_uri="http://schemas.xmlsoap.org/soap/envelope/"))  # False
